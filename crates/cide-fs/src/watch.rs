//! The filesystem watcher: debounced, ignore-filtered, and degradable.
//!
//! # Two layers of coalescing, because they solve different problems
//!
//! `notify-debouncer-full` collapses events **per path**: five writes to one file within the
//! debounce window become one event, and a rename is paired with its other half. That is not
//! the same as what the UI needs. `touch`ing 5000 files produces 5000 *distinct* paths, and
//! the debouncer will happily deliver all of them — in one batch if their timers happen to
//! expire in the same tick, in several if the writes straddle a tick boundary. A file tree
//! that repaints two or three times for one `cargo build` is the bug this crate was asked to
//! avoid, so [`Coalescer`] sits on top and holds paths until the *tree* has been quiet.
//!
//! [`WatchConfig::max_wait`] exists so that a build which never goes quiet still produces
//! updates: without it, a continuous writer starves the tree indefinitely.
//!
//! # Watches are per directory, not recursive
//!
//! A recursive watch on the project root is one line of code and watches `target/` and
//! `.git/objects` along with everything else — tens of thousands of inotify descriptors for
//! directories the user cannot see, which is the fastest route to `ENOSPC`. So every
//! non-ignored directory the walk found gets its own non-recursive watch, plus the handful
//! of git paths in [`crate::filter::Filter::git_paths`] that the `.git` rule would
//! otherwise exclude.
//!
//! **"Non-ignored" stayed literal when the file tree learned to show ignored files.** The
//! watch list is [`crate::Index::watch_dirs`] and every event is tested with
//! [`crate::Filter::watchable`], neither of which softens for the setting — so turning
//! *Show ignored files* on adds rows and adds not one descriptor, and a `cargo build` produces
//! exactly the events it produced before. `crate::filter`'s module note has the trade this
//! buys and what it costs.
//!
//! The exception is scale: past [`WatchConfig::max_dir_watches`] the setup cost of
//! individual watches (`notify-debouncer-full` keeps its watch roots in a `Vec` and scans it
//! on every `watch` call, so N watches cost O(N²)) dominates, and one recursive watch per
//! root is used instead. Events are filtered either way, so the difference is descriptors
//! and setup time, never correctness.
//!
//! # ENOSPC
//!
//! `fs.inotify.max_user_watches` is 8192 on some distributions and shared across every
//! process the user is running. Exhausting it surfaces as `notify::ErrorKind::MaxFilesWatch`,
//! and the watcher rebuilds itself around `PollWatcher` rather than dying — with the reason
//! carried to the UI, because a 30-second poll interval is a real behaviour change and the
//! fix (`sysctl fs.inotify.max_user_watches`) is one the user can act on.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use cide_ipc::{FsChange, WatchBackend, WatchStatus};
use notify::{ErrorKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer_opt};

use crate::filter::Filter;

/// What the watcher tells the app about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// One coalesced burst.
    Changed(FsChange),
    /// The backend changed, or reported itself at startup. Drives the degraded-mode banner.
    Status(WatchStatus),
}

#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub roots: Vec<PathBuf>,
    /// Every non-ignored directory, from the walk.
    pub dirs: Vec<PathBuf>,
    /// How long `notify-debouncer-full` holds a single path.
    pub debounce: Duration,
    /// How long the whole tree must be quiet before a burst is released.
    pub quiet: Duration,
    /// The longest a burst is held while changes keep arriving.
    pub max_wait: Duration,
    /// The most paths one `FsChange` will carry before it is marked truncated.
    pub max_paths: usize,
    /// Past this many directories, watch roots recursively instead.
    pub max_dir_watches: usize,
    /// Start in polling mode without waiting for inotify to fail.
    ///
    /// Not only a test hook: inotify does not work at all on NFS or SMB, where a change made
    /// on the server produces no event ever. A user whose project lives on a network share
    /// needs this, and the alternative — waiting for a failure that never comes, because
    /// nothing failed — leaves the tree quietly stale.
    pub polling: bool,
    /// How often polling re-stats the tree. Only used in polling mode.
    pub poll_interval: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            dirs: Vec::new(),
            debounce: Duration::from_millis(300),
            quiet: Duration::from_millis(300),
            max_wait: Duration::from_secs(2),
            max_paths: 4096,
            max_dir_watches: 8192,
            polling: false,
            // 30 seconds: polling means stat-ing every file in the tree, and on the large
            // repositories that get here that is not something to do every second.
            poll_interval: Duration::from_secs(30),
        }
    }
}

/// A running watcher. Dropping it stops the thread and releases every descriptor.
#[derive(Debug)]
pub struct Watcher {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    /// Start watching. `sink` is called from the watcher's own thread.
    pub fn start(
        config: WatchConfig,
        filter: Arc<Filter>,
        mut sink: impl FnMut(WatchEvent) + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("cide-fs-watch".into())
            .spawn(move || run(config, filter, &flag, &mut sink))
            .expect("could not spawn the fs watcher thread");
        Self {
            stop,
            join: Some(join),
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            // Bounded by one `TICK`, and worth waiting for: the debouncer's own thread and
            // every inotify descriptor go away with it, and a project that is closed and
            // reopened would otherwise accumulate both.
            let _ = join.join();
        }
    }
}

/// How often the loop wakes to release a due burst when nothing is arriving.
const TICK: Duration = Duration::from_millis(25);

fn run(
    config: WatchConfig,
    filter: Arc<Filter>,
    stop: &AtomicBool,
    sink: &mut impl FnMut(WatchEvent),
) {
    let (tx, rx) = mpsc::channel::<DebounceEventResult>();
    let (mut backend, mut status) = Backend::start(&config, &filter, tx.clone());
    sink(WatchEvent::Status(status.clone()));

    let mut coalescer = Coalescer::new(config.quiet, config.max_wait, config.max_paths);

    while !stop.load(Ordering::Acquire) {
        match rx.recv_timeout(TICK) {
            Ok(Ok(events)) => {
                for event in events {
                    for path in &event.paths {
                        let is_dir = path.is_dir();
                        // `watchable`, not `admits`: with *show ignored files* on, the tree
                        // contains `target/` and this loop still must not. See the note at the
                        // top of `crate::filter` — one event per object file a build writes is
                        // exactly the storm the two layers of coalescing below exist to
                        // prevent, and the cheapest place to stop it is before it is queued.
                        if !filter.watchable(path, is_dir) {
                            continue;
                        }
                        // A directory that has just appeared needs its own watch, and so do
                        // the directories under it: a `git clone` or a `mkdir -p a/b/c`
                        // creates the whole tree before we hear about the top of it.
                        if is_dir && backend.per_directory && !backend.watched.contains(path) {
                            backend.watch_tree(path, &filter);
                        }
                        coalescer.push(path.clone(), filter.is_git_path(path));
                    }
                }
            }
            Ok(Err(errors)) => {
                let exhausted = errors
                    .iter()
                    .any(|e| matches!(e.kind, ErrorKind::MaxFilesWatch));
                for error in &errors {
                    tracing::warn!(kind = ?error.kind, paths = ?error.paths, "watcher error");
                }
                if exhausted && !backend.is_polling() {
                    let reason = enospc_reason(backend.watched.len());
                    tracing::warn!(%reason, "degrading the file watcher to polling");
                    // The old backend is dropped by the assignment, releasing the descriptors
                    // it did manage to take. A `PollWatcher` needs none of them, so the two
                    // being briefly alive together costs nothing.
                    (backend, status) = Backend::start_polling(&config, tx.clone(), reason);
                    sink(WatchEvent::Status(status.clone()));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            // Every sender is gone, which can only happen if the debouncer thread died.
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if let Some(change) = coalescer.take_due(Instant::now()) {
            sink(WatchEvent::Changed(change));
        }
    }

    // Anything still pending is worth one last delivery: the common way to reach here is a
    // project closing, but a stop during a build would otherwise drop a real change.
    if let Some(change) = coalescer.take_pending() {
        sink(WatchEvent::Changed(change));
    }
}

fn enospc_reason(watched: usize) -> String {
    format!(
        "The kernel refused more inotify watches after {watched} directories \
         (fs.inotify.max_user_watches). Falling back to polling every 30s — changes made \
         outside cide will take longer to appear. Raising the limit with \
         `sysctl fs.inotify.max_user_watches=524288` restores instant updates."
    )
}

/// The two watcher flavours behind one interface.
///
/// An enum rather than a trait object because `Debouncer<T, C>` is generic over the watcher
/// and the two instantiations are different types; there are exactly two and there will not
/// be a third.
enum Inner {
    Native(Debouncer<RecommendedWatcher, RecommendedCache>),
    Polling(Debouncer<notify::PollWatcher, RecommendedCache>),
}

struct Backend {
    inner: Inner,
    /// Everything currently under a watch, so a directory created twice is not watched
    /// twice and the ENOSPC message can say how far it got.
    watched: HashSet<PathBuf>,
    /// False when the roots are watched recursively, in which case a new directory is
    /// already covered.
    per_directory: bool,
}

impl Backend {
    /// Start watching, degrading to polling if the kernel refuses the descriptors.
    fn start(
        config: &WatchConfig,
        filter: &Filter,
        tx: mpsc::Sender<DebounceEventResult>,
    ) -> (Self, WatchStatus) {
        if config.polling {
            let reason = "Polling was requested for this project. Changes are noticed within \
                          one poll interval rather than immediately."
                .to_string();
            return Self::start_polling(config, tx, reason);
        }

        let per_directory = config.dirs.len() <= config.max_dir_watches;
        let debouncer = match new_debouncer_opt::<_, RecommendedWatcher, RecommendedCache>(
            config.debounce,
            None,
            tx.clone(),
            RecommendedCache::new(),
            notify::Config::default(),
        ) {
            Ok(d) => d,
            Err(err) => {
                let reason = format!("The native file watcher could not start ({err}).");
                return Self::start_polling(config, tx, reason);
            }
        };

        let mut backend = Self {
            inner: Inner::Native(debouncer),
            watched: HashSet::new(),
            per_directory,
        };

        let targets: Vec<(PathBuf, RecursiveMode)> = if per_directory {
            config
                .dirs
                .iter()
                .cloned()
                .map(|d| (d, RecursiveMode::NonRecursive))
                .chain(
                    filter
                        .git_paths()
                        .iter()
                        .cloned()
                        .map(|p| (p, RecursiveMode::NonRecursive)),
                )
                .collect()
        } else {
            tracing::info!(
                dirs = config.dirs.len(),
                cap = config.max_dir_watches,
                "too many directories for per-directory watches; watching roots recursively"
            );
            config
                .roots
                .iter()
                .cloned()
                .map(|r| (r, RecursiveMode::Recursive))
                .collect()
        };

        for (path, mode) in targets {
            if let Err(err) = backend.watch(&path, mode) {
                if matches!(err.kind, ErrorKind::MaxFilesWatch) {
                    let reason = enospc_reason(backend.watched.len());
                    tracing::warn!(%reason, "degrading the file watcher to polling");
                    return Self::start_polling(config, tx, reason);
                }
                // A directory that vanished between the walk and the watch is ordinary.
                tracing::debug!(path = %path.display(), kind = ?err.kind, "could not watch");
            }
        }

        let status = WatchStatus {
            backend: WatchBackend::Native,
            reason: None,
            watched_dirs: backend.watched.len() as u32,
        };
        (backend, status)
    }

    fn start_polling(
        config: &WatchConfig,
        tx: mpsc::Sender<DebounceEventResult>,
        reason: String,
    ) -> (Self, WatchStatus) {
        // Contents deliberately not compared: polling already means stat-ing every file in
        // the tree, and hashing them as well would turn a degraded mode into an unusable one
        // on the large repositories that are the reason we are here at all.
        let notify_config = notify::Config::default()
            .with_poll_interval(config.poll_interval)
            .with_compare_contents(false);
        let debouncer = new_debouncer_opt::<_, notify::PollWatcher, RecommendedCache>(
            config.debounce,
            None,
            tx,
            RecommendedCache::new(),
            notify_config,
        )
        .expect("PollWatcher has no resources to run out of");

        let mut backend = Self {
            inner: Inner::Polling(debouncer),
            watched: HashSet::new(),
            per_directory: false,
        };
        for root in &config.roots {
            if let Err(err) = backend.watch(root, RecursiveMode::Recursive) {
                tracing::warn!(path = %root.display(), kind = ?err.kind, "could not poll-watch a root");
            }
        }
        let status = WatchStatus {
            backend: WatchBackend::Polling,
            reason: Some(reason),
            watched_dirs: 0,
        };
        (backend, status)
    }

    fn is_polling(&self) -> bool {
        matches!(self.inner, Inner::Polling(_))
    }

    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        match &mut self.inner {
            Inner::Native(d) => d.watch(path, mode)?,
            Inner::Polling(d) => d.watch(path, mode)?,
        }
        self.watched.insert(path.to_path_buf());
        Ok(())
    }

    /// Watch a newly created directory and everything non-ignored under it.
    ///
    /// The contents are re-read by the index anyway, so this is only about not going blind
    /// inside a directory that appeared after the walk.
    fn watch_tree(&mut self, path: &Path, filter: &Filter) {
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if self.watched.contains(&dir) {
                continue;
            }
            if let Err(err) = self.watch(&dir, RecursiveMode::NonRecursive) {
                tracing::debug!(path = %dir.display(), kind = ?err.kind, "could not watch a new directory");
                continue;
            }
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                let child = entry.path();
                // `watchable` for the same reason as the event loop: a `git clone` that lands
                // a directory the ignore rules cover is drawn, and not descended into here.
                if entry.file_type().is_ok_and(|t| t.is_dir()) && filter.watchable(&child, true) {
                    stack.push(child);
                }
            }
        }
    }
}

/// Holds a burst of changed paths until the tree goes quiet.
///
/// Pure state plus a clock passed in, so the interesting behaviour — one notification for
/// 5000 files, a release under sustained load, the truncation cap — is testable without a
/// filesystem or a sleep.
#[derive(Debug)]
pub struct Coalescer {
    quiet: Duration,
    max_wait: Duration,
    max_paths: usize,
    /// Sorted and deduplicated: 5000 writes to 100 files are 100 paths.
    paths: BTreeSet<PathBuf>,
    truncated: bool,
    git: bool,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Coalescer {
    pub fn new(quiet: Duration, max_wait: Duration, max_paths: usize) -> Self {
        Self {
            quiet,
            max_wait,
            max_paths,
            paths: BTreeSet::new(),
            truncated: false,
            git: false,
            first: None,
            last: None,
        }
    }

    pub fn push(&mut self, path: PathBuf, git: bool) {
        self.push_at(path, git, Instant::now());
    }

    pub fn push_at(&mut self, path: PathBuf, git: bool, now: Instant) {
        self.git |= git;
        if self.paths.len() >= self.max_paths && !self.paths.contains(&path) {
            // Past the cap the paths stop being useful individually — a receiver that has to
            // re-read anyway does not benefit from a longer list — but the *fact* that they
            // were dropped is what keeps the tree from going stale.
            self.truncated = true;
        } else {
            self.paths.insert(path);
        }
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// The burst, if it is ready to go out.
    pub fn take_due(&mut self, now: Instant) -> Option<FsChange> {
        let last = self.last?;
        let first = self.first?;
        let due =
            now.duration_since(last) >= self.quiet || now.duration_since(first) >= self.max_wait;
        due.then(|| self.take())
    }

    /// The burst regardless of timing. For shutdown.
    pub fn take_pending(&mut self) -> Option<FsChange> {
        (!self.paths.is_empty() || self.truncated).then(|| self.take())
    }

    fn take(&mut self) -> FsChange {
        let change = FsChange {
            paths: std::mem::take(&mut self.paths).into_iter().collect(),
            truncated: std::mem::take(&mut self.truncated),
            git: std::mem::take(&mut self.git),
        };
        self.first = None;
        self.last = None;
        change
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coalescer() -> Coalescer {
        Coalescer::new(Duration::from_millis(300), Duration::from_secs(2), 4)
    }

    #[test]
    fn five_thousand_paths_become_one_change() {
        let mut c = Coalescer::new(Duration::from_millis(300), Duration::from_secs(60), 8192);
        let t0 = Instant::now();
        for i in 0..5_000 {
            // Spread across 250ms, i.e. straddling several debouncer ticks.
            let at = t0 + Duration::from_micros(i * 50);
            c.push_at(PathBuf::from(format!("/p/{i}")), false, at);
            assert!(
                c.take_due(at).is_none(),
                "released while writes were still arriving"
            );
        }
        let change = c
            .take_due(t0 + Duration::from_millis(600))
            .expect("the burst should be released once the tree is quiet");
        assert_eq!(change.paths.len(), 5_000);
        assert!(!change.truncated);
        assert!(c.take_due(t0 + Duration::from_secs(10)).is_none());
    }

    #[test]
    fn repeated_writes_to_one_file_are_one_path() {
        let mut c = coalescer();
        let t0 = Instant::now();
        for i in 0..100 {
            c.push_at(PathBuf::from("/p/a"), false, t0 + Duration::from_millis(i));
        }
        let change = c.take_due(t0 + Duration::from_secs(1)).unwrap();
        assert_eq!(change.paths, vec![PathBuf::from("/p/a")]);
    }

    #[test]
    fn a_burst_that_never_goes_quiet_is_released_after_max_wait() {
        let mut c = coalescer();
        let t0 = Instant::now();
        let mut released = None;
        for i in 0..1_000u64 {
            let at = t0 + Duration::from_millis(i * 10);
            c.push_at(PathBuf::from(format!("/p/{i}")), false, at);
            if let Some(change) = c.take_due(at) {
                released = Some((i, change));
                break;
            }
        }
        let (i, change) = released.expect("max_wait must release a sustained burst");
        assert_eq!(i, 200, "released at 2s of continuous writes");
        assert!(change.truncated, "the cap is 4 paths in this test");
        assert_eq!(change.paths.len(), 4);
    }

    #[test]
    fn a_git_path_marks_the_change() {
        let mut c = coalescer();
        let t0 = Instant::now();
        c.push_at(PathBuf::from("/p/src/a.rs"), false, t0);
        c.push_at(PathBuf::from("/p/.git/HEAD"), true, t0);
        let change = c.take_due(t0 + Duration::from_secs(1)).unwrap();
        assert!(change.git);
        // And the flag does not leak into the next burst.
        c.push_at(PathBuf::from("/p/src/b.rs"), false, t0);
        assert!(!c.take_due(t0 + Duration::from_secs(1)).unwrap().git);
    }

    #[test]
    fn an_empty_coalescer_produces_nothing() {
        let mut c = coalescer();
        assert!(c.take_due(Instant::now()).is_none());
        assert!(c.take_pending().is_none());
    }
}
