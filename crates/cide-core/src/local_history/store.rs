use super::{HistoryEligibility, RootIdentity, validate_relative};
use cide_ipc::local_history::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA: u32 = 1;
const PAGE_SIZE: usize = 100;
fn initial_generation() -> u32 {
    1
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    schema: u32,
    canonical_root: Vec<u8>,
    generation: u32,
    #[serde(default = "initial_generation")]
    min_generation: u32,
    next_sequence: u32,
    policy: HistoryPolicy,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    schema: u32,
    generation: u32,
    revision: LocalRevision,
}

/// One owner per canonical root. Callers serialize access on a worker thread.
pub struct HistoryStore {
    pub eligibility: HistoryEligibility,
    dir: PathBuf,
    _ownership: File,
    meta: Metadata,
    records: BTreeMap<u32, LocalRevision>,
    ids: BTreeMap<String, u32>,
    paths: BTreeMap<String, Vec<u32>>,
    lineage: BTreeMap<String, Vec<u32>>,
    pub coverage: HistoryCoverage,
    revision: u32,
    pins: BTreeMap<String, u32>,
    pending_safety: BTreeMap<String, LocalRevisionId>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn private_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    crate::persist::write_atomic(path, &serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

impl HistoryStore {
    pub fn profile_path(root: &RootIdentity) -> PathBuf {
        crate::persist::state_dir()
            .join("local-history")
            .join(&root.id.0)
    }
    pub fn open_profile(root: RootIdentity) -> Result<Self, String> {
        let dir = Self::profile_path(&root);
        Self::open(root, dir)
    }
    pub fn open_background(
        root: RootIdentity,
        dir: PathBuf,
    ) -> std::thread::JoinHandle<Result<Self, String>> {
        std::thread::spawn(move || Self::open(root, dir))
    }
    /// An explicit directory lets fixtures avoid the user's profile.
    pub fn open(root: RootIdentity, dir: PathBuf) -> Result<Self, String> {
        private_dir(&dir)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let ownership = options
            .open(dir.join("owner.lock"))
            .map_err(|e| e.to_string())?;
        ownership
            .try_lock()
            .map_err(|_| "Local History is owned by another process".to_owned())?;
        let metadata = dir.join("root.json");
        let reopened = metadata.exists();
        let mut meta: Metadata = if reopened {
            serde_json::from_slice(&fs::read(&metadata).map_err(|e| e.to_string())?)
                .map_err(|e| format!("Unavailable history metadata: {e}"))?
        } else {
            let meta = Metadata {
                schema: SCHEMA,
                canonical_root: root.path.as_os_str().as_encoded_bytes().to_vec(),
                generation: 1,
                min_generation: 1,
                next_sequence: 1,
                policy: HistoryPolicy::default(),
            };
            json(&metadata, &meta)?;
            meta
        };
        if meta.schema != SCHEMA {
            return Err("Unsupported Local History schema; history is unavailable".into());
        }
        if meta.canonical_root != root.path.as_os_str().as_encoded_bytes() {
            return Err("History root identity does not match this store".into());
        }
        let eligibility = HistoryEligibility::new(root, meta.policy.clone())?;
        if reopened {
            meta.generation = meta
                .generation
                .checked_add(1)
                .ok_or("History generation exhausted")?;
            json(&metadata, &meta)?;
        }
        private_dir(&dir.join("blobs"))?;
        private_dir(&dir.join("records"))?;
        let mut store = Self {
            eligibility,
            dir,
            _ownership: ownership,
            meta,
            records: BTreeMap::new(),
            ids: BTreeMap::new(),
            paths: BTreeMap::new(),
            lineage: BTreeMap::new(),
            coverage: HistoryCoverage::default(),
            revision: 0,
            pins: BTreeMap::new(),
            pending_safety: BTreeMap::new(),
        };
        store.load_index()?;
        Ok(store)
    }
    fn record_path(&self, r: &LocalRevision) -> PathBuf {
        self.dir
            .join("records")
            .join(format!("{:010}-{}.json", r.sequence, r.id.0))
    }
    fn load_index(&mut self) -> Result<(), String> {
        for entry in fs::read_dir(&self.dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "tmp") {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
        for entry in fs::read_dir(self.dir.join("records")).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "tmp") {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
                continue;
            }
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let loaded = (|| -> Result<Option<LocalRevision>, String> {
                let mut data = Vec::new();
                File::open(&path)
                    .map_err(|e| e.to_string())?
                    .take(65537)
                    .read_to_end(&mut data)
                    .map_err(|e| e.to_string())?;
                if data.len() > 65536 {
                    return Err("History record is too large".into());
                }
                let record: Record = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
                if record.schema != SCHEMA {
                    return Err("Unsupported revision schema".into());
                }
                if record.generation < self.meta.min_generation
                    || record.generation > self.meta.generation
                {
                    // A crash after the generation commit must not resurrect cleared records.
                    fs::remove_file(&path).map_err(|e| e.to_string())?;
                    return Ok(None);
                }
                let mut r = record.revision;
                validate_relative(Path::new(&r.path))?;
                if let Some(prior) = &r.prior_path {
                    validate_relative(Path::new(prior))?;
                }
                uuid::Uuid::parse_str(&r.id.0).map_err(|_| "Invalid revision ID")?;
                uuid::Uuid::parse_str(&r.lifecycle).map_err(|_| "Invalid lifecycle ID")?;
                if self.record_path(&r) != path
                    || self.records.contains_key(&r.sequence)
                    || self.ids.contains_key(&r.id.0)
                {
                    return Err("Invalid or duplicate record identity".into());
                }
                if let Err(reason) = self.bytes(&r.content) {
                    r.unavailable = Some(reason);
                }
                Ok(Some(r))
            })();
            match loaded {
                Ok(Some(r)) => {
                    self.meta.next_sequence = self.meta.next_sequence.max(
                        r.sequence
                            .checked_add(1)
                            .ok_or("History sequence exhausted")?,
                    );
                    if let Some(reason) = &r.unavailable {
                        self.coverage.degraded = Some(reason.clone());
                    }
                    self.insert(r);
                }
                Ok(None) => {}
                Err(reason) => {
                    self.coverage.degraded = Some(format!(
                        "Some history records could not be recovered: {reason}"
                    ));
                    // Keep the offending file for recovery; unrelated records remain readable.
                }
            }
        }
        self.rebuild_paths();
        self.gc_blobs()?;
        Ok(())
    }
    fn gc_blobs(&self) -> Result<(), String> {
        let referenced: HashSet<&str> = self
            .records
            .values()
            .filter_map(|r| match &r.content {
                HistoryContent::Content { digest, .. } => Some(digest.as_str()),
                _ => None,
            })
            .collect();
        for entry in fs::read_dir(self.dir.join("blobs")).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_none_or(|name| !referenced.contains(name))
            {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
        crate::persist::sync_dir(&self.dir.join("blobs")).map_err(|e| e.to_string())
    }
    fn insert(&mut self, r: LocalRevision) {
        self.ids.insert(r.id.0.clone(), r.sequence);
        self.records.insert(r.sequence, r);
    }
    fn rebuild_paths(&mut self) {
        self.paths.clear();
        self.lineage.clear();
        let mut lifecycles: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for r in self.records.values() {
            self.paths
                .entry(r.path.clone())
                .or_default()
                .push(r.sequence);
            lifecycles
                .entry(r.lifecycle.clone())
                .or_default()
                .push(r.sequence);
        }
        for (path, own) in &self.paths {
            let mut sequences = std::collections::BTreeSet::new();
            for n in own {
                sequences.extend(lifecycles[&self.records[n].lifecycle].iter().copied());
            }
            self.lineage
                .insert(path.clone(), sequences.into_iter().collect());
        }
    }
    pub fn known_paths(&self) -> Vec<String> {
        self.paths.keys().cloned().collect()
    }
    pub fn known_path_count(&self) -> u32 {
        self.paths.len().min(u32::MAX as usize) as u32
    }
    pub fn generation(&self) -> u32 {
        self.meta.generation
    }
    pub fn restart_observation(&mut self) -> Result<(), String> {
        let mut next = self.meta.clone();
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or("History generation exhausted")?;
        json(&self.dir.join("root.json"), &next)?;
        self.meta = next;
        Ok(())
    }
    pub fn status(&self) -> HistoryStatus {
        HistoryStatus {
            root: self.eligibility.root.id.clone(),
            canonical_root: self.eligibility.root.path.to_string_lossy().into_owned(),
            generation: self.generation(),
            revision: self.revision,
            used_bytes: self.used_bytes().min(u32::MAX as u64) as u32,
            policy: self.meta.policy.clone(),
            coverage: self.coverage.clone(),
        }
    }
    pub fn used_bytes(&self) -> u64 {
        let mut total = fs::metadata(self.dir.join("root.json"))
            .map(|m| m.len())
            .unwrap_or(0);
        for folder in ["records", "blobs"] {
            if let Ok(entries) = fs::read_dir(self.dir.join(folder)) {
                total += entries
                    .filter_map(Result::ok)
                    .filter_map(|e| e.metadata().ok())
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
                    .sum::<u64>();
            }
        }
        total
    }
    pub fn pin(&mut self, id: &LocalRevisionId) -> Result<(), String> {
        self.get(id)?;
        *self.pins.entry(id.0.clone()).or_default() += 1;
        Ok(())
    }
    pub fn unpin(&mut self, id: &LocalRevisionId) {
        if let Some(count) = self.pins.get_mut(&id.0) {
            *count -= 1;
            if *count == 0 {
                self.pins.remove(&id.0);
            }
        }
    }

    fn evict(&mut self, sequence: u32) -> Result<(), String> {
        let r = self.records.get(&sequence).ok_or("Missing revision")?;
        fs::remove_file(self.record_path(r)).map_err(|e| e.to_string())?;
        crate::persist::sync_dir(&self.dir.join("records")).map_err(|e| e.to_string())?;
        self.ids.remove(&r.id.0);
        self.records.remove(&sequence);
        self.rebuild_paths();
        self.gc_blobs()?;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    pub fn cleanup(&mut self, at_ms: u64) -> Result<(), String> {
        let cutoff = at_ms.saturating_sub(self.meta.policy.retention_days as u64 * 86_400_000);
        let expired: Vec<u32> = self
            .records
            .values()
            .filter(|r| r.observed_ms < cutoff && !self.pins.contains_key(&r.id.0))
            .map(|r| r.sequence)
            .collect();
        for n in expired {
            self.evict(n)?;
        }
        while self.used_bytes() > self.meta.policy.budget_bytes as u64 {
            let Some(n) = self
                .records
                .values()
                .find(|r| !self.pins.contains_key(&r.id.0))
                .map(|r| r.sequence)
            else {
                return Err("History budget cannot fit pinned revisions and metadata".into());
            };
            self.evict(n)?;
        }
        Ok(())
    }

    fn admit(&mut self, r: &LocalRevision) -> Result<(), String> {
        self.cleanup(now_ms())?;
        let record_bytes = serde_json::to_vec(&Record {
            schema: SCHEMA,
            generation: self.generation(),
            revision: r.clone(),
        })
        .map_err(|e| e.to_string())?
        .len() as u64;
        loop {
            let blob_bytes = match &r.content {
                HistoryContent::Content { digest, bytes }
                    if !self.dir.join("blobs").join(digest).exists() =>
                {
                    *bytes as u64
                }
                _ => 0,
            };
            // Leave space for the next sequence's metadata growth.
            if self.used_bytes() + record_bytes + blob_bytes + 16
                <= self.meta.policy.budget_bytes as u64
            {
                return Ok(());
            }
            let Some(n) = self
                .records
                .values()
                .find(|r| !self.pins.contains_key(&r.id.0))
                .map(|r| r.sequence)
            else {
                return Err(
                    "History budget cannot admit this revision; increase the budget".into(),
                );
            };
            self.evict(n)?;
        }
    }

    pub fn set_policy(&mut self, policy: HistoryPolicy) -> Result<HistoryStatus, String> {
        let eligibility = HistoryEligibility::new(self.eligibility.root.clone(), policy.clone())?;
        if self.meta.policy == policy {
            return Ok(self.status());
        }
        let mut next = self.meta.clone();
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or("History generation exhausted")?;
        next.policy = policy;
        json(&self.dir.join("root.json"), &next)?;
        self.meta = next;
        self.eligibility = eligibility;
        self.revision = self.revision.wrapping_add(1);
        if self.meta.policy.enabled
            && let Err(reason) = self.cleanup(now_ms())
        {
            self.coverage.degraded = Some(reason);
        }
        Ok(self.status())
    }

    pub fn clear(&mut self, confirmed: bool) -> Result<HistoryStatus, String> {
        if !confirmed {
            return Err("Confirm clearing this root's Local History".into());
        }
        if self
            .pins
            .keys()
            .any(|id| !self.pending_safety.values().any(|pending| &pending.0 == id))
        {
            return Err("A restore is using history; retry after it finishes".into());
        }
        let mut next = self.meta.clone();
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or("History generation exhausted")?;
        next.min_generation = next.generation;
        json(&self.dir.join("root.json"), &next)?;
        // The durable generation changes before any deletion; recovery ignores obsolete records.
        self.meta = next;
        self.records.clear();
        self.ids.clear();
        self.paths.clear();
        self.lineage.clear();
        self.pins.clear();
        self.pending_safety.clear();
        self.coverage = HistoryCoverage::default();
        self.revision = self.revision.wrapping_add(1);
        for entry in fs::read_dir(self.dir.join("records")).map_err(|e| e.to_string())? {
            fs::remove_file(entry.map_err(|e| e.to_string())?.path()).map_err(|e| e.to_string())?;
        }
        crate::persist::sync_dir(&self.dir.join("records")).map_err(|e| e.to_string())?;
        self.gc_blobs()?;
        Ok(self.status())
    }
    pub fn check_generation(&self, generation: u32) -> Result<(), String> {
        if generation != self.generation() {
            Err("History was cleared or capture policy changed; refresh the history view".into())
        } else {
            Ok(())
        }
    }
    pub fn get(&self, id: &LocalRevisionId) -> Result<&LocalRevision, String> {
        self.ids
            .get(&id.0)
            .and_then(|n| self.records.get(n))
            .ok_or_else(|| "This revision is no longer available; refresh history".into())
    }
    pub fn latest(&self, path: &str) -> Option<&LocalRevision> {
        self.paths
            .get(path)
            .and_then(|v| v.last())
            .and_then(|n| self.records.get(n))
    }
    pub fn revision_belongs(&self, path: &str, id: &LocalRevisionId) -> Result<(), String> {
        let revision = self.get(id)?;
        if self
            .lineage
            .get(path)
            .is_some_and(|sequences| sequences.contains(&revision.sequence))
        {
            Ok(())
        } else {
            Err("This revision does not belong to the selected path".into())
        }
    }
    pub fn paths(&self, after: Option<&str>, filter: &str, deleted_only: bool) -> HistoryPathPage {
        let mut paths = self
            .paths
            .keys()
            .filter(|p| after.is_none_or(|a| p.as_str() > a) && p.contains(filter))
            .filter_map(|p| {
                let r = self.latest(p)?;
                let deleted = r.content == HistoryContent::Absent;
                if deleted_only && !deleted {
                    return None;
                }
                Some(HistoryPath {
                    path: p.clone(),
                    deleted,
                    latest: r.id.clone(),
                    excluded: self.eligibility.path(Path::new(p), false).err(),
                })
            })
            .take(PAGE_SIZE + 1)
            .collect::<Vec<_>>();
        let next = if paths.len() > PAGE_SIZE {
            paths.truncate(PAGE_SIZE);
            paths.last().map(|p| p.path.clone())
        } else {
            None
        };
        HistoryPathPage {
            paths,
            next,
            generation: self.generation(),
        }
    }
    pub fn list(&self, path: &str, before: Option<u32>) -> Result<HistoryRevisionPage, String> {
        validate_relative(Path::new(path))?;
        let mut revisions = self
            .lineage
            .get(path)
            .into_iter()
            .flatten()
            .rev()
            .filter(|n| before.is_none_or(|b| **n < b))
            .filter_map(|n| self.records.get(n))
            .take(PAGE_SIZE + 1)
            .cloned()
            .collect::<Vec<_>>();
        let next = if revisions.len() > PAGE_SIZE {
            revisions.truncate(PAGE_SIZE);
            revisions.last().map(|r| r.sequence)
        } else {
            None
        };
        Ok(HistoryRevisionPage {
            revisions,
            next,
            generation: self.generation(),
            excluded: self.eligibility.path(Path::new(path), false).err(),
        })
    }
    pub fn bytes(&self, content: &HistoryContent) -> Result<Option<Vec<u8>>, String> {
        match content {
            HistoryContent::Absent => Ok(None),
            HistoryContent::Content { digest, bytes } => {
                if digest.len() != 64
                    || !digest.bytes().all(|b| b.is_ascii_hexdigit())
                    || *bytes as u64 > HISTORY_MAX_FILE_BYTES
                {
                    return Err("Invalid history blob reference".into());
                }
                let blob = self.dir.join("blobs").join(digest);
                if fs::metadata(&blob)
                    .map_err(|_| "Revision content is unavailable")?
                    .len()
                    != *bytes as u64
                {
                    return Err("Revision content is corrupt".into());
                }
                let mut data = Vec::new();
                File::open(blob)
                    .map_err(|_| "Revision content is unavailable")?
                    .take(HISTORY_MAX_FILE_BYTES + 1)
                    .read_to_end(&mut data)
                    .map_err(|e| e.to_string())?;
                if data.len() != *bytes as usize || blake3::hash(&data).to_hex().as_str() != digest
                {
                    return Err("Revision content is corrupt".into());
                }
                HistoryEligibility::validate_bytes(&data)?;
                Ok(Some(data))
            }
        }
    }
    pub fn content(bytes: Option<&[u8]>) -> HistoryContent {
        match bytes {
            None => HistoryContent::Absent,
            Some(data) => HistoryContent::Content {
                digest: blake3::hash(data).to_hex().to_string(),
                bytes: data.len() as u32,
            },
        }
    }
    pub fn capture(
        &mut self,
        generation: u32,
        path: &str,
        bytes: Option<&[u8]>,
        source: HistorySource,
    ) -> Result<LocalRevisionId, String> {
        let result = self.capture_record(generation, path, bytes, source, None);
        if result.is_ok() {
            if let Ok((_, relative)) = self.eligibility.root.resolve(Path::new(path)) {
                if let Some(safety) = self.pending_safety.remove(&relative) {
                    self.unpin(&safety);
                }
            }
        }
        result
    }
    fn capture_record(
        &mut self,
        generation: u32,
        path: &str,
        bytes: Option<&[u8]>,
        source: HistorySource,
        rename: Option<(String, String)>,
    ) -> Result<LocalRevisionId, String> {
        self.check_generation(generation)?;
        if !self.meta.policy.enabled {
            return Err("Local History capture is disabled".into());
        }
        let (_, path) = self.eligibility.path(Path::new(path), false)?;
        if let Some(bytes) = bytes {
            HistoryEligibility::validate_bytes(bytes)?;
        }
        self.cleanup(now_ms())?;
        let content = Self::content(bytes);
        if rename.is_none()
            && let Some(last) = self.latest(&path)
            && last.content == content
            && last.unavailable.is_none()
        {
            return Ok(last.id.clone());
        }
        let lifecycle = rename
            .as_ref()
            .map(|(_, lifecycle)| lifecycle.clone())
            .or_else(|| {
                self.latest(&path)
                    .filter(|r| {
                        content == HistoryContent::Absent || r.content != HistoryContent::Absent
                    })
                    .map(|r| r.lifecycle.clone())
            })
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let r = LocalRevision {
            id: LocalRevisionId(uuid::Uuid::new_v4().to_string()),
            sequence: self.meta.next_sequence,
            observed_ms: now_ms(),
            path,
            lifecycle,
            source,
            prior_path: rename.map(|(prior, _)| prior),
            content,
            unavailable: None,
        };
        self.admit(&r)?;
        if let HistoryContent::Content { digest, .. } = &r.content {
            let dest = self.dir.join("blobs").join(digest);
            if !dest.exists() {
                crate::persist::write_atomic(&dest, bytes.unwrap()).map_err(|e| e.to_string())?;
            }
            self.bytes(&r.content)?;
        }
        self.meta.next_sequence = r
            .sequence
            .checked_add(1)
            .ok_or("History sequence exhausted")?;
        json(&self.dir.join("root.json"), &self.meta)?;
        json(
            &self.record_path(&r),
            &Record {
                schema: SCHEMA,
                generation,
                revision: r.clone(),
            },
        )?;
        let id = r.id.clone();
        self.insert(r);
        self.rebuild_paths();
        self.revision = self.revision.wrapping_add(1);
        Ok(id)
    }
    /// Called only after a successful within-root move. The old path remains discoverable.
    pub fn renamed(
        &mut self,
        generation: u32,
        from: &str,
        to: &str,
        bytes: &[u8],
    ) -> Result<LocalRevisionId, String> {
        self.check_generation(generation)?;
        validate_relative(Path::new(from))?;
        validate_relative(Path::new(to))?;
        let lifecycle = self
            .latest(from)
            .map(|r| r.lifecycle.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.capture(generation, from, None, HistorySource::FileOperation)?;
        self.capture_record(
            generation,
            to,
            Some(bytes),
            HistorySource::FileOperation,
            Some((from.into(), lifecycle)),
        )
    }
}

impl HistoryStore {
    pub fn ensure_durable(&self, id: &LocalRevisionId) -> Result<(), String> {
        let revision = self.get(id)?;
        let mut bytes = Vec::new();
        File::open(self.record_path(revision))
            .map_err(|_| "The safety revision is not durable")?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("Invalid safety revision record".into());
        }
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|_| "The safety revision record is corrupt")?;
        if record.schema != SCHEMA
            || record.generation < self.meta.min_generation
            || record.generation > self.generation()
            || record.revision.id != *id
            || record.revision.content != revision.content
            || record.revision.path != revision.path
            || record.revision.lifecycle != revision.lifecycle
            || record.revision.sequence != revision.sequence
        {
            return Err("The safety revision record is unavailable".into());
        }
        self.bytes(&revision.content)?;
        Ok(())
    }

    /// `commit` runs the last check and mutation under the app's editor/window gates.
    pub fn restore_with(
        &mut self,
        token: &HistoryCompareToken,
        commit: impl FnOnce(&mut dyn FnMut() -> Result<(), String>) -> Result<(), String>,
    ) -> Result<HistoryRestoreResult, String> {
        self.validate_restore(token)?;
        self.ensure_durable(&token.target)?;
        let target = self.get(&token.target)?.content.clone();
        let desired = self.bytes(&target)?;
        self.pin(&token.target)?;
        let mut safety: Option<LocalRevisionId> = None;
        let mut mutated = false;
        let result = (|| {
            let (_, before) = self.eligibility.read(Path::new(&token.path))?;
            if Self::content(before.as_deref()) != token.current {
                return Err("The file changed; refresh the comparison".into());
            }
            let id = self.capture(
                self.generation(),
                &token.path,
                before.as_deref(),
                HistorySource::External,
            )?;
            self.ensure_durable(&id)?;
            self.pin(&id)?;
            safety = Some(id.clone());
            let mut durability_warning = None;
            let committed = commit(&mut || {
                if mutated {
                    return Err("The restore mutation already ran".into());
                }
                let physical = self.validate_restore(token)?;
                let written = match &desired {
                    Some(bytes) if physical.exists() => {
                        crate::document::write_bytes(&physical, bytes).map_err(|e| e.to_string())
                    }
                    Some(bytes) => crate::persist::write_atomic_with_mode(
                        &physical,
                        bytes,
                        crate::persist::SHARED_MODE,
                    )
                    .map_err(|e| e.to_string()),
                    None if physical.exists() => fs::remove_file(&physical)
                        .map_err(|e| e.to_string())
                        .and_then(|_| {
                            crate::persist::sync_dir(physical.parent().unwrap())
                                .map_err(|e| e.to_string())
                        }),
                    None => Ok(()),
                };
                if let Err(reason) = written {
                    // Atomic publication may have succeeded before directory fsync failed.
                    if self
                        .eligibility
                        .read(Path::new(&token.path))
                        .ok()
                        .is_some_and(|(_, actual)| actual == desired)
                    {
                        durability_warning = Some(format!(
                            "File restored, but durability could not be confirmed: {reason}"
                        ));
                    } else {
                        return Err(reason);
                    }
                }
                mutated = true;
                Ok(())
            });
            if let Err(reason) = committed {
                if !mutated {
                    return Err(reason);
                }
                durability_warning = Some(format!(
                    "File restored, but completing preparation failed: {reason}"
                ));
            }
            if !mutated {
                return Err("Restore preparation did not commit the mutation".into());
            }
            let post = self.capture(
                self.generation(),
                &token.path,
                desired.as_deref(),
                HistorySource::Restore,
            );
            let degraded = post.err().map(|reason| format!("File restored; the safety revision is retained, but recording the restored state failed: {reason}")).or(durability_warning);
            if let Some(reason) = &degraded {
                self.coverage.degraded = Some(reason.clone());
                self.pending_safety.insert(token.path.clone(), id.clone());
            }
            Ok(HistoryRestoreResult {
                restored: true,
                safety_revision: id,
                degraded,
            })
        })();
        self.unpin(&token.target);
        if let Some(id) = safety {
            if !mutated || result.as_ref().is_ok_and(|r| r.degraded.is_none()) {
                self.unpin(&id);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_fs::testing::scratch;
    #[test]
    fn restore_preserves_safety_bytes_and_can_reverse_recreation_with_absence() {
        let project = scratch("history-restore-bytes");
        let state = scratch("history-restore-bytes-state");
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let retained = s
            .capture(1, "a", Some(b"old\r\ntext\r\n"), HistorySource::Baseline)
            .unwrap();
        fs::write(project.join("a"), "current").unwrap();
        let token = s.compare("a", &retained, None).unwrap().token;
        let restored = s.restore_with(&token, |write| write()).unwrap();
        assert_eq!(fs::read(project.join("a")).unwrap(), b"old\r\ntext\r\n");
        assert_eq!(
            s.bytes(&s.get(&restored.safety_revision).unwrap().content)
                .unwrap()
                .unwrap(),
            b"current"
        );
        assert_eq!(s.latest("a").unwrap().source, HistorySource::Restore);
        fs::remove_file(project.join("a")).unwrap();
        let token = s.compare("a", &retained, None).unwrap().token;
        let recreated = s.restore_with(&token, |write| write()).unwrap();
        assert!(project.join("a").exists());
        let absence = s
            .compare("a", &recreated.safety_revision, None)
            .unwrap()
            .token;
        s.restore_with(&absence, |write| write()).unwrap();
        assert!(!project.join("a").exists());
        assert!(s.pins.is_empty());
    }

    #[test]
    fn failed_safety_capture_or_last_check_never_mutates_disk() {
        let project = scratch("history-restore-refused");
        let state = scratch("history-restore-refused-state");
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let retained = s
            .capture(1, "a", Some(b"old"), HistorySource::Baseline)
            .unwrap();
        fs::write(project.join("a"), "current").unwrap();
        let token = s.compare("a", &retained, None).unwrap().token;
        assert!(
            s.restore_with(&token, |write| {
                fs::write(project.join("a"), "raced").unwrap();
                write()
            })
            .is_err()
        );
        assert_eq!(fs::read_to_string(project.join("a")).unwrap(), "raced");
        assert!(s.pins.is_empty());
        let token = s.compare("a", &retained, None).unwrap().token;
        fs::rename(state.join("records"), state.join("records-held")).unwrap();
        fs::write(state.join("records"), "not a directory").unwrap();
        assert!(s.restore_with(&token, |write| write()).is_err());
        assert_eq!(fs::read_to_string(project.join("a")).unwrap(), "raced");
        assert!(s.pins.is_empty());
    }

    #[test]
    fn successful_restore_with_post_record_failure_reports_success_and_pins_safety() {
        let project = scratch("history-restore-post-failed");
        let state = scratch("history-restore-post-state");
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let retained = s
            .capture(1, "a", Some(b"old"), HistorySource::Baseline)
            .unwrap();
        fs::write(project.join("a"), "current").unwrap();
        let token = s.compare("a", &retained, None).unwrap().token;
        let restored = s
            .restore_with(&token, |write| {
                write()?;
                fs::rename(state.join("records"), state.join("records-held")).unwrap();
                fs::write(state.join("records"), "blocked").unwrap();
                Ok(())
            })
            .unwrap();
        assert!(restored.restored);
        assert!(restored.degraded.is_some());
        assert_eq!(fs::read_to_string(project.join("a")).unwrap(), "old");
        assert!(s.pins.contains_key(&restored.safety_revision.0));
        assert_eq!(
            s.bytes(&s.get(&restored.safety_revision).unwrap().content)
                .unwrap()
                .unwrap(),
            b"current"
        );
        fs::remove_file(state.join("records")).unwrap();
        fs::rename(state.join("records-held"), state.join("records")).unwrap();
        s.capture(s.generation(), "a", Some(b"old"), HistorySource::External)
            .unwrap();
        assert!(s.pins.is_empty());
    }
    #[test]
    fn shared_blobs_survive_pins_and_age_cleanup_reclaims_them() {
        let project = scratch("history-retention-root");
        let state = scratch("history-retention-state");
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let first = s
            .capture(1, "a", Some(b"shared"), HistorySource::Baseline)
            .unwrap();
        let second = s
            .capture(1, "b", Some(b"shared"), HistorySource::Baseline)
            .unwrap();
        assert_eq!(fs::read_dir(state.join("blobs")).unwrap().count(), 1);
        s.pin(&first).unwrap();
        s.cleanup(now_ms() + 8 * 86_400_000).unwrap();
        assert!(s.get(&first).is_ok());
        assert!(s.get(&second).is_err());
        assert_eq!(fs::read_dir(state.join("blobs")).unwrap().count(), 1);
        assert!(s.clear(true).is_err());
        s.unpin(&first);
        s.cleanup(now_ms() + 8 * 86_400_000).unwrap();
        assert_eq!(fs::read_dir(state.join("blobs")).unwrap().count(), 0);
    }

    #[test]
    fn policy_disable_and_clear_persist_without_reviving_old_jobs() {
        let project = scratch("history-clear-root");
        let state = scratch("history-clear-state");
        let root = RootIdentity::new(&project).unwrap();
        let mut s = HistoryStore::open(root.clone(), state.to_path_buf()).unwrap();
        let first = s
            .capture(1, "a", Some(b"before"), HistorySource::Baseline)
            .unwrap();
        let policy = HistoryPolicy {
            enabled: false,
            exclusions: vec!["*.secret".into()],
            ..HistoryPolicy::default()
        };
        s.set_policy(policy.clone()).unwrap();
        assert!(
            s.capture(1, "a", Some(b"queued"), HistorySource::External)
                .is_err()
        );
        assert!(
            s.capture(
                s.generation(),
                "a",
                Some(b"disabled"),
                HistorySource::External
            )
            .is_err()
        );
        drop(s);
        let mut s = HistoryStore::open(root, state.to_path_buf()).unwrap();
        assert_eq!(s.status().policy, policy);
        assert!(s.get(&first).is_ok());
        assert!(s.clear(false).is_err());
        let generation = s.generation();
        s.clear(true).unwrap();
        assert_eq!(s.status().policy, policy);
        assert!(s.get(&first).is_err());
        assert!(
            s.capture(generation, "a", Some(b"old job"), HistorySource::External)
                .is_err()
        );
        s.set_policy(HistoryPolicy {
            enabled: true,
            ..policy
        })
        .unwrap();
        s.capture(
            s.generation(),
            "a",
            Some(b"new baseline"),
            HistorySource::Baseline,
        )
        .unwrap();
        assert_eq!(s.list("a", None).unwrap().revisions.len(), 1);
    }

    #[test]
    fn small_budget_evicts_oldest_unpinned_and_refuses_unadmittable_content() {
        let project = scratch("history-budget-root");
        let state = scratch("history-budget-state");
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let first = s
            .capture(1, "a", Some(&vec![b'a'; 2000]), HistorySource::Baseline)
            .unwrap();
        s.pin(&first).unwrap();
        let budget = s.used_bytes() as u32 + 700;
        s.set_policy(HistoryPolicy {
            budget_bytes: budget,
            ..HistoryPolicy::default()
        })
        .unwrap();
        assert!(
            s.capture(
                s.generation(),
                "b",
                Some(&vec![b'b'; 2000]),
                HistorySource::External
            )
            .is_err()
        );
        assert!(s.get(&first).is_ok());
        s.unpin(&first);
        let newest = s
            .capture(
                s.generation(),
                "b",
                Some(&vec![b'b'; 2000]),
                HistorySource::External,
            )
            .unwrap();
        assert!(s.get(&first).is_err());
        assert!(s.get(&newest).is_ok());
        assert!(s.used_bytes() <= budget as u64);
        s.set_policy(HistoryPolicy {
            budget_bytes: 1,
            ..HistoryPolicy::default()
        })
        .unwrap();
        assert!(
            s.capture(s.generation(), "c", Some(b"x"), HistorySource::External)
                .is_err()
        );
        assert!(s.status().coverage.degraded.is_some());
    }

    #[test]
    fn clear_generation_commit_survives_interrupted_removal() {
        let project = scratch("history-clear-crash-root");
        let state = scratch("history-clear-crash-state");
        let root = RootIdentity::new(&project).unwrap();
        let mut s = HistoryStore::open(root.clone(), state.to_path_buf()).unwrap();
        let id = s
            .capture(1, "a", Some(b"old"), HistorySource::Baseline)
            .unwrap();
        let record = s.record_path(s.get(&id).unwrap());
        let saved = fs::read(&record).unwrap();
        s.clear(true).unwrap();
        fs::write(record, saved).unwrap();
        drop(s);
        let s = HistoryStore::open(root, state.to_path_buf()).unwrap();
        assert!(s.paths(None, "", false).paths.is_empty());
        assert!(s.get(&id).is_err());
    }
    #[test]
    fn consecutive_dedup_preserves_source_and_recreation_starts_a_lifecycle() {
        let project = scratch("history-dedup-root");
        let state = scratch("history-dedup-state");
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let first = s
            .capture(1, "a", Some(b"A"), HistorySource::CideSave)
            .unwrap();
        assert_eq!(
            s.capture(1, "a", Some(b"A"), HistorySource::External)
                .unwrap(),
            first
        );
        assert_eq!(s.get(&first).unwrap().source, HistorySource::CideSave);
        s.capture(1, "a", Some(b"B"), HistorySource::External)
            .unwrap();
        let again = s
            .capture(1, "a", Some(b"A"), HistorySource::External)
            .unwrap();
        assert_ne!(first, again);
        assert_eq!(
            s.get(&first).unwrap().lifecycle,
            s.get(&again).unwrap().lifecycle
        );
        s.capture(1, "a", None, HistorySource::External).unwrap();
        let recreated = s
            .capture(1, "a", Some(b"A"), HistorySource::CideSave)
            .unwrap();
        assert_ne!(
            s.get(&first).unwrap().lifecycle,
            s.get(&recreated).unwrap().lifecycle
        );
        assert_eq!(s.list("a", None).unwrap().revisions.len(), 5);
    }

    #[test]
    fn explicit_rename_keeps_old_revisions_accessible_at_new_path() {
        let project = scratch("history-rename-root");
        let state = scratch("history-rename-state");
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let old = s
            .capture(1, "old", Some(b"old content"), HistorySource::Baseline)
            .unwrap();
        let new = s.renamed(1, "old", "new", b"old content").unwrap();
        assert_eq!(
            s.get(&old).unwrap().lifecycle,
            s.get(&new).unwrap().lifecycle
        );
        assert_eq!(s.get(&new).unwrap().prior_path.as_deref(), Some("old"));
        assert!(
            s.list("new", None)
                .unwrap()
                .revisions
                .iter()
                .any(|r| r.id == old)
        );
        assert_eq!(s.paths(None, "", true).paths[0].path, "old");
    }

    #[test]
    fn independent_records_recover_around_corruption_and_interrupted_commits() {
        let project = scratch("history-recovery-root");
        let state = scratch("history-recovery-state");
        let root = RootIdentity::new(&project).unwrap();
        let mut s = HistoryStore::open(root.clone(), state.to_path_buf()).unwrap();
        let good = s
            .capture(1, "good", Some(b"good"), HistorySource::Baseline)
            .unwrap();
        let missing = s
            .capture(1, "missing", Some(b"missing"), HistorySource::Baseline)
            .unwrap();
        let HistoryContent::Content { digest, .. } = &s.get(&missing).unwrap().content else {
            panic!()
        };
        fs::remove_file(state.join("blobs").join(digest)).unwrap();
        fs::write(state.join("records/corrupt.json"), "{truncated").unwrap();
        fs::write(state.join("records/.interrupted.tmp"), "partial").unwrap();
        fs::write(state.join("blobs/orphan"), "blob committed before record").unwrap();
        drop(s);
        let s = HistoryStore::open(root, state.to_path_buf()).unwrap();
        assert_eq!(
            s.bytes(&s.get(&good).unwrap().content).unwrap().unwrap(),
            b"good"
        );
        assert!(s.get(&missing).unwrap().unavailable.is_some());
        assert!(s.bytes(&s.get(&missing).unwrap().content).is_err());
        assert!(s.coverage.degraded.is_some());
        assert!(!state.join("blobs/orphan").exists());
        assert!(!state.join("records/.interrupted.tmp").exists());
    }

    #[test]
    fn unsupported_schema_and_root_mismatch_are_unavailable() {
        let project = scratch("history-schema-root");
        let other = scratch("history-schema-other");
        let state = scratch("history-schema-state");
        let root = RootIdentity::new(&project).unwrap();
        drop(HistoryStore::open(root.clone(), state.to_path_buf()).unwrap());
        assert!(
            HistoryStore::open(RootIdentity::new(&other).unwrap(), state.to_path_buf()).is_err()
        );
        let mut meta: Metadata =
            serde_json::from_slice(&fs::read(state.join("root.json")).unwrap()).unwrap();
        meta.schema = 99;
        json(&state.join("root.json"), &meta).unwrap();
        assert!(HistoryStore::open(root, state.to_path_buf()).is_err());
    }
    #[test]
    fn restart_preserves_bytes_root_and_revision() {
        let project = scratch("history-store-root");
        let state = scratch("history-store-state");
        let root = RootIdentity::new(&project).unwrap();
        let mut store = HistoryStore::open_background(root.clone(), state.to_path_buf())
            .join()
            .unwrap()
            .unwrap();
        let id = store
            .capture(
                1,
                "file",
                Some(b"first\r\nsecond\r\n"),
                HistorySource::CideSave,
            )
            .unwrap();
        assert!(HistoryStore::open(root.clone(), state.to_path_buf()).is_err());
        drop(store);
        let store = HistoryStore::open(root, state.to_path_buf()).unwrap();
        assert_eq!(
            store
                .bytes(&store.get(&id).unwrap().content)
                .unwrap()
                .unwrap(),
            b"first\r\nsecond\r\n"
        );
        assert_eq!(store.list("file", None).unwrap().revisions.len(), 1);
        assert_eq!(store.paths(None, "", false).paths[0].path, "file");
        assert!(store.get(&LocalRevisionId("../escape".into())).is_err());
    }
    #[test]
    fn pages_are_bounded_and_cursors_do_not_repeat_rows() {
        let project = scratch("history-pages-root");
        let state = scratch("history-pages-state");
        let mut store =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        for n in 0..105 {
            store
                .capture(
                    1,
                    "file",
                    Some(n.to_string().as_bytes()),
                    HistorySource::External,
                )
                .unwrap();
        }
        let page = store.list("file", None).unwrap();
        assert_eq!(page.revisions.len(), 100);
        let rest = store.list("file", page.next).unwrap();
        assert_eq!(rest.revisions.len(), 5);
        assert!(rest.next.is_none());
        assert!(page.revisions.last().unwrap().sequence > rest.revisions[0].sequence);
    }
}
