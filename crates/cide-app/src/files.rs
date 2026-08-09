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
use cide_ipc::{FsStatus, ProjectId, WatchBackend, WatchStatus};
use cide_search::{Candidate, Matcher as _, NucleoMatcher};
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tauri::AppHandle;

/// Every open project's file state.
#[derive(Default)]
pub struct FsRegistry {
    projects: DashMap<ProjectId, Arc<ProjectFs>>,
}

impl FsRegistry {
    pub fn get(&self, project: ProjectId) -> Option<Arc<ProjectFs>> {
        self.projects.get(&project).map(|e| Arc::clone(e.value()))
    }

    /// Take a project's file state out of the registry, stopping its watcher.
    pub fn remove(&self, project: ProjectId) -> bool {
        self.projects.remove(&project).is_some()
    }

    pub fn close_all(&self) {
        self.projects.clear();
    }

    /// Index a project: walk its roots, fill the picker as the walk runs, then watch.
    ///
    /// Runs on the calling thread, which for a Tauri command is one of its worker threads,
    /// so a two-second walk on a large repository blocks nothing the user can see. The
    /// picker is answerable throughout — that is the whole reason the entry is registered
    /// before the walk starts rather than after it finishes.
    pub fn index(&self, app: &AppHandle, project: ProjectId, roots: Vec<PathBuf>) -> FsStatus {
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

        if fs.indexing.swap(true, Ordering::AcqRel) {
            // A second `fs.index` for the same project while the first is still walking.
            // Answering with the current status is right: the caller gets a live picker and
            // an `indexing: true` it can watch, rather than a second walk of the same tree.
            return fs.status();
        }
        // The flag is cleared by a guard rather than by a `store` at the end of the happy
        // path. A panic anywhere in the walk would otherwise leave it set for ever, and the
        // consequence is silent and total: every later `fs.index` takes the branch above and
        // returns immediately, so the project keeps an empty tree, an empty picker and no
        // watcher, with nothing anywhere saying why. Tauri catches the panic and the user
        // sees one failed command; a retry has to be able to work.
        let indexing_guard = IndexingGuard(Arc::clone(&fs));

        // A re-index of an already-indexed project starts from an empty picker, or the old
        // paths would be offered alongside the new ones for ever. The old watcher goes with
        // it: leaving it running would have it folding changes into an index that is about
        // to be thrown away, and its watch list is the one the previous walk produced.
        *fs.watcher.lock() = None;
        fs.matcher.clear();
        crate::emit::fs_status(app, project, &fs.status());

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
        drop(indexing_guard);

        let config = WatchConfig {
            roots: root_paths,
            dirs,
            ..WatchConfig::default()
        };
        let watcher = Watcher::start(config, filter, {
            let app = app.clone();
            let weak = Arc::downgrade(&fs);
            move |event| on_watch_event(&app, project, &weak, event)
        });
        *fs.watcher.lock() = Some(watcher);

        let status = fs.status();
        crate::emit::fs_status(app, project, &status);
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
        }
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| r.path.clone()).collect()
    }

    pub fn matcher(&self) -> &NucleoMatcher {
        &self.matcher
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
fn on_watch_event(app: &AppHandle, project: ProjectId, weak: &Weak<ProjectFs>, event: WatchEvent) {
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
            crate::emit::fs_changed(app, project, &change);
        }
        WatchEvent::Status(status) => {
            *fs.watch_status.lock() = status;
            crate::emit::fs_status(app, project, &fs.status());
        }
    }
}
