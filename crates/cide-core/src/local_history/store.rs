use super::{HistoryEligibility, RootIdentity, validate_relative};
use cide_ipc::local_history::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA: u32 = 1;
const PAGE_SIZE: usize = 100;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    schema: u32,
    canonical_root: Vec<u8>,
    generation: u32,
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
    pub coverage: HistoryCoverage,
    revision: u32,
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
        let meta: Metadata = if metadata.exists() {
            serde_json::from_slice(&fs::read(&metadata).map_err(|e| e.to_string())?)
                .map_err(|e| format!("Unavailable history metadata: {e}"))?
        } else {
            let meta = Metadata {
                schema: SCHEMA,
                canonical_root: root.path.as_os_str().as_encoded_bytes().to_vec(),
                generation: 1,
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
            coverage: HistoryCoverage::default(),
            revision: 0,
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
        for entry in fs::read_dir(self.dir.join("records")).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let record: Record =
                serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("Unreadable history record: {e}"))?;
            if record.schema != SCHEMA || record.generation != self.meta.generation {
                continue;
            }
            let r = record.revision;
            validate_relative(Path::new(&r.path))?;
            uuid::Uuid::parse_str(&r.id.0).map_err(|_| "Invalid history revision ID")?;
            if self.record_path(&r) != path {
                return Err("History record identity does not match its filename".into());
            }
            self.meta.next_sequence = self.meta.next_sequence.max(
                r.sequence
                    .checked_add(1)
                    .ok_or("History sequence exhausted")?,
            );
            self.insert(r);
        }
        self.rebuild_paths();
        Ok(())
    }
    fn insert(&mut self, r: LocalRevision) {
        self.ids.insert(r.id.0.clone(), r.sequence);
        self.records.insert(r.sequence, r);
    }
    fn rebuild_paths(&mut self) {
        self.paths.clear();
        for r in self.records.values() {
            self.paths
                .entry(r.path.clone())
                .or_default()
                .push(r.sequence);
        }
    }
    pub fn generation(&self) -> u32 {
        self.meta.generation
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
        let mut digests = HashSet::new();
        let mut bytes = 0;
        for r in self.records.values() {
            bytes += fs::metadata(self.record_path(r))
                .map(|m| m.len())
                .unwrap_or(0);
            if let HistoryContent::Content { digest, .. } = &r.content {
                if digests.insert(digest) {
                    bytes += fs::metadata(self.dir.join("blobs").join(digest))
                        .map(|m| m.len())
                        .unwrap_or(0);
                }
            }
        }
        bytes
            + fs::metadata(self.dir.join("root.json"))
                .map(|m| m.len())
                .unwrap_or(0)
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
            .paths
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
                let data = fs::read(self.dir.join("blobs").join(digest))
                    .map_err(|_| "Revision content is unavailable".to_owned())?;
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
        self.check_generation(generation)?;
        if !self.meta.policy.enabled {
            return Err("Local History capture is disabled".into());
        }
        let (_, path) = self.eligibility.path(Path::new(path), false)?;
        if let Some(bytes) = bytes {
            HistoryEligibility::validate_bytes(bytes)?;
        }
        let content = Self::content(bytes);
        if let HistoryContent::Content { digest, .. } = &content {
            let dest = self.dir.join("blobs").join(digest);
            if !dest.exists() {
                crate::persist::write_atomic(&dest, bytes.unwrap()).map_err(|e| e.to_string())?;
            }
        }
        let r = LocalRevision {
            id: LocalRevisionId(uuid::Uuid::new_v4().to_string()),
            sequence: self.meta.next_sequence,
            observed_ms: now_ms(),
            path,
            lifecycle: uuid::Uuid::new_v4().to_string(),
            source,
            prior_path: None,
            content,
            unavailable: None,
        };
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_fs::testing::scratch;
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
