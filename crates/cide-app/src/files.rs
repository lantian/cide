//! One file index, picker and watcher per open project.
//!
//! # Why the app owns the wiring
//!
//! `cide-fs` walks and watches; `cide-search` matches. Neither knows what a project is, and
//! neither should — the domain crates are testable precisely because they do not. Joining
//! them is three decisions that only the app can make: which roots a project has, that the
//! walk's sink is the picker's injector, and that a coalesced change becomes a `cide://`
//! event.
//!
//! # The lifetimes involved
//!
//! [`ProjectFs`] is held by an `Arc` in the registry and by nothing else. The watcher's
//! callback holds a `Weak`, deliberately: an `Arc` there would be a cycle — the project owns
//! the watcher, the watcher's closure owns the project — and closing a project would leave
//! its index, its 100k-entry matcher and its inotify descriptors alive for the life of the
//! process. With a `Weak`, dropping the registry entry drops the watcher, whose `Drop` stops
//! the thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use cide_fs::{BuildOptions, Filter, Index, Root, WalkItem, WatchConfig, WatchEvent, Watcher};
use cide_ipc::{FsChange, FsStatus, ProjectId, WatchBackend, WatchStatus};
use cide_search::{Candidate, Matcher as _, NucleoMatcher};
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tauri::AppHandle;

/// Where a project's `cide://fs-*` events go.
///
/// In the running app this is the `AppHandle`, and the impl below is three lines of
/// forwarding to [`crate::emit`]. It is a trait for one reason: a walk driven by an
/// `AppHandle` can only be started from inside a Tauri app, and the property that matters
/// here — the picker answering while the walk is still running — has no test at the command
/// layer unless the walk can be started from an ordinary `#[test]`.
///
/// The alternative was tauri's `test` feature and `mock_app()`. It loses twice: the feature
/// is a dev-dependency line in `crates/cide-app/Cargo.toml`, and `fs_index` also needs a
/// managed [`crate::workspace_state::WorkspaceState`], which loads *and saves* the user's
/// real workspace file. A vtable call per emitted event is the cheaper half of that trade.
pub trait FsEvents: Send + Sync + 'static {
    fn status(&self, project: ProjectId, status: &FsStatus);
    fn changed(&self, project: ProjectId, change: &FsChange);
}

impl FsEvents for AppHandle {
    fn status(&self, project: ProjectId, status: &FsStatus) {
        crate::emit::fs_status(self, project, status);
    }

    fn changed(&self, project: ProjectId, change: &FsChange) {
        crate::emit::fs_changed(self, project, change);
    }
}

/// Every open project's file state.
#[derive(Default)]
pub struct FsRegistry {
    projects: DashMap<ProjectId, Arc<ProjectFs>>,
}

impl FsRegistry {
    pub fn get(&self, project: ProjectId) -> Option<Arc<ProjectFs>> {
        self.projects.get(&project).map(|e| Arc::clone(e.value()))
    }

    /// Take a project's file state out of the registry.
    ///
    /// Returns the entry rather than a bool so the caller decides *where* it is dropped:
    /// the drop stops the watcher thread and frees a 100k-entry matcher, which is work, and
    /// `fs_close` hands it to a blocking worker rather than doing it on the async runtime.
    #[must_use = "dropping the entry here does the watcher teardown on this thread"]
    pub fn remove(&self, project: ProjectId) -> Option<Arc<ProjectFs>> {
        self.projects.remove(&project).map(|(_, fs)| fs)
    }

    pub fn close_all(&self) {
        self.projects.clear();
    }

    /// Claim a project for indexing, creating its entry if it has none.
    ///
    /// Cheap: map bookkeeping, two atomics, and — when the entry is new — the handful of
    /// `stat`s [`Filter::build`] spends on the global gitignore and each root's
    /// `info/exclude`. Bounded by the number of roots rather than by the size of the tree,
    /// which is what lets it stay on the caller's thread while the walk it hands back goes to
    /// a blocking one. That split is what lets `fs_index` be an async command: the entry is
    /// in the registry, and so answerable by the picker, before the walk has read a single
    /// inode of the project.
    ///
    /// # `Err(status)` — there is nothing to do
    ///
    /// Two cases, and the caller wants the same thing from both: the status of the index that
    /// already exists, not a second walk of the same tree.
    ///
    /// * A walk is **running**. The honest answer is that walk's `indexing: true` and its
    ///   live picker.
    /// * A walk has already **finished** over exactly these roots. This is the every-day
    ///   case, not the rare one: `store/workspace.ts` remembers what it has indexed, but that
    ///   memory is module state in one webview, so a second window, a detached pane, or a
    ///   reloaded webview asks again for every open project — with the first walk long since
    ///   done. `ui/src/ipc/client.ts` documents the command as "safe to call twice; the
    ///   second is a no-op", and this is where that becomes true. Re-walking instead is worse
    ///   than wasteful: [`Indexing::run`] opens by dropping the watcher and clearing the
    ///   matcher, so the second window's request would empty the *first* window's Ctrl+P and
    ///   stop its file events for the length of a fresh walk.
    ///
    /// A project whose **roots changed** is not either case — its entry is dropped below and
    /// it is walked again. That distinction is the whole reason this is keyed on the root
    /// list rather than on the project id.
    pub fn claim(&self, project: ProjectId, roots: Vec<PathBuf>) -> Result<Indexing, FsStatus> {
        // A project can gain or lose a root between two indexings. Reusing the old entry
        // would then walk the old set for ever, with no error anywhere to say so — the tree
        // would simply be missing a root the user added.
        //
        // The comparison is done in a statement of its own: `DashMap::remove` while a `Ref`
        // into the same shard is alive is a self-deadlock, and letting the guard live to the
        // end of an `if let` block is exactly how that happens.
        let stale = self
            .projects
            .get(&project)
            .is_some_and(|entry| entry.root_paths() != roots);
        if stale {
            self.projects.remove(&project);
        }

        let entry = self.projects.entry(project).or_insert_with(|| {
            Arc::new(ProjectFs::new(
                roots.iter().cloned().map(Root::new).collect(),
            ))
        });
        let fs = Arc::clone(entry.value());
        drop(entry);

        // Checked before the flag is taken, so a repeat call neither walks nor disturbs the
        // flag a concurrent walk owns. `walked` is only ever set by a walk that ran to
        // completion, which is what keeps the panic-retry property the guard below exists
        // for: a walk that unwound leaves `walked` false, so the retry still walks.
        if fs.walked.load(Ordering::Acquire) {
            return Err(fs.status());
        }
        if fs.indexing.swap(true, Ordering::AcqRel) {
            let status = fs.status();
            return Err(status);
        }
        // The flag is cleared by a guard rather than by a `store` at the end of the happy
        // path. A panic anywhere in the walk would otherwise leave it set for ever, and the
        // consequence is silent and total: every later `fs.index` takes the branch above and
        // returns immediately, so the project keeps an empty tree, an empty picker and no
        // watcher, with nothing anywhere saying why. Tauri catches the panic and the user
        // sees one failed command; a retry has to be able to work. The guard now also covers
        // the walk being dropped mid-flight with the blocking worker it runs on.
        let guard = IndexingGuard(Arc::clone(&fs));
        Ok(Indexing { fs, guard })
    }
}

/// A claimed walk, waiting to be run.
///
/// Holding one is what `indexing: true` means; running it or dropping it clears the flag.
pub struct Indexing {
    fs: Arc<ProjectFs>,
    guard: IndexingGuard,
}

impl Indexing {
    /// Walk the roots, filling the picker as the walk runs, then start watching.
    ///
    /// **Blocking**, and unapologetically so: seconds on a large repository. Call it from
    /// `spawn_blocking`; `cmd::fs::fs_index` is the only caller in the app and does.
    pub fn run(self, events: Arc<dyn FsEvents>, project: ProjectId) -> FsStatus {
        let Indexing { fs, guard } = self;

        // A re-index of an already-indexed project starts from an empty picker, or the old
        // paths would be offered alongside the new ones for ever. The old watcher goes with
        // it: leaving it running would have it folding changes into an index that is about
        // to be thrown away, and its watch list is the one the previous walk produced.
        *fs.watcher.lock() = None;
        fs.matcher.clear();
        events.status(project, &fs.status());

        let matcher = Arc::clone(&fs.matcher);
        let index = Index::build(
            fs.roots.clone(),
            BuildOptions::default(),
            &move |batch: &[WalkItem]| {
                for item in batch.iter().filter(|i| !i.is_dir) {
                    matcher.push(Candidate::new(
                        item.rel.clone(),
                        item.path.to_string_lossy().into_owned(),
                    ));
                }
            },
        );

        let root_paths: Vec<PathBuf> = fs.roots.iter().map(|r| r.path.clone()).collect();
        let dirs = index.dir_paths();
        let filter = Arc::new(Filter::build(&root_paths, dirs.iter().map(|p| p.as_path())));

        *fs.index.write() = index;
        *fs.filter.write() = Arc::clone(&filter);
        drop(guard);

        let config = WatchConfig {
            roots: root_paths,
            dirs,
            ..WatchConfig::default()
        };
        let watcher = Watcher::start(config, filter, {
            let events = Arc::clone(&events);
            let weak = Arc::downgrade(&fs);
            move |event| on_watch_event(&events, project, &weak, event)
        });
        *fs.watcher.lock() = Some(watcher);

        // Last, and only on this path. `FsRegistry::claim` reads it to turn a repeat
        // `fs.index` into a no-op, so it must mean "this tree has been walked and is being
        // watched" and nothing weaker — set it before `Watcher::start` and a walk that failed
        // to start a watcher would be permanently unwatchable with no way to ask again.
        fs.walked.store(true, Ordering::Release);

        let status = fs.status();
        events.status(project, &status);
        status
    }
}

/// Clears `ProjectFs::indexing` however the walk ends, including by unwinding.
struct IndexingGuard(Arc<ProjectFs>);

impl Drop for IndexingGuard {
    fn drop(&mut self) {
        self.0.indexing.store(false, Ordering::Release);
    }
}

/// One project's index, picker and watcher.
pub struct ProjectFs {
    pub roots: Vec<Root>,
    index: RwLock<Index>,
    filter: RwLock<Arc<Filter>>,
    matcher: Arc<NucleoMatcher>,
    /// `Option` because it only exists once the walk has produced a directory list, and
    /// because dropping it is how watching stops.
    watcher: Mutex<Option<Watcher>>,
    watch_status: Mutex<WatchStatus>,
    indexing: AtomicBool,
    /// A walk has run to completion over [`Self::roots`] and a watcher is installed.
    ///
    /// Distinct from `!indexing`, which is also true *before* the first walk. This is what
    /// makes a repeat `fs.index` a no-op; see [`FsRegistry::claim`].
    walked: AtomicBool,
}

impl ProjectFs {
    fn new(roots: Vec<Root>) -> Self {
        Self {
            index: RwLock::new(Index::empty(roots.clone())),
            filter: RwLock::new(Arc::new(Filter::build(
                &roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
                Vec::new(),
            ))),
            roots,
            matcher: Arc::new(NucleoMatcher::new()),
            watcher: Mutex::new(None),
            watch_status: Mutex::new(WatchStatus {
                backend: WatchBackend::Native,
                reason: None,
                watched_dirs: 0,
            }),
            indexing: AtomicBool::new(false),
            walked: AtomicBool::new(false),
        }
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| r.path.clone()).collect()
    }

    pub fn matcher(&self) -> &NucleoMatcher {
        &self.matcher
    }

    /// Is a walk running? One atomic — [`Self::status`] answers the same question but locks
    /// the index and the watch status to do it, and `picker_query` asks this per keystroke.
    pub fn is_indexing(&self) -> bool {
        self.indexing.load(Ordering::Acquire)
    }

    /// Read the tree. Held only for the length of one window request.
    pub fn with_index<T>(&self, f: impl FnOnce(&Index) -> T) -> T {
        f(&self.index.read())
    }

    pub fn with_index_mut<T>(&self, f: impl FnOnce(&mut Index) -> T) -> T {
        f(&mut self.index.write())
    }

    pub fn status(&self) -> FsStatus {
        let index = self.index.read();
        FsStatus {
            indexing: self.indexing.load(Ordering::Acquire),
            files: index.files(),
            dirs: index.dirs(),
            rows: index.count() as u32,
            watch: self.watch_status.lock().clone(),
        }
    }
}

/// Fold one watcher event back into the index, the picker and the UI.
fn on_watch_event(
    events: &Arc<dyn FsEvents>,
    project: ProjectId,
    weak: &Weak<ProjectFs>,
    event: WatchEvent,
) {
    let Some(fs) = weak.upgrade() else {
        // The project closed while this event was in flight.
        return;
    };
    match event {
        WatchEvent::Changed(change) => {
            let filter = Arc::clone(&fs.filter.read());
            let added = fs.with_index_mut(|index| index.apply(&change, &filter));
            for item in added.iter().filter(|i| !i.is_dir) {
                fs.matcher.push(Candidate::new(
                    item.rel.clone(),
                    item.path.to_string_lossy().into_owned(),
                ));
            }
            events.changed(project, &change);
        }
        WatchEvent::Status(status) => {
            *fs.watch_status.lock() = status;
            events.status(project, &fs.status());
        }
    }
}
