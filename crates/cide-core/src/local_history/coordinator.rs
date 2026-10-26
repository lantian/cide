//! Bounded observations plus a cancellable incremental baseline. Direct mutations hold the
//! same authority as observation, so an echo cannot interleave a save's pre/post snapshots.
use super::{HistoryStore, RootIdentity};
use cide_ipc::local_history::*;
use parking_lot::{Condvar, Mutex, RwLock};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};

const MAX_BATCHES: usize = 256;
const MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;
const MAX_BATCH_PATHS: usize = 4096;
type Changed = dyn Fn(HistoryStatus, Vec<String>) + Send + Sync;

struct Observation {
    generation: u32,
    paths: Vec<PathBuf>,
    bytes: usize,
}
struct State {
    loading: bool,
    store: Option<HistoryStore>,
}
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    status: RwLock<HistoryStatus>,
    generation: AtomicU32,
    stop: AtomicBool,
    reconcile: AtomicBool,
    queued_bytes: AtomicUsize,
    nested: RwLock<Vec<PathBuf>>,
    watch_dirs: RwLock<HashSet<PathBuf>>,
    watched_dirs: RwLock<HashSet<PathBuf>>,
    observer_attached: AtomicBool,
    membership_changed: AtomicBool,
    changed: Box<Changed>,
}

pub struct HistoryCoordinator {
    shared: Arc<Shared>,
    tx: SyncSender<Observation>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

pub struct HistoryTreeChange {
    pub from: PathBuf,
    pub to: PathBuf,
    pub moved: bool,
    pub created: bool,
}

fn tree_files(store: &mut HistoryStore, selections: &[PathBuf]) -> Vec<PathBuf> {
    let mut stack = selections.to_vec();
    let mut directories = HashSet::new();
    let mut files = HashSet::new();
    while let Some(path) = stack.pop() {
        let directory = path.is_dir();
        let Ok((physical, _)) = store.eligibility.path(&path, directory) else {
            continue;
        };
        if directory {
            if !directories.insert(physical.clone()) {
                continue;
            }
            match fs::read_dir(&physical) {
                Ok(entries) => stack.extend(entries.filter_map(Result::ok).map(|e| e.path())),
                Err(reason) => store.coverage.degraded = Some(reason.to_string()),
            }
        } else if physical.is_file() {
            files.insert(physical);
        }
    }
    files.into_iter().collect()
}

fn publish(shared: &Shared, store: &HistoryStore, paths: Vec<String>) {
    let mut status = store.status();
    let previous = shared.status.read().clone();
    if previous.generation != status.generation {
        shared.watched_dirs.write().clear();
    }
    if previous.generation == status.generation {
        for notice in previous.coverage.notices {
            if !status.coverage.notices.contains(&notice) {
                status.coverage.notices.push(notice);
            }
        }
        status.coverage.degraded = status.coverage.degraded.or(previous.coverage.degraded);
    }
    shared
        .generation
        .store(status.generation, Ordering::Release);
    *shared.status.write() = status.clone();
    (shared.changed)(status, paths);
}

fn observe(store: &mut HistoryStore, path: &Path, source: HistorySource, allow_new_absence: bool) {
    if !store.eligibility.policy.enabled || store.eligibility.path(path, false).is_err() {
        return;
    }
    match store.eligibility.read(path) {
        Ok((relative, bytes)) => {
            if bytes.is_none() && !allow_new_absence && store.latest(&relative).is_none() {
                return;
            }
            if let Err(reason) =
                store.capture(store.generation(), &relative, bytes.as_deref(), source)
            {
                store.coverage.degraded = Some(reason);
            }
        }
        Err(reason) if reason.starts_with("History supports") => {}
        Err(reason) => {
            store.coverage.degraded =
                Some(format!("Could not observe {}: {reason}", path.display()))
        }
    }
}

struct Scan {
    generation: u32,
    readers: Vec<(PathBuf, fs::ReadDir)>,
    dirs: HashSet<PathBuf>,
    retained: std::vec::IntoIter<String>,
    source: HistorySource,
}
impl Scan {
    fn start(store: &mut HistoryStore, source: HistorySource) -> Result<Self, String> {
        let retained = store.known_paths();
        store.coverage.baseline_running = true;
        store.coverage.scanned_files = 0;
        store.coverage.captured_files = 0;
        let boundary = "The baseline starts with each file's earliest read; external writes may coalesce. Unsaved edits are not recorded.";
        if !store.coverage.notices.iter().any(|n| n == boundary) {
            store.coverage.notices.push(boundary.into());
        }
        if source == HistorySource::Reopen {
            let notice = "Changes while this root was closed are represented by its next observation, without intermediate offline versions.";
            if !store.coverage.notices.iter().any(|n| n == notice) {
                store.coverage.notices.push(notice.into());
            }
        }
        let root = store.eligibility.root.path.clone();
        let reader = fs::read_dir(&root).map_err(|e| e.to_string())?;
        Ok(Self {
            generation: store.generation(),
            readers: vec![(root.clone(), reader)],
            dirs: HashSet::from([root]),
            retained: retained.into_iter(),
            source,
        })
    }

    /// One bounded batch; interactive calls can acquire the authority between files.
    fn step(&mut self, store: &mut HistoryStore, nested: &[PathBuf], shared: &Shared) -> bool {
        if self.generation != store.generation() || !store.eligibility.policy.enabled {
            return false;
        }
        for _ in 0..16 {
            if shared.observer_attached.load(Ordering::Acquire)
                && self
                    .readers
                    .last()
                    .is_some_and(|(dir, _)| !shared.watched_dirs.read().contains(dir))
            {
                return true;
            }
            let next = self
                .readers
                .last_mut()
                .and_then(|(_, reader)| reader.next());
            match next {
                Some(Ok(entry)) => {
                    let path = entry.path();
                    if nested.iter().any(|r| path.starts_with(r)) {
                        continue;
                    }
                    let is_dir = path.is_dir();
                    let Ok((physical, _)) = store.eligibility.path(&path, is_dir) else {
                        continue;
                    };
                    if nested.iter().any(|r| physical.starts_with(r)) {
                        continue;
                    }
                    if is_dir {
                        if self.dirs.insert(physical.clone()) {
                            shared.watch_dirs.write().insert(physical.clone());
                            match fs::read_dir(&physical) {
                                Ok(reader) => self.readers.push((physical, reader)),
                                Err(e) => store.coverage.degraded = Some(e.to_string()),
                            }
                        }
                    } else {
                        store.coverage.scanned_files =
                            store.coverage.scanned_files.saturating_add(1);
                        observe(store, &path, self.source, false);
                        store.coverage.captured_files = store.known_path_count();
                    }
                }
                Some(Err(e)) => store.coverage.degraded = Some(e.to_string()),
                None if !self.readers.is_empty() => {
                    self.readers.pop();
                }
                None => {
                    if let Some(path) = self.retained.next() {
                        observe(store, Path::new(&path), self.source, false);
                    } else {
                        store.coverage.baseline_running = false;
                        return false;
                    }
                }
            }
        }
        true
    }
}

impl HistoryCoordinator {
    /// Coordinators are acquired in stable root order by the app for cross-root operations.
    pub fn mutate_tree<T, E>(
        &self,
        paths: &[PathBuf],
        operation: impl FnOnce() -> Result<T, E>,
        summarize: &impl Fn(&Result<T, E>) -> Vec<HistoryTreeChange>,
    ) -> Result<T, E> {
        let mut state = self.shared.state.lock();
        while state.loading {
            self.shared.ready.wait(&mut state);
        }
        if self.shared.stop.load(Ordering::Acquire) || state.store.is_none() {
            return operation();
        }
        let store = state.store.as_mut().unwrap();
        let before = tree_files(store, paths);
        let regions: std::collections::HashMap<_, _> = paths
            .iter()
            .filter_map(|p| {
                if p.is_symlink() {
                    return None;
                }
                store
                    .eligibility
                    .root
                    .resolve(p)
                    .ok()
                    .map(|(physical, _)| (p.clone(), physical))
            })
            .collect();
        for path in &before {
            if !self.is_nested(path) {
                observe(store, path, HistorySource::External, false);
            }
        }
        let result = operation();
        let changes = summarize(&result);
        let mut after_selections = paths.to_vec();
        after_selections.extend(changes.iter().map(|c| c.to.clone()));
        let after = tree_files(store, &after_selections);
        for change in &changes {
            let expected_dest = change
                .to
                .parent()
                .and_then(|p| fs::canonicalize(p).ok())
                .and_then(|p| change.to.file_name().map(|n| p.join(n)));
            if change.created
                && let Some(expected) = &expected_dest
            {
                for path in after
                    .iter()
                    .filter(|p| p.starts_with(expected) && !before.contains(p))
                {
                    if store
                        .eligibility
                        .read(path)
                        .ok()
                        .is_none_or(|(_, bytes)| bytes.is_none())
                    {
                        continue;
                    }
                    if !self.is_nested(path)
                        && let Ok((_, relative)) = store.eligibility.root.resolve(path)
                    {
                        if let Err(reason) = store.capture(
                            store.generation(),
                            &relative,
                            None,
                            HistorySource::FileOperation,
                        ) {
                            store.coverage.degraded = Some(reason);
                        }
                    }
                }
            }
            if change.moved
                && !change.to.is_symlink()
                && let Some(source) = regions.get(&change.from)
            {
                for old in before.iter().filter(|p| p.starts_with(source)) {
                    let suffix = old.strip_prefix(source).unwrap();
                    let new = if suffix.as_os_str().is_empty() {
                        change.to.clone()
                    } else {
                        change.to.join(suffix)
                    };
                    if self.is_nested(old) {
                        continue;
                    }
                    if let (Ok((_, old_relative)), Ok((new_relative, Some(bytes)))) = (
                        store.eligibility.root.resolve(old),
                        store.eligibility.read(&new),
                    ) {
                        if !self.is_nested(&new)
                            && let Err(reason) = store.renamed(
                                store.generation(),
                                &old_relative,
                                &new_relative,
                                &bytes,
                            )
                        {
                            store.coverage.degraded = Some(reason);
                        }
                    }
                }
            }
        }
        let post_source = if result.is_ok() {
            HistorySource::FileOperation
        } else {
            HistorySource::External
        };
        for path in before.iter().chain(after.iter()) {
            if !self.is_nested(path) {
                observe(store, path, post_source, false);
            }
        }
        publish(&self.shared, store, Vec::new());
        result
    }
    pub fn start(
        root: RootIdentity,
        dir: PathBuf,
        nested: Vec<PathBuf>,
        changed: impl Fn(HistoryStatus, Vec<String>) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::start_inner(root, dir, nested, false, changed)
    }
    pub fn start_observed(
        root: RootIdentity,
        dir: PathBuf,
        nested: Vec<PathBuf>,
        changed: impl Fn(HistoryStatus, Vec<String>) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::start_inner(root, dir, nested, true, changed)
    }
    fn start_inner(
        root: RootIdentity,
        dir: PathBuf,
        nested: Vec<PathBuf>,
        observed: bool,
        changed: impl Fn(HistoryStatus, Vec<String>) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let status = HistoryStatus {
            root: root.id.clone(),
            canonical_root: root.path.to_string_lossy().into_owned(),
            generation: 0,
            revision: 0,
            used_bytes: 0,
            policy: HistoryPolicy::default(),
            coverage: HistoryCoverage {
                baseline_running: true,
                ..Default::default()
            },
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                loading: true,
                store: None,
            }),
            ready: Condvar::new(),
            status: RwLock::new(status),
            generation: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            reconcile: AtomicBool::new(true),
            queued_bytes: AtomicUsize::new(0),
            nested: RwLock::new(nested),
            watch_dirs: RwLock::new(HashSet::from([root.path.clone()])),
            watched_dirs: RwLock::new(HashSet::new()),
            observer_attached: AtomicBool::new(observed),
            membership_changed: AtomicBool::new(false),
            changed: Box::new(changed),
        });
        let (tx, rx) = mpsc::sync_channel::<Observation>(MAX_BATCHES);
        let worker_shared = shared.clone();
        let worker = thread::Builder::new()
            .name("cide-local-history".into())
            .spawn(move || {
                let shared = worker_shared;
                {
                    let opened = HistoryStore::open(root, dir);
                    let mut state = shared.state.lock();
                    match opened {
                        Ok(mut store) => {
                            store.coverage.baseline_running = store.eligibility.policy.enabled;
                            publish(&shared, &store, Vec::new());
                            state.store = Some(store);
                        }
                        Err(reason) => {
                            let mut status = shared.status.write();
                            status.coverage.baseline_running = false;
                            status.coverage.degraded = Some(reason);
                            (shared.changed)(status.clone(), Vec::new());
                        }
                    }
                    state.loading = false;
                    shared.ready.notify_all();
                }
                let startup_generation = shared.generation.load(Ordering::Acquire);
                let mut scan: Option<Scan> = None;
                let mut ticks = 0u32;
                let mut first_scan = true;
                while !shared.stop.load(Ordering::Acquire) {
                    if shared.membership_changed.swap(false, Ordering::AcqRel) {
                        if let Some(store) = &mut shared.state.lock().store {
                            if let Err(reason) = store.restart_observation() {
                                store.coverage.degraded = Some(reason);
                            }
                            publish(&shared, store, Vec::new());
                        }
                    }
                    let observation = if scan.is_some() {
                        rx.try_recv().ok()
                    } else {
                        rx.recv_timeout(Duration::from_millis(30)).ok()
                    };
                    if let Some(job) = observation {
                        shared.queued_bytes.fetch_sub(job.bytes, Ordering::AcqRel);
                        let mut changed_paths = Vec::new();
                        for path in &job.paths {
                            if shared.stop.load(Ordering::Acquire) {
                                break;
                            }
                            let mut state = shared.state.lock();
                            if let Some(store) = &mut state.store {
                                if job.generation != store.generation()
                                    && !(job.generation == 0
                                        && store.generation() == startup_generation)
                                {
                                    break;
                                }
                                if let Ok((physical, relative)) =
                                    store.eligibility.root.resolve(path)
                                {
                                    if shared.nested.read().iter().any(|r| physical.starts_with(r))
                                    {
                                        continue;
                                    }
                                    observe(store, path, HistorySource::External, false);
                                    changed_paths.push(relative);
                                }
                            }
                        }
                        if let Some(store) = &shared.state.lock().store {
                            publish(&shared, store, changed_paths);
                        }
                    }
                    let mut state = shared.state.lock();
                    let Some(store) = &mut state.store else {
                        continue;
                    };
                    if shared.reconcile.swap(false, Ordering::AcqRel)
                        || scan
                            .as_ref()
                            .is_some_and(|s| s.generation != store.generation())
                    {
                        scan = if store.eligibility.policy.enabled {
                            match Scan::start(
                                store,
                                if first_scan {
                                    if store.known_path_count() == 0 {
                                        HistorySource::Baseline
                                    } else {
                                        HistorySource::Reopen
                                    }
                                } else {
                                    HistorySource::External
                                },
                            ) {
                                Ok(scan) => Some(scan),
                                Err(reason) => {
                                    store.coverage.degraded = Some(reason);
                                    None
                                }
                            }
                        } else {
                            store.coverage.baseline_running = false;
                            None
                        };
                    }
                    first_scan = false;
                    if let Some(active) = &mut scan {
                        if !active.step(store, &shared.nested.read(), &shared) {
                            scan = None;
                            publish(&shared, store, Vec::new());
                        }
                    }
                    ticks = ticks.wrapping_add(1);
                    if ticks % 32 == 0 {
                        publish(&shared, store, Vec::new());
                    }
                    if ticks % 2000 == 0
                        && let Err(reason) = store.cleanup(super::store::now_ms())
                    {
                        store.coverage.degraded = Some(reason);
                    }
                    drop(state);
                    if scan.is_some() {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                // In-flight direct calls hold this lock; wait for them before releasing ownership.
                shared.state.lock().store.take();
            })
            .expect("could not start history worker");
        Arc::new(Self {
            shared,
            tx,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn attach_observer(&self) {
        self.shared.observer_attached.store(true, Ordering::Release);
        self.shared.reconcile.store(true, Ordering::Release);
    }
    pub fn confirm_watches(&self, dirs: &[PathBuf]) {
        self.shared
            .watched_dirs
            .write()
            .extend(dirs.iter().cloned());
    }
    pub fn is_nested(&self, path: &Path) -> bool {
        self.shared
            .nested
            .read()
            .iter()
            .any(|r| path.starts_with(r))
    }
    pub fn watch_directories(&self) -> Vec<PathBuf> {
        self.shared.watch_dirs.read().iter().cloned().collect()
    }
    pub fn status(&self) -> HistoryStatus {
        self.shared.status.read().clone()
    }

    pub fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut HistoryStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.shared.state.lock();
        while state.loading {
            self.shared.ready.wait(&mut state);
        }
        if self.shared.stop.load(Ordering::Acquire) {
            return Err("This history root is closed".into());
        }
        let store = state.store.as_mut().ok_or_else(|| {
            self.status()
                .coverage
                .degraded
                .unwrap_or_else(|| "History is unavailable".into())
        })?;
        let generation = store.generation();
        let result = operation(store);
        if generation != store.generation() {
            self.shared.reconcile.store(true, Ordering::Release);
        }
        publish(&self.shared, store, Vec::new());
        result
    }

    pub fn observe(&self, paths: Vec<PathBuf>, truncated: bool) {
        if self.shared.stop.load(Ordering::Acquire) {
            return;
        }
        if paths
            .iter()
            .any(|p| p.is_dir() || self.shared.watch_dirs.read().contains(p))
        {
            self.shared.reconcile.store(true, Ordering::Release);
        }
        let bytes = paths.iter().map(|p| p.as_os_str().len()).sum::<usize>();
        if truncated || paths.len() > MAX_BATCH_PATHS || bytes > MAX_QUEUED_BYTES {
            self.gap("Filesystem observation was truncated; reconciling the root");
            return;
        }
        let previous = self.shared.queued_bytes.fetch_add(bytes, Ordering::AcqRel);
        if previous + bytes > MAX_QUEUED_BYTES {
            self.shared.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.gap("History queue byte limit reached; reconciling the root");
            return;
        }
        let job = Observation {
            generation: self.shared.generation.load(Ordering::Acquire),
            paths,
            bytes,
        };
        if self.tx.try_send(job).is_err() {
            self.shared.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.gap("History observation queue overflowed; reconciling the root");
        }
    }

    pub fn gap(&self, reason: &str) {
        self.shared.reconcile.store(true, Ordering::Release);
        // Watcher callbacks never wait on capture IO.
        let mut status = self.shared.status.write();
        status.coverage.degraded = Some(reason.into());
        if !status.coverage.notices.iter().any(|n| n == reason) {
            status.coverage.notices.push(reason.into());
        }
        (self.shared.changed)(status.clone(), Vec::new());
    }

    pub fn set_nested(&self, nested: Vec<PathBuf>) {
        if *self.shared.nested.read() != nested {
            *self.shared.nested.write() = nested;
            self.shared
                .membership_changed
                .store(true, Ordering::Release);
            self.shared.reconcile.store(true, Ordering::Release);
        }
    }

    /// Must be called on a blocking command thread. History failures do not change its result.
    pub fn mutate<T, E>(
        &self,
        path: &Path,
        source: HistorySource,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let mut state = self.shared.state.lock();
        while state.loading {
            self.shared.ready.wait(&mut state);
        }
        if self.shared.stop.load(Ordering::Acquire) || state.store.is_none() {
            return operation();
        }
        let store = state.store.as_mut().unwrap();
        observe(store, path, HistorySource::External, true);
        let result = operation();
        if result.is_ok() {
            observe(store, path, source, true);
        }
        publish(
            &self.shared,
            store,
            store
                .eligibility
                .root
                .resolve(path)
                .ok()
                .map(|(_, p)| vec![p])
                .unwrap_or_default(),
        );
        result
    }

    pub fn close(&self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().take() {
            let _ = worker.join();
        }
    }
}
impl Drop for HistoryCoordinator {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_fs::testing::scratch;
    use std::time::Instant;
    #[test]
    fn directory_moves_deletes_and_overwrites_keep_recoverable_descendants() {
        let project = scratch("history-tree-mutations");
        let state = scratch("history-tree-mutations-state");
        fs::create_dir(project.join("old")).unwrap();
        fs::write(project.join("old/child"), "original").unwrap();
        fs::write(project.join("destination"), "overwritten").unwrap();
        let c = HistoryCoordinator::start(
            RootIdentity::new(&project).unwrap(),
            state.to_path_buf(),
            vec![],
            |_, _| {},
        );
        wait_done(&c);
        let from = project.join("old");
        let to = project.join("new");
        c.mutate_tree(
            &[from.clone(), to.clone()],
            || fs::rename(&from, &to),
            &|result| {
                if result.is_ok() {
                    vec![HistoryTreeChange {
                        from: from.clone(),
                        to: to.clone(),
                        moved: true,
                        created: true,
                    }]
                } else {
                    vec![]
                }
            },
        )
        .unwrap();
        c.with_store(|s| {
            assert!(
                s.list("new/child", None)?
                    .revisions
                    .iter()
                    .any(|r| r.path == "old/child")
            );
            assert_eq!(
                s.latest("old/child").unwrap().content,
                HistoryContent::Absent
            );
            Ok(())
        })
        .unwrap();
        let dest = project.join("destination");
        c.mutate_tree(
            &[to.clone(), dest.clone()],
            || fs::copy(to.join("child"), &dest),
            &|_| vec![],
        )
        .unwrap();
        c.with_store(|s| {
            assert_eq!(s.list("destination", None)?.revisions.len(), 2);
            Ok(())
        })
        .unwrap();
        c.mutate_tree(
            std::slice::from_ref(&to),
            || fs::remove_dir_all(&to),
            &|_| vec![],
        )
        .unwrap();
        c.with_store(|s| {
            assert_eq!(
                s.latest("new/child").unwrap().content,
                HistoryContent::Absent
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn cross_root_move_has_independent_deletion_and_creation() {
        let a = scratch("history-cross-a");
        let b = scratch("history-cross-b");
        let sa = scratch("history-cross-sa");
        let sb = scratch("history-cross-sb");
        let from = a.join("a");
        let to = b.join("b");
        fs::write(&from, "text").unwrap();
        let ca = HistoryCoordinator::start(
            RootIdentity::new(&a).unwrap(),
            sa.to_path_buf(),
            vec![],
            |_, _| {},
        );
        let cb = HistoryCoordinator::start(
            RootIdentity::new(&b).unwrap(),
            sb.to_path_buf(),
            vec![],
            |_, _| {},
        );
        wait_done(&ca);
        wait_done(&cb);
        let paths = [from.clone(), to.clone()];
        let summary = |result: &std::io::Result<()>| {
            if result.is_ok() {
                vec![HistoryTreeChange {
                    from: from.clone(),
                    to: to.clone(),
                    moved: true,
                    created: true,
                }]
            } else {
                vec![]
            }
        };
        ca.mutate_tree(
            &paths,
            || cb.mutate_tree(&paths, || fs::rename(&from, &to), &summary),
            &summary,
        )
        .unwrap();
        let old = ca
            .with_store(|s| {
                assert_eq!(s.latest("a").unwrap().content, HistoryContent::Absent);
                Ok(s.latest("a").unwrap().lifecycle.clone())
            })
            .unwrap();
        cb.with_store(|s| {
            assert_ne!(s.latest("b").unwrap().lifecycle, old);
            assert_eq!(s.latest("b").unwrap().prior_path, None);
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn refused_save_never_records_requested_content_and_creation_keeps_absence() {
        let project = scratch("history-refused-save");
        let state = scratch("history-refused-save-state");
        let path = project.join("a");
        fs::write(&path, "A").unwrap();
        let c = HistoryCoordinator::start(
            RootIdentity::new(&project).unwrap(),
            state.to_path_buf(),
            vec![],
            |_, _| {},
        );
        wait_done(&c);
        let old = crate::document::stamp_at(&path).unwrap();
        fs::write(&path, "external contents with a different length").unwrap();
        assert!(
            c.mutate(&path, HistorySource::CideSave, || {
                crate::document::write_bytes_if_unchanged(
                    &path,
                    b"refused editor buffer",
                    Some(old),
                )
            })
            .is_err()
        );
        c.with_store(|s| {
            for r in s.list("a", None)?.revisions {
                assert_ne!(
                    s.bytes(&r.content)?.as_deref(),
                    Some(b"refused editor buffer".as_slice())
                );
            }
            Ok(())
        })
        .unwrap();
        let created = project.join("new");
        c.mutate(&created, HistorySource::FileOperation, || {
            fs::write(&created, "new")
        })
        .unwrap();
        c.with_store(|s| {
            let page = s.list("new", None)?;
            assert_eq!(page.revisions.len(), 2);
            assert_eq!(page.revisions[1].content, HistoryContent::Absent);
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn observer_barrier_captures_unopened_hidden_edits_and_external_deletion() {
        let project = scratch("history-hidden-feed");
        let state = scratch("history-hidden-feed-state");
        fs::create_dir(project.join(".hidden")).unwrap();
        let file = project.join(".hidden/source");
        fs::write(&file, "A").unwrap();
        let c = HistoryCoordinator::start_observed(
            RootIdentity::new(&project).unwrap(),
            state.to_path_buf(),
            vec![],
            |_, _| {},
        );
        c.confirm_watches(&[project.to_path_buf()]);
        let start = Instant::now();
        while !c.watch_directories().contains(&project.join(".hidden")) {
            c.confirm_watches(&[project.to_path_buf()]);
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(5));
        }
        c.with_store(|s| {
            assert!(s.list(".hidden/source", None)?.revisions.is_empty());
            Ok(())
        })
        .unwrap();
        c.confirm_watches(&[project.to_path_buf(), project.join(".hidden")]);
        wait_done(&c);
        let tree = cide_fs::Filter::build(
            &[project.to_path_buf()],
            vec![project.path()],
            cide_fs::Visibility::CONSERVATIVE,
        );
        assert!(!tree.watchable(&file, false));
        let consumer = c.clone();
        let feed = cide_fs::watch::HistorySubscription {
            directories: Arc::new(Vec::new),
            admits: Arc::new(|_, _| true),
            ready: Arc::new(|_| {}),
            degraded: Arc::new(|_| {}),
            changed: Arc::new(move |change| consumer.observe(change.paths, change.truncated)),
        };
        fs::write(&file, "B").unwrap();
        feed.deliver(cide_ipc::FsChange {
            paths: vec![file.clone()],
            truncated: false,
            git: false,
        });
        let start = Instant::now();
        while c
            .with_store(|s| Ok(s.list(".hidden/source", None)?.revisions.len()))
            .unwrap()
            < 2
        {
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(5));
        }
        fs::remove_file(&file).unwrap();
        feed.deliver(cide_ipc::FsChange {
            paths: vec![file],
            truncated: false,
            git: false,
        });
        let start = Instant::now();
        while !c
            .with_store(|s| {
                Ok(s.latest(".hidden/source")
                    .is_some_and(|r| r.content == HistoryContent::Absent))
            })
            .unwrap()
        {
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(5));
        }
        c.with_store(|s| {
            assert_eq!(s.list(".hidden/source", None)?.revisions.len(), 3);
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn watcher_admission_stays_bounded_while_a_direct_write_holds_authority() {
        let project = scratch("history-queue-root");
        let state = scratch("history-queue-state");
        fs::write(project.join("a"), "A").unwrap();
        let c = HistoryCoordinator::start(
            RootIdentity::new(&project).unwrap(),
            state.to_path_buf(),
            vec![],
            |_, _| {},
        );
        wait_done(&c);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = c.clone();
        let path = project.join("a");
        let job = thread::spawn(move || {
            writer.mutate(&path, HistorySource::CideSave, || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                fs::write(&path, "B")
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..MAX_BATCHES + 4 {
            c.observe(vec![project.join("a")], false);
        }
        assert!(
            c.status()
                .coverage
                .degraded
                .as_deref()
                .unwrap()
                .contains("queue")
        );
        assert!(c.shared.queued_bytes.load(Ordering::Acquire) <= MAX_QUEUED_BYTES);
        release_tx.send(()).unwrap();
        job.join().unwrap().unwrap();
        c.close();
    }
    fn wait_done(c: &HistoryCoordinator) {
        let start = Instant::now();
        while c.status().coverage.baseline_running {
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn baseline_external_delete_and_reopen_observe_closed_periods() {
        let project = scratch("history-coordinator-root");
        let state = scratch("history-coordinator-state");
        fs::write(project.join(".hidden"), "baseline").unwrap();
        let root = RootIdentity::new(&project).unwrap();
        let c = HistoryCoordinator::start(root.clone(), state.to_path_buf(), vec![], |_, _| {});
        wait_done(&c);
        c.with_store(|s| {
            assert_eq!(s.list(".hidden", None)?.revisions.len(), 1);
            Ok(())
        })
        .unwrap();
        fs::remove_file(project.join(".hidden")).unwrap();
        c.observe(vec![project.join(".hidden")], false);
        c.close();
        fs::write(project.join(".hidden"), "offline").unwrap();
        let c = HistoryCoordinator::start(root, state.to_path_buf(), vec![], |_, _| {});
        wait_done(&c);
        c.with_store(|s| {
            assert!(s.list(".hidden", None)?.revisions.len() >= 2);
            assert!(s.coverage.notices.iter().any(|n| n.contains("closed")));
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn direct_save_serializes_echo_and_history_failure_keeps_mutation_successful() {
        let project = scratch("history-direct-root");
        let state = scratch("history-direct-state");
        fs::write(project.join("file"), "A").unwrap();
        let c = HistoryCoordinator::start(
            RootIdentity::new(&project).unwrap(),
            state.to_path_buf(),
            vec![],
            |_, _| {},
        );
        let result = c.mutate(&project.join("file"), HistorySource::CideSave, || {
            fs::write(project.join("file"), "B")
        });
        result.unwrap();
        wait_done(&c);
        c.with_store(|s| {
            assert_eq!(s.list("file", None)?.revisions.len(), 2);
            Ok(())
        })
        .unwrap();
        c.with_store(|s| {
            s.set_policy(HistoryPolicy {
                budget_bytes: 1,
                ..HistoryPolicy::default()
            })
        })
        .unwrap();
        c.mutate(&project.join("file"), HistorySource::CideSave, || {
            fs::write(project.join("file"), "C")
        })
        .unwrap();
        assert_eq!(fs::read_to_string(project.join("file")).unwrap(), "C");
        assert!(c.status().coverage.degraded.is_some());
    }
    #[test]
    fn overflow_truncation_and_clear_reconcile_without_stale_jobs() {
        let project = scratch("history-overflow-root");
        let state = scratch("history-overflow-state");
        fs::write(project.join("a"), "A").unwrap();
        let c = HistoryCoordinator::start(
            RootIdentity::new(&project).unwrap(),
            state.to_path_buf(),
            vec![],
            |_, _| {},
        );
        wait_done(&c);
        c.observe(vec![project.join("a"); MAX_BATCH_PATHS + 1], false);
        c.observe(vec![], true);
        assert!(c.status().coverage.degraded.is_some());
        let generation = c
            .with_store(|s| {
                let old = s.generation();
                s.clear(true)?;
                Ok(old)
            })
            .unwrap();
        assert!(
            c.with_store(|s| s.capture(generation, "a", Some(b"stale"), HistorySource::External))
                .is_err()
        );
        c.close();
        assert!(c.with_store(|_| Ok(())).is_err());
    }
}
