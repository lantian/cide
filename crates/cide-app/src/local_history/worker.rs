//! The thread that keeps one project's store true to its files.
//!
//! # One thread, and why
//!
//! Everything that writes to a store runs here: the watcher's bursts, cide's saves, a reconcile
//! pass and retention, as chunks with the job queue drained between them. So there is no story
//! about a reconcile racing a capture or a compaction racing an append: they cannot overlap. The
//! one thing it does not do is the walk, which holds a shared permit for as long as it runs, so
//! that gets a thread of its own and reports back through the queue.
//!
//! Nothing here touches Tauri, which is what lets `tests.rs` drive it with real files, real
//! walks and no window.
//!
//! # Three sources, because none of them is enough alone
//!
//! * **The watcher** sees agents, terminals and other programs, but it reports only paths, after
//!   the old bytes are gone, and it says nothing about what happened while it was not running.
//! * **A save** knows the exact bytes, and it can look at the file before overwriting it.
//! * **A reconcile pass** compares the disk with the store. It is the only source that can
//!   produce a first baseline, or notice what happened while cide was closed, and the only one
//!   that survives a burst too large for the watcher to itemise.
//!
//! Content hashes reconcile the three: `Store::commit` compares with the path's head, so the same
//! change reported twice is one version however it arrived.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Read as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cide_fs::budget::{Budget, SharedBudget};
use cide_fs::{BuildOptions, Exclusions, Filter, Index, Root, Visibility};
use cide_ipc::FsChange;
use cide_local_history::{
    BASELINE_SHARE, Entry, Error, Head, MAX_ROWS, Policy, RootId, Stamp, Store, TargetState,
    VersionKind, VersionSource, local_day,
};
use parking_lot::{Mutex, RwLock};

/// Jobs queued before a sender gives up and marks the project for a full pass instead.
const QUEUE: usize = 1024;
/// A batch's limits. Each is one fsync pair, and the store is write-locked while it deflates, so
/// these bound how long a query can wait behind a baseline.
const BATCH_FILES: usize = 256;
const BATCH_BYTES: u64 = 8 << 20;
/// Paths stat'ed per step of a pass, before the queue is looked at again.
const STAT_CHUNK: usize = 512;
/// Entries a directory event may expand to before it becomes a full pass.
const EXPAND_LIMIT: usize = 20_000;
/// Far above any real project, so that a refusal means something odd rather than a big repo.
const WALK_BUDGET: u64 = 4 << 30;
/// Git's racy rule: a stamp this close to the moment it was read cannot rule out a later write
/// in the same timestamp tick. 2 s covers FAT and HFS+, ext4's tick is far smaller.
const RACY_MS: i64 = 2_000;
/// The longest the worker sleeps, so a stop flag or a due pass is noticed without being sent.
const IDLE: Duration = Duration::from_secs(1);
/// How often a window hears that versions landed. A baseline commits constantly, and an open
/// history tab refetching on every batch would be the only thing it did.
const NOTIFY_EVERY: Duration = Duration::from_millis(500);
const LOCK_ATTEMPTS: usize = 5;
const WORKER_GONE: &str = "Local History has stopped for this project.";

/// The timings a test wants to be zero.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// How long a path must stay gone before it is called deleted. `mv foo foo~; write foo`, the
    /// way every careful editor saves, passes through "missing".
    pub delete_recheck: Duration,
    /// The gap between passes. A long build dirties the tree every 2 s, and a pass stats every
    /// file in the project, so without this a build is a permanent full scan.
    pub scan_gap: Duration,
    /// How often to pass anyway when the watcher cannot be relied on.
    pub backstop: Duration,
    pub compact_every: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            delete_recheck: Duration::from_millis(500),
            scan_gap: Duration::from_secs(30),
            backstop: Duration::from_secs(600),
            compact_every: Duration::from_secs(24 * 60 * 60),
        }
    }
}

/// The user's limits, resolved. Replaced whole when Settings moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_file_size: u64,
    pub policy: Policy,
}

impl Limits {
    pub fn of(settings: &cide_ipc::LocalHistorySettings) -> Self {
        Self {
            max_file_size: u64::from(settings.max_file_size_mib) << 20,
            policy: Policy {
                days_to_keep: settings.days_to_keep,
                max_storage: u64::from(settings.max_storage_mib) << 20,
                max_rows: MAX_ROWS,
            },
        }
    }
}

/// A root as the project spells it, and what it really is.
#[derive(Debug, Clone)]
pub struct ScopeRoot {
    /// As spelled in `Project::roots`, which is what the filter's matchers and the watcher's
    /// paths are written against.
    pub path: PathBuf,
    /// What identity is computed from. Assigned at scope creation so a symlinked project and its
    /// target land on one row per file.
    pub canonical: PathBuf,
    /// `None` until the store is open.
    pub id: Option<RootId>,
}

/// What a store is watching right now. Rebuilt per index generation.
#[derive(Debug, Clone)]
pub struct Scope {
    pub roots: Vec<ScopeRoot>,
    pub filter: Arc<Filter>,
    /// The walk's exclusions: the user's, plus `.cide`. See [`Scope::new`].
    pub exclusions: Arc<Exclusions>,
    /// The index's budget. Held for `stopped`: past it, new directories go unwatched in silence.
    pub index_budget: Arc<Budget>,
}

impl Scope {
    /// `rules` are the project's effective indexing exclusions.
    ///
    /// `.cide` joins them for the walk alone, as a prune: `Filter::capturable` would refuse every
    /// file under it anyway, but only after visiting them, and `.cide/worktrees` is a complete
    /// checkout per agent run, whatever the user's list says.
    pub fn new(
        roots: Vec<PathBuf>,
        filter: Arc<Filter>,
        rules: &[String],
        index_budget: Arc<Budget>,
    ) -> Result<Self, String> {
        let mut rules = rules.to_vec();
        rules.push(".cide".to_owned());
        let exclusions = Arc::new(Exclusions::compile(roots.clone(), &rules)?);
        let roots = roots
            .into_iter()
            .map(|path| ScopeRoot {
                canonical: path.canonicalize().unwrap_or_else(|_| path.clone()),
                path,
                id: None,
            })
            .collect();
        Ok(Self {
            roots,
            filter,
            exclusions,
            index_budget,
        })
    }

    /// Which root a path belongs to and what it is called there.
    ///
    /// Identity is the **canonical parent plus the name**. The tree can reach one file by two
    /// spellings through a link, and a row per spelling would split its history. The name is left
    /// alone because a symlink is not to be followed: it is the link that is being asked about.
    ///
    /// Lexical only when the parent is gone, which is what a deletion looks like, and which is
    /// why it cannot be asked the other way round. A canonical answer outside every root is a
    /// refusal, not a reason to fall back: that is a link pointing out of the project.
    pub fn locate(&self, path: &Path) -> Option<(usize, String)> {
        fn best<'a>(
            roots: &'a [ScopeRoot],
            path: &Path,
            of: impl Fn(&'a ScopeRoot) -> &'a Path,
        ) -> Option<(usize, String)> {
            let (index, root) = roots
                .iter()
                .enumerate()
                .filter(|(_, root)| path.starts_with(of(root)))
                .max_by_key(|(_, root)| of(root).as_os_str().len())?;
            // `to_str` is the non-UTF-8 refusal: `rel` is a `String` because every row is JSON.
            let rel = path.strip_prefix(of(root)).ok()?.to_str()?;
            Some((index, rel.to_owned()))
        }

        let name = path.file_name()?;
        match path.parent().map(Path::canonicalize) {
            Some(Ok(parent)) => best(&self.roots, &parent.join(name), |root| &root.canonical),
            _ => best(&self.roots, path, |root| &root.path),
        }
    }

    /// The spelling the filter's matchers understand.
    fn spelled(&self, root: usize, rel: &str) -> PathBuf {
        self.roots[root].path.join(rel)
    }
}

/// A file's previous contents, read just before a save replaced them.
#[derive(Debug)]
pub struct Before {
    bytes: Vec<u8>,
    mode: u32,
}

struct Saved {
    path: PathBuf,
    before: Option<Before>,
    bytes: Vec<u8>,
    mode: u32,
}

enum Job {
    Wake,
    Changed(Vec<PathBuf>, bool),
    Saved(Box<Saved>),
    Moved(Vec<(PathBuf, PathBuf)>),
    Rescope(Box<Scope>, bool),
    Pause(bool),
    Run(Box<dyn FnOnce(&mut Worker) + Send>),
    Walked(Box<Walked>),
    #[cfg(test)]
    Settle(mpsc::Sender<()>),
}

/// Everything shared with the rest of the app. The worker is the only writer of the store.
struct Shared {
    dir: PathBuf,
    primary_root: PathBuf,
    store: RwLock<Option<Store>>,
    scope: RwLock<Arc<Scope>>,
    limits: RwLock<Limits>,
    stop: AtomicBool,
    /// Capture is switched off and this store stays open only to be read and restored from.
    /// A worker with nothing to record costs a sleeping thread, and it means browsing, restoring
    /// and clearing have one road whether the switch is on or off, with no second opener that
    /// could hold the lock at the moment a real worker starts.
    paused: AtomicBool,
    /// A send failed because the queue was full. Whatever it carried is recovered by a full pass.
    overflowed: AtomicBool,
    notify: Box<dyn Fn() + Send + Sync>,
}

/// A running store. Owned by the registry, which is the one that knows about projects.
pub struct Capture {
    shared: Arc<Shared>,
    tx: SyncSender<Job>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Capture {
    /// Start the worker. Returns at once: the store is opened on the worker, because replaying a
    /// log can take seconds, and this is called from the watcher's thread.
    ///
    /// `after` is the worker of a store being replaced. It is joined first, or the new worker's
    /// `Store::open` would find the lock still held and come up read-only for the whole session,
    /// which is what a quick off-and-on would otherwise produce.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        dir: PathBuf,
        primary_root: PathBuf,
        scope: Scope,
        limits: Limits,
        timing: Timing,
        paused: bool,
        notify: Box<dyn Fn() + Send + Sync>,
        after: Option<JoinHandle<()>>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let shared = Arc::new(Shared {
            dir,
            primary_root,
            store: RwLock::new(None),
            scope: RwLock::new(Arc::new(scope)),
            limits: RwLock::new(limits),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(paused),
            overflowed: AtomicBool::new(false),
            notify,
        });
        let worker = Worker::new(Arc::clone(&shared), rx, tx.clone(), timing);
        let thread = std::thread::Builder::new()
            .name("cide-local-history".into())
            .spawn(move || worker.run(after))
            .ok();
        Arc::new(Self {
            shared,
            tx,
            thread: Mutex::new(thread),
        })
    }

    fn send(&self, job: Job) {
        if self.tx.try_send(job).is_err() {
            self.shared.overflowed.store(true, Ordering::Release);
        }
    }

    /// The roots this store covers, as spelled.
    pub fn covers(&self, path: &Path) -> bool {
        self.shared
            .scope
            .read()
            .roots
            .iter()
            .any(|root| path.starts_with(&root.path) || path.starts_with(&root.canonical))
    }

    /// A new index generation: new matchers, maybe new roots. `gap` says events may have been
    /// lost, which is true of everything except a settings change.
    pub fn rescope(&self, scope: Scope, gap: bool) {
        self.send(Job::Rescope(Box::new(scope), gap));
    }

    pub fn set_limits(&self, limits: Limits) {
        *self.shared.limits.write() = limits;
    }

    /// Switch capture off or on. Off stops at the next chunk and keeps everything recorded so
    /// far. On owes a pass, because nothing was watching in between.
    pub fn set_paused(&self, paused: bool) {
        // A flag as well as a job, so `before_save` and `saved` stop at once, ahead of the queue.
        self.shared.paused.store(paused, Ordering::Release);
        self.send(Job::Pause(paused));
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }

    /// Run on the worker and wait for the answer. For the few things a command needs done in
    /// order with everything else, which is any write to the store.
    fn on_worker<T: Send + 'static>(
        &self,
        run: impl FnOnce(&mut Worker) -> T + Send + 'static,
    ) -> Option<T> {
        let (reply, answer) = mpsc::sync_channel(1);
        let job = Job::Run(Box::new(move |worker| {
            let _ = reply.send(run(worker));
        }));
        self.tx.send(job).ok()?;
        answer.recv().ok()
    }

    /// Put a name on this moment.
    pub fn label(&self, text: String) -> Result<(), String> {
        self.on_worker(move |worker| worker.record_label(text))
            .unwrap_or_else(|| Err(WORKER_GONE.to_owned()))
    }

    /// Make a file exactly what a version says. See [`Worker::restore`].
    pub fn restore(&self, request: RestoreRequest) -> Result<Restored, String> {
        self.on_worker(move |worker| worker.restore(&request))
            .unwrap_or_else(|| Err(WORKER_GONE.to_owned()))
    }

    /// Forget every version of this store.
    pub fn clear(&self) -> Result<(), String> {
        self.on_worker(Worker::clear)
            .unwrap_or_else(|| Err(WORKER_GONE.to_owned()))
    }

    /// Put a folder back to how it was. See [`Worker::restore_tree`].
    pub fn restore_tree(&self, request: TreeRequest) -> Result<TreeReport, String> {
        self.on_worker(move |worker| worker.restore_tree(&request))
            .unwrap_or_else(|| Err(WORKER_GONE.to_owned()))
    }

    /// Which root a path is under, and its name there.
    pub fn locate(&self, path: &Path) -> Option<(RootId, String)> {
        let scope = Arc::clone(&self.shared.scope.read());
        let (root, rel) = scope.locate(path)?;
        Some((scope.roots[root].id?, rel))
    }

    /// [`Self::locate`] for a folder or a root: where a path is, whether or not it is a file.
    ///
    /// `locate` resolves a parent and keeps the name, which for a root itself lands outside it.
    pub fn root_scope(&self, path: &Path) -> Option<(RootId, String)> {
        let scope = Arc::clone(&self.shared.scope.read());
        let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        for root in &scope.roots {
            if real == root.canonical
                && let Some(id) = root.id
            {
                return Some((id, String::new()));
            }
        }
        self.locate(path)
    }

    /// The absolute path of a row, as the project spells its root.
    pub fn path_of(&self, root: RootId, rel: &str) -> Option<PathBuf> {
        let scope = self.shared.scope.read();
        let at = scope.roots.iter().find(|r| r.id == Some(root))?;
        Some(if rel.is_empty() {
            at.path.clone()
        } else {
            at.path.join(rel)
        })
    }

    /// Why a path has no history, or `None` when it would have one.
    pub fn untracked_reason(&self, path: &Path) -> Option<&'static str> {
        let scope = Arc::clone(&self.shared.scope.read());
        let Some((root, rel)) = scope.locate(path) else {
            return Some("This file is outside the project, so Local History does not keep it.");
        };
        (!scope.filter.capturable(&scope.spelled(root, &rel), false)).then_some(
            "This file is gitignored, excluded from indexing or under .cide, so Local History \
             does not keep it.",
        )
    }

    /// From the watcher's thread. Enqueues and nothing else: this thread also carries the file
    /// tree's updates.
    pub fn changed(&self, change: &FsChange) {
        if !self.is_paused() {
            self.send(Job::Changed(change.paths.clone(), change.truncated));
        }
    }

    /// Call on the save's own thread, before it writes.
    ///
    /// Answers the old bytes only when the disk is not the version the store has, which is to
    /// say when something changed the file behind cide's back. That matters most on a stale
    /// Ctrl+S, which overwrites without asking: this is the only moment that external change
    /// still exists. An ordinary save reads nothing here.
    pub fn before_save(&self, path: &Path) -> Option<Before> {
        if self.is_paused() {
            return None;
        }
        let scope = Arc::clone(&self.shared.scope.read());
        let (root, rel) = scope.locate(path)?;
        if !scope.filter.capturable(&scope.spelled(root, &rel), false) {
            return None;
        }
        let meta = fs::symlink_metadata(path).ok().filter(Metadata::is_file)?;
        // `try_read`: a save must never wait on a compaction. Not knowing the head means reading
        // the old bytes, which costs a read and is always safe.
        let known = self
            .shared
            .store
            .try_read()
            .and_then(|guard| {
                let store = guard.as_ref()?;
                let head = store.head(scope.roots[root].id?, &rel)?;
                Some(head.stamp == Some(Stamp::of(&meta)))
            })
            .unwrap_or(false);
        if known {
            return None;
        }
        let limit = self.shared.limits.read().max_file_size;
        match read_stable(path, limit) {
            Ok(Snapshot::Bytes { bytes, mode, .. }) => Some(Before { bytes, mode }),
            _ => None,
        }
    }

    /// Call after the save's bytes are on disk.
    ///
    /// Carries the bytes rather than asking the worker to read them. During a baseline the queue
    /// can be minutes behind, and a path-only job would capture whatever the file has become by
    /// then, losing the version the user just saved.
    pub fn saved(&self, path: PathBuf, before: Option<Before>, bytes: Vec<u8>) {
        if self.is_paused() {
            return;
        }
        let mode = fs::symlink_metadata(&path).map_or(0o100644, |meta| meta.mode());
        self.send(Job::Saved(Box::new(Saved {
            path,
            before,
            bytes,
            mode,
        })));
    }

    /// cide moved these. Called before the tree hears about it where that is possible, so the
    /// watcher's view of the same move does not land first as a deletion and a creation.
    pub fn moved(&self, moves: Vec<(PathBuf, PathBuf)>) {
        self.send(Job::Moved(moves));
    }

    /// Ask the worker to finish. Sets a flag and returns: teardown must not wait on this, because
    /// the store is crash-only and an OS shutdown allows five seconds.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = self.tx.try_send(Job::Wake);
    }

    /// [`Self::stop`], handing back the thread to join.
    pub fn stop_and_take(&self) -> Option<JoinHandle<()>> {
        self.stop();
        self.thread.lock().take()
    }

    /// The store, for reading. `None` until it has been opened.
    pub fn with_store<T>(&self, read: impl FnOnce(&Store) -> T) -> Option<T> {
        self.shared.store.read().as_ref().map(read)
    }

    pub fn primary_root(&self) -> &Path {
        &self.shared.primary_root
    }

    /// Wait until the worker has nothing left to do. Tests only: in the app a pass is
    /// rate-limited, and nothing should ever wait for one.
    #[cfg(test)]
    pub fn settle(&self) {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Job::Settle(tx)).unwrap();
        rx.recv_timeout(Duration::from_secs(30))
            .expect("the worker settled");
    }
}

/// What a restore needs that only the caller knows.
pub struct RestoreRequest {
    pub path: PathBuf,
    pub version: u64,
    /// `cmd::file`'s read-only rule, which a restore must not get around.
    pub open_roots: Vec<PathBuf>,
    pub caches: Vec<PathBuf>,
}

/// A folder restore, by the moment it restores to.
pub struct TreeRequest {
    pub root: RootId,
    /// The folder, relative to the root. Empty for all of it.
    pub prefix: String,
    /// The newest row of the state being restored.
    pub at: u64,
    pub label: String,
    pub open_roots: Vec<PathBuf>,
    pub caches: Vec<PathBuf>,
    /// Where a removed file goes. A parameter so a test can hand scratch files to something other
    /// than the user's real Trash, and so that the one path here that removes a file has a test.
    pub trash: fn(&Path) -> cide_fs::Result<PathBuf>,
}

/// What a folder restore did. Files that failed are listed, because it carries on past them: a
/// half-restored folder with a list is recoverable, and the label it wrote first is the undo.
#[derive(Debug, Default)]
pub struct TreeReport {
    /// Absolute paths written or brought back.
    pub written: Vec<PathBuf>,
    /// Absolute paths moved to the trash.
    pub removed: Vec<PathBuf>,
    /// Files with no version that old, left as they are.
    pub left: u32,
    pub failed: Vec<(PathBuf, String)>,
}

/// How a restore landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restored {
    /// The file existed and now holds the version's bytes.
    Rewritten,
    /// The file was gone and has been created again.
    Recreated,
}

/// What reading a path found.
enum Snapshot {
    Bytes {
        bytes: Vec<u8>,
        stamp: Stamp,
        mode: u32,
    },
    TooLarge {
        size: u64,
        stamp: Stamp,
        mode: u32,
    },
    /// Gone, or not a regular file.
    Absent,
}

fn is_missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// Read a file so that the bytes and the stamp describe the same moment.
///
/// **One descriptor, `fstat` before and after.** A `stat` then an `open` is two questions about
/// possibly two files, and a program rewriting in place can be caught mid-write. If the stamp
/// moved during the read the bytes may be torn, so it tries again. After three tries it keeps
/// the last read, since a file written constantly will be read again on its next event.
///
/// `O_NOFOLLOW` because a link is never versioned, and `O_NONBLOCK` because the walk calls
/// everything that is not a directory a file: a FIFO swapped in after the `stat` would otherwise
/// block `open` for ever, and with it every project change behind it in the queue.
fn read_stable(path: &Path, limit: u64) -> io::Result<Snapshot> {
    let mut last = Snapshot::Absent;
    for _ in 0..3 {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path);
        let mut file = match file {
            Ok(file) => file,
            Err(e) if is_missing(&e) || e.raw_os_error() == Some(libc::ELOOP) => {
                return Ok(Snapshot::Absent);
            }
            Err(e) => return Err(e),
        };
        let before = file.metadata()?;
        if !before.is_file() {
            return Ok(Snapshot::Absent);
        }
        let stamp = Stamp::of(&before);
        let mode = before.mode();
        if before.len() > limit {
            return Ok(Snapshot::TooLarge {
                size: before.len(),
                stamp,
                mode,
            });
        }
        let mut bytes = Vec::with_capacity(before.len() as usize);
        (&mut file)
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)?;
        let settled = Stamp::of(&file.metadata()?) == stamp;
        last = Snapshot::Bytes { bytes, stamp, mode };
        if settled {
            break;
        }
    }
    Ok(last)
}

/// Whether the file at `path` is gone, for a caller about to say so.
///
/// Only `ENOENT` and `ENOTDIR` count. `EACCES`, `EIO` and a stale NFS handle say something is
/// wrong with reaching the file, not that the file is gone, and a burst of false deletions is
/// how a store loses the eviction protection on the only copies it has.
fn is_gone(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(meta) => !meta.is_file(),
        Err(error) => is_missing(&error),
    }
}

/// A short list of names that are a program's working files, gone by the time they could be read.
/// cide's own atomic saves use `.{name}.{pid}.{n}.tmp`, which passes the ignore rules.
fn is_transient(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    (name.starts_with('.') && name.ends_with(".tmp"))
        || name.starts_with(".#")
        || (name.starts_with('#') && name.ends_with('#'))
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name == "4913"
}

/// The path to write for a row, refusing one that would leave the project.
///
/// `check_within` is lexical, so `root/link/a.rs` passes it while `link` points anywhere. The
/// deepest ancestor that exists is what the kernel will actually follow, so that is what is
/// resolved, and it has to land inside the root's real path.
fn confined(scope: &Scope, root: usize, rel: &str) -> Result<PathBuf, String> {
    let at = &scope.roots[root];
    let target = at.path.join(rel);
    let mut ancestor = target.parent();
    while let Some(dir) = ancestor {
        match dir.canonicalize() {
            Ok(real) if real.starts_with(&at.canonical) => return Ok(target),
            Ok(_) => return Err("This path leads outside the project.".to_owned()),
            Err(error) if is_missing(&error) => ancestor = dir.parent(),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("This path leads outside the project.".to_owned())
}

/// What the walk found. `complete` is false when it stopped early, which rules out anything that
/// reasons from a file being absent from the list.
struct Walked {
    files: Vec<(usize, String, PathBuf)>,
    complete: bool,
}

struct Work {
    root: usize,
    rel: String,
    path: PathBuf,
    size: u64,
    stamp: Stamp,
    mode: u32,
}

enum Stage {
    Walking(Arc<Budget>),
    Stat { walked: Walked, at: usize },
    Capture { complete: bool, at: usize },
}

struct Scan {
    stage: Stage,
    seen: HashSet<(usize, String)>,
    work: Vec<Work>,
}

struct Worker {
    shared: Arc<Shared>,
    rx: Receiver<Job>,
    tx: SyncSender<Job>,
    timing: Timing,
    pending: BTreeSet<PathBuf>,
    /// Paths that looked gone, and when to look again.
    deletions: Vec<(Instant, usize, String, PathBuf)>,
    /// A full pass is owed.
    dirty: bool,
    scan: Option<Scan>,
    last_scan: Option<Instant>,
    last_compact: Option<Instant>,
    /// Another process holds the store. Reads still work, so this idles rather than exits.
    read_only: bool,
    /// Heads whose racy stamp has been re-read late enough to trust.
    verified: HashSet<(usize, String)>,
    last_notify: Option<Instant>,
    notify_owed: bool,
    #[cfg(test)]
    settling: Vec<mpsc::Sender<()>>,
}

impl Worker {
    fn new(shared: Arc<Shared>, rx: Receiver<Job>, tx: SyncSender<Job>, timing: Timing) -> Self {
        Self {
            shared,
            rx,
            tx,
            timing,
            pending: BTreeSet::new(),
            deletions: Vec::new(),
            dirty: true,
            scan: None,
            last_scan: None,
            last_compact: None,
            read_only: false,
            verified: HashSet::new(),
            last_notify: None,
            notify_owed: false,
            #[cfg(test)]
            settling: Vec::new(),
        }
    }

    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }

    fn scope(&self) -> Arc<Scope> {
        Arc::clone(&self.shared.scope.read())
    }

    fn run(mut self, after: Option<JoinHandle<()>>) {
        if let Some(previous) = after {
            let _ = previous.join();
        }
        if self.stopped() {
            return;
        }
        let Some(store) = self.open_store() else {
            return;
        };
        self.read_only = !store.is_writable();
        cide_local_history::write_origin(&self.shared.primary_root);
        *self.shared.store.write() = Some(store);
        self.assign_root_ids();
        (self.shared.notify)();

        while !self.stopped() {
            match self.rx.recv_timeout(self.next_wake()) {
                Ok(job) => {
                    self.accept(job);
                    let mut drained = 0;
                    while drained < QUEUE {
                        let Ok(job) = self.rx.try_recv() else { break };
                        self.accept(job);
                        drained += 1;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.step();
            #[cfg(test)]
            self.answer_settlers();
        }
        if let Some(Scan {
            stage: Stage::Walking(budget),
            ..
        }) = &self.scan
        {
            budget.cancel();
        }
    }

    /// Open the store, giving a passing reader a moment to let go of the lock.
    ///
    /// `cide-headless history` is a writer for as long as it runs, when nothing else holds the
    /// lock. Losing that race for the fraction of a second it takes must not make this worker
    /// read-only for the whole session.
    fn open_store(&self) -> Option<Store> {
        for attempt in 0..LOCK_ATTEMPTS {
            match Store::open(&self.shared.dir) {
                Ok(store) if store.is_writable() || attempt + 1 == LOCK_ATTEMPTS => {
                    return Some(store);
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(100)),
                Err(error) => {
                    tracing::warn!(%error, "could not open Local History");
                    return None;
                }
            }
        }
        None
    }

    /// How long the queue may stay quiet. Zero while there is work, so a chunk of a pass goes
    /// straight back to checking for a save rather than sleeping.
    fn next_wake(&self) -> Duration {
        let walking = matches!(
            self.scan,
            Some(Scan {
                stage: Stage::Walking(_),
                ..
            })
        );
        if !self.pending.is_empty() || (self.scan.is_some() && !walking) {
            return Duration::ZERO;
        }
        let now = Instant::now();
        let mut wake = if self.notify_owed { NOTIFY_EVERY } else { IDLE };
        if let Some(due) = self.deletions.iter().map(|(due, ..)| *due).min() {
            wake = wake.min(due.saturating_duration_since(now));
        }
        if self.dirty && self.scan.is_none() {
            let due = self.last_scan.map(|last| last + self.timing.scan_gap);
            wake = wake.min(due.map_or(Duration::ZERO, |due| due.saturating_duration_since(now)));
        }
        wake
    }

    /// Whether a pass may start. A bool and not an `Instant` to compare with: "no pass yet" has no
    /// moment of its own, and answering it with `Instant::now()` loses to any earlier `now`, so
    /// the first pass of every project would never have started.
    fn scan_is_due(&self) -> bool {
        self.last_scan
            .is_none_or(|last| last.elapsed() >= self.timing.scan_gap)
    }

    /// Whether the watcher has a blind spot only a periodic pass can see past.
    ///
    /// Past the index budget, new directories go unwatched with no event to say so. With hidden
    /// files off, the watcher refuses dotfiles while `capturable` keeps them.
    fn needs_backstop(&self) -> bool {
        let scope = self.scope();
        scope.index_budget.stopped() || !scope.filter.visibility().hidden
    }

    fn assign_root_ids(&mut self) {
        let mut guard = self.shared.store.write();
        let Some(store) = guard.as_mut() else { return };
        let current = self.shared.scope.read().clone();
        let mut scope = (*current).clone();
        for root in &mut scope.roots {
            let Some(canonical) = root.canonical.to_str() else {
                continue;
            };
            root.id = if self.read_only {
                store.root_id(canonical)
            } else {
                match store.ensure_root(canonical) {
                    Ok(id) => Some(id),
                    Err(error) => {
                        tracing::warn!(%error, "could not record a Local History root");
                        None
                    }
                }
            };
        }
        *self.shared.scope.write() = Arc::new(scope);
    }

    fn accept(&mut self, job: Job) {
        let capturing = matches!(job, Job::Changed(..) | Job::Saved(_) | Job::Moved(_));
        if capturing && self.paused() {
            return;
        }
        match job {
            Job::Wake => {}
            Job::Pause(paused) => self.dirty |= !paused,
            Job::Run(run) => {
                // In order: whatever is asked for here should see everything reported before it.
                self.process_pending();
                run(self);
            }
            Job::Changed(paths, truncated) => {
                self.dirty |= truncated;
                let scope = self.scope();
                let is_root = |path: &PathBuf| scope.roots.iter().any(|root| &root.path == path);
                // A truncated burst lists the roots as "something changed". The pass that
                // `dirty` asks for is the answer to that, so they must not expand here as well.
                self.pending
                    .extend(paths.into_iter().filter(|p| !(truncated && is_root(p))));
            }
            Job::Saved(saved) => self.record_save(*saved),
            Job::Moved(moves) => {
                // In order: a move refers to a state the earlier events have already brought in.
                self.process_pending();
                self.record_moves(moves);
            }
            Job::Rescope(scope, gap) => {
                *self.shared.scope.write() = Arc::new(*scope);
                self.assign_root_ids();
                self.dirty |= gap;
            }
            Job::Walked(walked) => {
                if let Some(scan) = &mut self.scan
                    && matches!(scan.stage, Stage::Walking(_))
                {
                    scan.stage = Stage::Stat {
                        walked: *walked,
                        at: 0,
                    };
                }
            }
            #[cfg(test)]
            Job::Settle(tx) => self.settling.push(tx),
        }
    }

    #[cfg(test)]
    fn answer_settlers(&mut self) {
        let idle = self.pending.is_empty()
            && self.deletions.is_empty()
            && self.scan.is_none()
            && !self.dirty;
        if idle {
            for tx in self.settling.drain(..) {
                let _ = tx.send(());
            }
        }
    }

    fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }

    fn step(&mut self) {
        self.send_notify();
        if self.read_only || self.paused() {
            self.pending.clear();
            self.deletions.clear();
            self.dirty = false;
            if let Some(Scan {
                stage: Stage::Walking(budget),
                ..
            }) = self.scan.take()
            {
                budget.cancel();
            }
            return;
        }
        if self.shared.overflowed.swap(false, Ordering::AcqRel) {
            self.dirty = true;
        }
        self.process_pending();
        self.process_deletions();
        self.step_scan();
        self.compact_if_due();

        let backstop_due = self
            .last_scan
            .is_some_and(|last| last.elapsed() >= self.timing.backstop);
        if backstop_due && self.needs_backstop() {
            self.dirty = true;
        }
    }

    /// Tell the windows versions landed, at most every [`NOTIFY_EVERY`].
    fn send_notify(&mut self) {
        let due = self
            .last_notify
            .is_none_or(|last| last.elapsed() >= NOTIFY_EVERY);
        if self.notify_owed && due {
            self.notify_owed = false;
            self.last_notify = Some(Instant::now());
            (self.shared.notify)();
        }
    }

    fn head(&self, root: RootId, rel: &str) -> Option<Head> {
        self.shared.store.read().as_ref()?.head(root, rel)
    }

    fn baseline_done(&self, root: RootId) -> bool {
        self.shared
            .store
            .read()
            .as_ref()
            .is_some_and(|store| store.baseline_done(root))
    }

    /// What to call a file with no usable head, seen by a watcher or a save.
    ///
    /// `Created` only once a durable scan marker says the root has been walked. Before that, a
    /// file with no row may simply not have been reached yet. See [`VersionKind::Baseline`].
    fn appeared(&self, root: RootId) -> VersionKind {
        if self.baseline_done(root) {
            VersionKind::Created
        } else {
            VersionKind::Baseline
        }
    }

    fn commit(&mut self, group: &mut Option<u64>, entries: Vec<Entry>) {
        if entries.is_empty() {
            return;
        }
        let ts = cide_core::persist::now_ms();
        let done = {
            let mut guard = self.shared.store.write();
            let Some(store) = guard.as_mut() else { return };
            store.commit_into(ts, *group, entries)
        };
        match done {
            Ok(done) => {
                *group = group.or(done.group);
                if done.rows > 0 {
                    self.notify_owed = true;
                }
            }
            Err(Error::ReadOnly) => self.read_only = true,
            // ENOSPC, most likely. The rows did not land, so their stamps did not move, and the
            // next pass will see these files as changed. Nothing is lost that can be kept.
            Err(error) => {
                tracing::warn!(%error, "could not record Local History rows");
                self.dirty = true;
            }
        }
    }

    /// Stamp says same, so skip the read? Git's racy rule is the reason this is not one `==`.
    fn unchanged(&mut self, root: usize, rel: &str, head: Option<Head>, stamp: Stamp) -> bool {
        let key = (root, rel.to_owned());
        let Some(head) = head.filter(|head| head.kind != VersionKind::Deleted) else {
            self.verified.remove(&key);
            return false;
        };
        if head.stamp != Some(stamp) {
            self.verified.remove(&key);
            return false;
        }
        let mtime_ms = stamp.mtime_ns / 1_000_000;
        let racy = (head.ts as i64 - mtime_ms).abs() < RACY_MS;
        if !racy || self.verified.contains(&key) {
            return true;
        }
        // Re-read, and trust the answer for good only if enough time has passed since the write
        // that no same-tick edit could still be hiding behind it.
        if cide_core::persist::now_ms() as i64 - mtime_ms >= RACY_MS {
            self.verified.insert(key);
        }
        false
    }

    fn process_pending(&mut self) {
        let mut group = None;
        let mut entries = Vec::new();
        let mut bytes = 0;
        while let Some(path) = self.pending.pop_first() {
            if self.stopped() {
                return;
            }
            let Some(entry) = self.observe(&path) else {
                continue;
            };
            bytes += entry.size;
            entries.push(entry);
            if entries.len() >= BATCH_FILES || bytes >= BATCH_BYTES {
                self.commit(&mut group, std::mem::take(&mut entries));
                bytes = 0;
            }
        }
        self.commit(&mut group, entries);
    }

    /// One path from a burst. Everything a path can turn out to be is decided here.
    fn observe(&mut self, path: &Path) -> Option<Entry> {
        let scope = self.scope();
        // Lexical first, and free: most of a burst around a git operation is `.git` paths.
        if is_transient(path) || !scope.filter.capturable(path, false) {
            return None;
        }
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                self.expand(&scope, path);
                None
            }
            Ok(meta) if meta.is_file() => {
                let (root, rel) = scope.locate(path)?;
                if !scope.filter.capturable(&scope.spelled(root, &rel), false) {
                    return None;
                }
                let id = scope.roots[root].id?;
                let head = self.head(id, &rel);
                if self.unchanged(root, &rel, head, Stamp::of(&meta)) {
                    return None;
                }
                let kind = match head {
                    Some(head) if head.kind != VersionKind::Deleted => VersionKind::Modified,
                    _ => self.appeared(id),
                };
                self.capture(id, rel, path, kind, VersionSource::External)
            }
            // A link, a FIFO, or gone. Whichever it is, the file that was here is not.
            Ok(_) => self.note_gone(&scope, path),
            Err(error) if is_missing(&error) => self.note_gone(&scope, path),
            Err(error) => {
                tracing::debug!(%error, path = %path.display(), "skipped a Local History path");
                None
            }
        }
    }

    fn capture(
        &self,
        root: RootId,
        rel: String,
        path: &Path,
        kind: VersionKind,
        source: VersionSource,
    ) -> Option<Entry> {
        let limit = self.shared.limits.read().max_file_size;
        match read_stable(path, limit) {
            Ok(Snapshot::Bytes { bytes, stamp, mode }) => {
                Some(Entry::new(root, rel, kind, source).content(bytes, mode, Some(stamp)))
            }
            Ok(Snapshot::TooLarge { size, stamp, mode }) => {
                Some(Entry::new(root, rel, kind, source).omit(size, mode, Some(stamp)))
            }
            Ok(Snapshot::Absent) => None,
            Err(error) => {
                tracing::debug!(%error, path = %path.display(), "could not read a changed file");
                None
            }
        }
    }

    /// A directory in a burst stands for its contents.
    ///
    /// `mv src lib` reports the two directories and not one file inside them, and files copied
    /// in before the watch attaches are never reported at all. So the directory is read here, by
    /// hand and without following links. A directory too large for this becomes a full pass.
    fn expand(&mut self, scope: &Scope, dir: &Path) {
        if !scope.filter.capturable(dir, true) {
            return;
        }
        let mut stack = vec![dir.to_path_buf()];
        let mut visited = 0;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                visited += 1;
                if visited > EXPAND_LIMIT {
                    self.dirty = true;
                    return;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    if scope.filter.capturable(&path, true) {
                        stack.push(path);
                    }
                } else if kind.is_file() {
                    self.pending.insert(path);
                }
            }
        }
    }

    /// A path is not a file any more. Schedules the deletion rather than recording it.
    ///
    /// A missing directory has no head of its own, so this is also where its files are found:
    /// by prefix, over the heads, since nothing on disk can say what used to be in it.
    fn note_gone(&mut self, scope: &Scope, path: &Path) -> Option<Entry> {
        let (root, rel) = scope.locate(path)?;
        let id = scope.roots[root].id?;
        let due = Instant::now() + self.timing.delete_recheck;
        if self
            .head(id, &rel)
            .is_some_and(|head| head.kind != VersionKind::Deleted)
        {
            self.deletions.push((due, root, rel, path.to_path_buf()));
            return None;
        }
        let prefix = format!("{rel}/");
        let heads = self
            .shared
            .store
            .read()
            .as_ref()
            .map(|store| store.heads(id))
            .unwrap_or_default();
        for (child, head) in heads {
            if head.kind != VersionKind::Deleted && child.starts_with(&prefix) {
                let path = scope.spelled(root, &child);
                self.deletions.push((due, root, child, path));
            }
        }
        None
    }

    fn process_deletions(&mut self) {
        let now = Instant::now();
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.deletions)
            .into_iter()
            .partition(|(when, ..)| *when <= now);
        self.deletions = later;
        let scope = self.scope();
        let mut group = None;
        let mut entries = Vec::new();
        for (_, root, rel, path) in due {
            if !is_gone(&path) {
                // Back already: an editor's save-by-rename. It is a change, not a deletion.
                self.pending.insert(path);
                continue;
            }
            let Some(id) = scope.roots.get(root).and_then(|root| root.id) else {
                continue;
            };
            entries.push(Entry::new(
                id,
                rel,
                VersionKind::Deleted,
                VersionSource::External,
            ));
            if entries.len() >= BATCH_FILES {
                self.commit(&mut group, std::mem::take(&mut entries));
            }
        }
        self.commit(&mut group, entries);
    }

    fn record_save(&mut self, saved: Saved) {
        if self.read_only {
            return;
        }
        let scope = self.scope();
        let Saved {
            path,
            before,
            bytes,
            mode,
        } = saved;
        let Some((root, rel)) = scope.locate(&path) else {
            return;
        };
        if !scope.filter.capturable(&scope.spelled(root, &rel), false) {
            return;
        }
        let Some(id) = scope.roots[root].id else {
            return;
        };
        let live = self
            .head(id, &rel)
            .is_some_and(|head| head.kind != VersionKind::Deleted);
        let mut kind = if live {
            VersionKind::Modified
        } else {
            self.appeared(id)
        };
        let mut entries = Vec::with_capacity(2);
        if let Some(before) = before {
            entries.push(
                Entry::new(id, rel.clone(), kind, VersionSource::External).content(
                    before.bytes,
                    before.mode,
                    None,
                ),
            );
            kind = VersionKind::Modified;
        }
        let limit = self.shared.limits.read().max_file_size;
        let entry = Entry::new(id, rel, kind, VersionSource::Saved);
        // No stamp, on purpose. One taken now could describe an external write that landed just
        // after the save, and then the watcher's event for that write would find the stamp
        // matching and skip the read, losing the version. Without one the event reads the file
        // once more and the hash settles it, which is one read per save.
        entries.push(if bytes.len() as u64 > limit {
            entry.omit(bytes.len() as u64, mode, None)
        } else {
            entry.content(bytes, mode, None)
        });
        self.commit(&mut None, entries);
    }

    fn record_moves(&mut self, moves: Vec<(PathBuf, PathBuf)>) {
        if self.read_only {
            return;
        }
        let scope = self.scope();
        let mut group = None;
        let mut entries = Vec::new();
        for (from, to) in moves {
            let Some((root, rel)) = scope.locate(&from) else {
                self.pending.insert(to);
                continue;
            };
            let Some(id) = scope.roots[root].id else {
                continue;
            };
            // One file, or a directory whose files are known only by their heads.
            let live = self
                .head(id, &rel)
                .is_some_and(|head| head.kind != VersionKind::Deleted);
            let pairs: Vec<(String, PathBuf)> = if live {
                vec![(rel, to.clone())]
            } else {
                let prefix = format!("{rel}/");
                self.shared
                    .store
                    .read()
                    .as_ref()
                    .map(|store| store.heads(id))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(child, head)| {
                        head.kind != VersionKind::Deleted && child.starts_with(&prefix)
                    })
                    .map(|(child, _)| {
                        let tail = child[prefix.len()..].to_owned();
                        (child, to.join(tail))
                    })
                    .collect()
            };
            // Nothing tracked here: not moved as far as history can tell, so it is simply new.
            if pairs.is_empty() {
                self.pending.insert(to);
                continue;
            }
            for (old, new_path) in pairs {
                let destination = scope.locate(&new_path).filter(|(new_root, new_rel)| {
                    scope
                        .filter
                        .capturable(&scope.spelled(*new_root, new_rel), false)
                });
                let Some((new_root, new_rel)) = destination else {
                    // Moved somewhere history does not follow, such as an ignored folder.
                    entries.push(Entry::new(
                        id,
                        old,
                        VersionKind::Deleted,
                        VersionSource::FileOp,
                    ));
                    continue;
                };
                let Some(new_id) = scope.roots[new_root].id else {
                    continue;
                };
                let created = self.capture(
                    new_id,
                    new_rel.clone(),
                    &new_path,
                    VersionKind::Created,
                    VersionSource::FileOp,
                );
                let deleted =
                    Entry::new(id, old.clone(), VersionKind::Deleted, VersionSource::FileOp);
                // A rename chain is walked within one root: `moved` is relative to it.
                if new_id == id {
                    entries.push(deleted.moved(new_rel));
                    entries.extend(created.map(|entry| entry.moved(old)));
                } else {
                    entries.push(deleted);
                    entries.extend(created);
                }
            }
            if entries.len() >= BATCH_FILES {
                self.commit(&mut group, std::mem::take(&mut entries));
            }
        }
        self.commit(&mut group, entries);
    }

    /// [`Self::commit`], for a caller that must not carry on if the rows did not land.
    fn commit_strict(&mut self, entries: Vec<Entry>) -> Result<(), String> {
        let ts = cide_core::persist::now_ms();
        let done = {
            let mut guard = self.shared.store.write();
            let store = guard.as_mut().ok_or(WORKER_GONE)?;
            store.commit(ts, entries)
        };
        match done {
            Ok(done) => {
                self.notify_owed |= done.rows > 0;
                Ok(())
            }
            Err(error) => {
                self.read_only |= matches!(error, Error::ReadOnly);
                Err(error.to_string())
            }
        }
    }

    fn record_label(&mut self, text: String) -> Result<(), String> {
        let scope = self.scope();
        let root = scope.roots.first().and_then(|root| root.id);
        let root = root.ok_or(WORKER_GONE)?;
        self.commit_strict(vec![
            Entry::new(root, "", VersionKind::Label, VersionSource::Saved).label(text),
        ])
    }

    /// Make a file exactly what a version says, and keep what it said before.
    ///
    /// **The current file is saved first, past every limit, and failing to save it stops the
    /// restore.** A restore is the one operation here that destroys content on purpose, and the
    /// promise that it can be undone is only as good as that copy. So it ignores the size limit
    /// and the storage cap, and it runs with capture switched off too: an explicit overwrite is
    /// not the place to honour "stop recording".
    ///
    /// The stamp is taken **before** that read. If anything writes the file after it, the
    /// conditional write below refuses rather than overwriting bytes nobody has a copy of.
    ///
    /// A missing file goes through `create_exact`, which uses `create_new`: an agent that
    /// recreated it in the meantime gets a refusal, where a rename would silently replace it.
    fn restore(&mut self, request: &RestoreRequest) -> Result<Restored, String> {
        if self.read_only {
            return Err(Error::ReadOnly.to_string());
        }
        let scope = self.scope();
        let (root, rel) = scope
            .locate(&request.path)
            .ok_or("This file is outside the project.")?;
        let id = scope.roots[root].id.ok_or(WORKER_GONE)?;
        let (handle, mode) = {
            let guard = self.shared.store.read();
            let store = guard.as_ref().ok_or(WORKER_GONE)?;
            let version = store
                .version(request.version)
                .ok_or("This version is no longer kept.")?;
            (store.blob(request.version), version.mode)
        };
        let handle = handle.ok_or("No copy of this version was kept.")?;
        let bytes = handle.read().map_err(|error| error.to_string())?;

        let target = confined(&scope, root, &rel)?;
        let mut existing = match fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_file() => true,
            Ok(_) => return Err("Something other than a file is at this path now.".to_owned()),
            Err(error) if is_missing(&error) => false,
            Err(error) => return Err(error.to_string()),
        };
        let stamp = cide_core::document::stamp_at(&target);
        if existing {
            match read_stable(&target, u64::MAX) {
                Ok(Snapshot::Bytes {
                    bytes: current,
                    stamp,
                    mode,
                }) => {
                    let live = self
                        .head(id, &rel)
                        .is_some_and(|head| head.kind != VersionKind::Deleted);
                    let kind = if live {
                        VersionKind::Modified
                    } else {
                        self.appeared(id)
                    };
                    self.commit_strict(vec![
                        Entry::new(id, rel.clone(), kind, VersionSource::External).content(
                            current,
                            mode,
                            Some(stamp),
                        ),
                    ])
                    .map_err(|error| {
                        format!(
                            "The current file could not be saved first, so nothing changed: {error}"
                        )
                    })?;
                }
                Ok(Snapshot::Absent) => existing = false,
                Ok(Snapshot::TooLarge { .. }) => unreachable!("the limit is u64::MAX"),
                Err(error) => {
                    return Err(format!(
                        "The current file could not be read first, so nothing changed: {error}"
                    ));
                }
            }
        }

        if existing {
            crate::cmd::file::write_document_bytes(
                &target,
                &bytes,
                &request.open_roots,
                &request.caches,
                stamp,
            )
            .map_err(|error| error.to_string())?;
        } else {
            if let Some(reason) = cide_core::toolchain::read_only_reason_in(
                &target,
                &request.open_roots,
                &request.caches,
            ) {
                return Err(reason);
            }
            let permissions = if mode & 0o777 == 0 {
                0o644
            } else {
                mode & 0o777
            };
            let writable = [scope.roots[root].path.clone()];
            cide_fs::ops::create_exact(&writable, &target, &bytes, permissions)
                .map_err(|error| error.to_string())?;
        }

        // The file has moved, so a failure past here reports nothing: the next event or pass will
        // pick the new content up, and telling the user the restore failed would be untrue.
        let kind = if existing {
            VersionKind::Modified
        } else {
            self.appeared(id)
        };
        let _ = self.commit_strict(vec![
            Entry::new(id, rel, kind, VersionSource::Restored).content(bytes, mode, None),
        ]);
        Ok(if existing {
            Restored::Rewritten
        } else {
            Restored::Recreated
        })
    }

    fn clear(&mut self) -> Result<(), String> {
        if let Some(Scan {
            stage: Stage::Walking(budget),
            ..
        }) = self.scan.take()
        {
            budget.cancel();
        }
        self.pending.clear();
        self.deletions.clear();
        self.verified.clear();
        let done = {
            let mut guard = self.shared.store.write();
            let store = guard.as_mut().ok_or(WORKER_GONE)?;
            store.clear()
        };
        done.map_err(|error| error.to_string())?;
        // With capture on, the next pass is a first scan again. Off, `step` discards this.
        self.dirty = true;
        self.notify_owed = true;
        Ok(())
    }

    /// Put a folder back to a moment, file by file, keeping everything it replaces.
    ///
    /// **The plan is computed here, on the worker, not taken from the preview.** The preview is a
    /// courtesy and it ages: a window can sit on it while an agent works. What is on the worker at
    /// this moment is what is true.
    ///
    /// A label goes in first and failing to write it stops everything: it is the undo point, and
    /// this is the one operation large enough to need one. After that each file stands alone,
    /// through the same [`Self::restore`] a single file takes, so it keeps the safety copy, the
    /// stamp check and the refusals, and one that fails is reported rather than ending the rest.
    fn restore_tree(&mut self, request: &TreeRequest) -> Result<TreeReport, String> {
        if self.read_only {
            return Err(Error::ReadOnly.to_string());
        }
        let scope = self.scope();
        let root = scope
            .roots
            .iter()
            .position(|root| root.id == Some(request.root))
            .ok_or("This folder is outside the project.")?;
        let plan = {
            let guard = self.shared.store.read();
            let store = guard.as_ref().ok_or(WORKER_GONE)?;
            store.plan_at(request.root, &request.prefix, request.at)
        };
        self.record_label(request.label.clone())?;

        let mut report = TreeReport::default();
        for file in plan {
            if self.stopped() {
                break;
            }
            let path = scope.spelled(root, &file.rel);
            match file.target {
                TargetState::Unknown => report.left += 1,
                TargetState::Content(version) => {
                    let current = fs::symlink_metadata(&path).ok().map(|m| Stamp::of(&m));
                    let head = self.head(request.root, &file.rel);
                    if file.unchanged && current.is_some() && head.and_then(|h| h.stamp) == current
                    {
                        continue;
                    }
                    let restored = self.restore(&RestoreRequest {
                        path: path.clone(),
                        version,
                        open_roots: request.open_roots.clone(),
                        caches: request.caches.clone(),
                    });
                    match restored {
                        Ok(_) => report.written.push(path),
                        Err(error) => report.failed.push((path, error)),
                    }
                }
                TargetState::Absent => match self.remove_file(request, root, &file.rel) {
                    Ok(true) => report.removed.push(path),
                    Ok(false) => {}
                    Err(error) => report.failed.push((path, error)),
                },
            }
        }
        Ok(report)
    }

    /// Trash a file a folder restore has shown did not exist, keeping a copy first.
    ///
    /// Answers whether anything was removed: the file may be gone already. To the trash, never
    /// `unlink`, and for [`Self::restore`]'s reason the copy failing stops this file. A stamp
    /// taken before that copy has to still hold after it, so a file that changed in between is
    /// left alone rather than removed with bytes nobody kept.
    fn remove_file(
        &mut self,
        request: &TreeRequest,
        root: usize,
        rel: &str,
    ) -> Result<bool, String> {
        let id = request.root;
        let scope = self.scope();
        let target = confined(&scope, root, rel)?;
        let Ok(meta) = fs::symlink_metadata(&target) else {
            return Ok(false);
        };
        if !meta.is_file() {
            return Ok(false);
        }
        let before = cide_core::document::stamp_at(&target);
        let Snapshot::Bytes { bytes, stamp, mode } = read_stable(&target, u64::MAX)
            .map_err(|error| format!("It could not be read first, so it was kept: {error}"))?
        else {
            return Ok(false);
        };
        let kind = match self.head(id, rel) {
            Some(head) if head.kind != VersionKind::Deleted => VersionKind::Modified,
            _ => self.appeared(id),
        };
        self.commit_strict(vec![
            Entry::new(id, rel.to_owned(), kind, VersionSource::External).content(
                bytes,
                mode,
                Some(stamp),
            ),
        ])
        .map_err(|error| format!("It could not be saved first, so it was kept: {error}"))?;
        if cide_core::document::stamp_at(&target) != before {
            return Err("It changed during the restore, so it was kept.".to_owned());
        }
        (request.trash)(&target).map_err(|error| error.to_string())?;
        self.commit_strict(vec![Entry::new(
            id,
            rel.to_owned(),
            VersionKind::Deleted,
            VersionSource::Restored,
        )])?;
        Ok(true)
    }

    fn start_scan(&mut self) {
        let scope = self.scope();
        let live: Vec<(usize, Root)> = scope
            .roots
            .iter()
            .enumerate()
            .filter(|(_, root)| root.path.is_dir())
            .map(|(index, root)| (index, Root::new(root.path.clone())))
            .collect();
        let shared = SharedBudget::new(WALK_BUDGET * 2);
        let budget = Budget::new(shared, WALK_BUDGET);
        let opts = BuildOptions {
            exclusions: Arc::clone(&scope.exclusions),
            // Its own answer, not the user's: what is recorded must not move with what is shown.
            visibility: Visibility {
                hidden: true,
                ignored: false,
            },
            ..BuildOptions::default()
        };
        let tx = self.tx.clone();
        let walk_budget = Arc::clone(&budget);
        let spawned = std::thread::Builder::new()
            .name("cide-history-walk".into())
            .spawn(move || {
                let files = Mutex::new(Vec::new());
                let roots: Vec<Root> = live.iter().map(|(_, root)| root.clone()).collect();
                Index::walk_roots_controlled(&roots, opts, false, &walk_budget, &|batch| {
                    let mut files = files.lock();
                    files.extend(
                        batch
                            .iter()
                            .filter(|item| !item.is_dir && !item.symlink)
                            .map(|item| {
                                let root = live[item.root as usize].0;
                                (root, item.rel.clone(), item.path.clone())
                            }),
                    );
                });
                let walked = Walked {
                    files: files.into_inner(),
                    complete: !walk_budget.stopped() && live.len() == scope.roots.len(),
                };
                let _ = tx.send(Job::Walked(Box::new(walked)));
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start a Local History walk");
            return;
        }
        self.dirty = false;
        self.scan = Some(Scan {
            stage: Stage::Walking(budget),
            seen: HashSet::new(),
            work: Vec::new(),
        });
    }

    fn step_scan(&mut self) {
        let Some(mut scan) = self.scan.take() else {
            if self.dirty && self.scan_is_due() {
                self.start_scan();
            }
            return;
        };
        match &mut scan.stage {
            Stage::Walking(_) => self.scan = Some(scan),
            Stage::Stat { .. } => {
                self.stat_chunk(&mut scan);
                self.scan = Some(scan);
            }
            Stage::Capture { complete, at } => {
                let (complete, at) = (*complete, *at);
                if at < scan.work.len() {
                    self.capture_chunk(&mut scan, at, complete);
                    self.scan = Some(scan);
                } else {
                    self.finish_scan(&scan.seen, complete);
                }
            }
        }
    }

    /// Stat a slice of the walk, keeping what needs reading.
    fn stat_chunk(&mut self, scan: &mut Scan) {
        let Stage::Stat { walked, at } = &mut scan.stage else {
            return;
        };
        let scope = self.scope();
        let end = (*at + STAT_CHUNK).min(walked.files.len());
        let files = std::mem::take(&mut walked.files);
        for (root, rel, path) in &files[*at..end] {
            // The walk's filter knows the rules it was built with, but has no exclusions of its
            // own and no `.cide`, and the user's `.gitignore` may have changed since.
            if is_transient(path) || !scope.filter.capturable(path, false) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            scan.seen.insert((*root, rel.clone()));
            let Some(id) = scope.roots[*root].id else {
                continue;
            };
            let stamp = Stamp::of(&meta);
            let head = self.head(id, rel);
            if !self.unchanged(*root, rel, head, stamp) {
                scan.work.push(Work {
                    root: *root,
                    rel: rel.clone(),
                    path: path.clone(),
                    size: meta.len(),
                    stamp,
                    mode: meta.mode(),
                });
            }
        }
        walked.files = files;
        *at = end;
        if *at >= walked.files.len() {
            let complete = walked.complete;
            // Smallest first while baselining, so a cap reached part way through has spent itself
            // on many source files and not on a few assets.
            let baselining = scope
                .roots
                .iter()
                .any(|root| root.id.is_some_and(|id| !self.baseline_done(id)));
            if baselining {
                scan.work.sort_by_key(|work| work.size);
            }
            scan.stage = Stage::Capture { complete, at: 0 };
        }
    }

    /// Read and record a batch of what the stat stage found.
    fn capture_chunk(&mut self, scan: &mut Scan, start: usize, complete: bool) {
        let scope = self.scope();
        let limits = *self.shared.limits.read();
        let share = limits.policy.max_storage as f64 * BASELINE_SHARE;
        let full = self
            .shared
            .store
            .read()
            .as_ref()
            .is_some_and(|store| store.disk_bytes() as f64 >= share);
        let mut entries = Vec::new();
        let mut bytes = 0;
        let mut at = start;
        while at < scan.work.len() && entries.len() < BATCH_FILES && bytes < BATCH_BYTES {
            let work = &scan.work[at];
            at += 1;
            let Some(id) = scope.roots[work.root].id else {
                continue;
            };
            let head = self.head(id, &work.rel);
            let known = head.is_some_and(|head| head.kind != VersionKind::Deleted);
            // Every sighting by a walk is a `Baseline`, including a file whose head says it was
            // deleted: a walk cannot tell when it came back.
            let (kind, source) = if known {
                (VersionKind::Modified, VersionSource::Detected)
            } else {
                (VersionKind::Baseline, VersionSource::Detected)
            };
            // Past the baseline's share, new files still get a row, with no bytes. The row is what
            // keeps a later edit from reading as a brand new file, which a folder restore would
            // be entitled to delete.
            let entry = if full && !known {
                Some(Entry::new(id, work.rel.clone(), kind, source).omit(
                    work.size,
                    work.mode,
                    Some(work.stamp),
                ))
            } else {
                self.capture(id, work.rel.clone(), &work.path, kind, source)
            };
            if let Some(entry) = entry {
                bytes += entry.size;
                entries.push(entry);
            }
        }
        scan.stage = Stage::Capture { complete, at };
        self.commit(&mut None, entries);
    }

    /// What can only be decided once the whole walk is in: what is missing from it.
    fn finish_scan(&mut self, seen: &HashSet<(usize, String)>, complete: bool) {
        self.last_scan = Some(Instant::now());
        if !complete {
            // A partial list cannot prove anything is gone, and cannot say a root is baselined.
            return;
        }
        let scope = self.scope();
        let due = Instant::now();
        let mut markers = Vec::new();
        for (index, root) in scope.roots.iter().enumerate() {
            let Some(id) = root.id else { continue };
            let heads = self
                .shared
                .store
                .read()
                .as_ref()
                .map(|store| store.heads(id))
                .unwrap_or_default();
            let live: Vec<_> = heads
                .into_iter()
                .filter(|(_, head)| head.kind != VersionKind::Deleted)
                .collect();
            let found = seen.iter().filter(|(at, _)| *at == index).count();
            // An empty root over a store full of files is an unmounted disk, or a mount point
            // with nothing on it. Real deletions arrive through the watcher, one by one, so
            // refusing to conclude anything from this costs nothing and saves every head.
            if found == 0 && !live.is_empty() {
                tracing::warn!(root = %root.path.display(), "a root looks empty; skipping deletions");
                continue;
            }
            for (rel, _) in live {
                if seen.contains(&(index, rel.clone())) {
                    continue;
                }
                let path = root.path.join(&rel);
                if is_gone(&path) {
                    self.deletions.push((due, index, rel, path));
                }
            }
            if !self.baseline_done(id) {
                markers.push(Entry::new(
                    id,
                    "",
                    VersionKind::BaselineDone,
                    VersionSource::Detected,
                ));
            }
        }
        self.commit(&mut None, markers);
    }

    fn compact_if_due(&mut self) {
        // After a pass, so that the first compaction of a session sees the disk as it is.
        if self.scan.is_some() || self.last_scan.is_none() {
            return;
        }
        if self
            .last_compact
            .is_some_and(|last| last.elapsed() < self.timing.compact_every)
        {
            return;
        }
        self.last_compact = Some(Instant::now());
        let policy = self.shared.limits.read().policy;
        let done = {
            let mut guard = self.shared.store.write();
            let Some(store) = guard.as_mut() else { return };
            store.compact(&policy, &local_day)
        };
        match done {
            Ok(done) if done.dropped_rows > 0 => self.notify_owed = true,
            Ok(_) => {}
            Err(Error::ReadOnly) => self.read_only = true,
            Err(error) => tracing::warn!(%error, "could not compact Local History"),
        }
    }
}
