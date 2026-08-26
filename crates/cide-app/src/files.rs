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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use cide_fs::filter::FilterInput;
use cide_fs::{
    BuildOptions, Filter, Index, Root, Visibility, WalkItem, WatchConfig, WatchEvent, Watcher,
};
use cide_ipc::{FsChange, FsStatus, ProjectId, Settings, WatchBackend, WatchStatus};
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

    /// Files changed on disk, for whoever needs to know beyond the tree. (M18)
    ///
    /// Separate from [`Self::changed`], which carries an `FsChange` to the *webview*, because the
    /// consumer is in Rust and one of the two must not be reshaped for the other: this hands
    /// owned paths to `cide_app::lsp`, where a language server is told an agent rewrote a file it
    /// has findings for. Routing it through the webview instead was the alternative and it loses
    /// — a round trip per changed file, the frontend reading files it has no reason to hold, and
    /// a second copy of the "which server owns this extension" map that `ui/src/editor/docSync.ts`
    /// explicitly refuses to grow. The watcher already runs in Rust, next to the registry.
    ///
    /// **Defaulted to nothing, and only for the test doubles.** The reason this trait exists at
    /// all is that a walk can then be driven from an ordinary `#[test]` (see the note above), and
    /// those doubles are asserting on tree events rather than on language servers. The one
    /// implementation that matters is `AppHandle`'s below, and it is not defaulted.
    fn files_changed(&self, project: ProjectId, paths: &[PathBuf]) {
        let _ = (project, paths);
    }
}

impl FsEvents for AppHandle {
    fn status(&self, project: ProjectId, status: &FsStatus) {
        crate::emit::fs_status(self, project, status);
    }

    fn changed(&self, project: ProjectId, change: &FsChange) {
        crate::emit::fs_changed(self, project, change);
    }

    fn files_changed(&self, project: ProjectId, paths: &[PathBuf]) {
        use tauri::Manager as _;
        // `try_state` rather than `state`: this runs on a watcher thread that outlives nothing in
        // particular, and during shutdown the managed registry may already be gone. `state` panics
        // there, which would abort the process over an event nobody was waiting for.
        let Some(registry) = self.try_state::<crate::lsp::DiagnosticsRegistry>() else {
            return;
        };
        // A project with no language server has no entry, which is the ordinary case for a
        // TypeScript repository and is not an error.
        if let Some(diagnostics) = registry.get(project) {
            diagnostics.files_changed(paths);
        }
        // The second Rust-side consumer: `.cide/` is state this process mirrors (the task
        // tracker, the agent roster), and an agent's Write or a `git pull` moves it with no
        // Tauri command involved. See `dotcide`'s header for the routes and the worktrees trap.
        crate::dotcide::files_changed(self, project, paths);
    }
}

/// What the user's settings say the file tree walks.
///
/// The one place `cide_ipc::ExplorerSettings` becomes `cide_fs::Visibility`. Two types rather
/// than one because the crate that owns the walk must not depend on the wire's settings shape
/// and cannot see `cide-ipc`'s defaults — and one conversion function is what keeps the two from
/// drifting into a tree that shows what the watcher hides.
pub fn visibility_of(settings: &Settings) -> Visibility {
    Visibility {
        hidden: settings.explorer.show_hidden_files,
        ignored: settings.explorer.show_ignored_files,
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

    /// Every project with an entry here, with the roots it was walked over.
    ///
    /// The answer to "which projects would a settings change have to re-walk". Collected into a
    /// `Vec` rather than handing out an iterator over the map, because the caller starts a walk
    /// per entry and a walk takes the same map's lock through [`FsRegistry::claim`] — iterating
    /// a `DashMap` while a task re-enters it is a self-deadlock, and it is the kind that only
    /// shows up when a second project is open.
    pub fn indexed(&self) -> Vec<(ProjectId, Vec<PathBuf>)> {
        self.projects
            .iter()
            .map(|entry| (*entry.key(), entry.value().root_paths()))
            .collect()
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
    pub fn claim(
        &self,
        project: ProjectId,
        roots: Vec<PathBuf>,
        visibility: Visibility,
    ) -> Result<Indexing, FsStatus> {
        // A project can gain or lose a root between two indexings. Reusing the old entry
        // would then walk the old set for ever, with no error anywhere to say so — the tree
        // would simply be missing a root the user added.
        //
        // The comparison is done in a statement of its own: `DashMap::remove` while a `Ref`
        // into the same shard is alive is a self-deadlock, and letting the guard live to the
        // end of an `if let` block is exactly how that happens.
        //
        // `visibility` joins the root list in that comparison for exactly the same reason. An
        // index built with *Show ignored files* off does not contain `target/` — those entries
        // were never walked, so there is nothing to reveal — and an index built with it on
        // cannot have them removed without re-deriving the ignore verdict for every node it
        // holds. Either way the answer is a fresh walk, and treating a changed setting as
        // "already indexed" is how a toggle comes to do nothing at all.
        let stale = self
            .projects
            .get(&project)
            .is_some_and(|entry| entry.root_paths() != roots || entry.visibility != visibility);
        if stale {
            self.projects.remove(&project);
        }

        let entry = self.projects.entry(project).or_insert_with(|| {
            Arc::new(ProjectFs::new(
                roots.iter().cloned().map(Root::new).collect(),
                visibility,
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
            BuildOptions {
                // The user's setting, taken from the entry rather than read again here: the
                // whole entry was rebuilt if it moved, so this is the value `claim` decided to
                // walk with and the value `Filter::build` below is about to be given. Reading
                // the workspace a second time would open a window in which the walk and the
                // filter disagree.
                visibility: fs.visibility,
                ..BuildOptions::default()
            },
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
        // Every visited directory, so that every `.gitignore` under the roots is found — which
        // with *Show ignored files* on includes the ones inside `target/`.
        let dirs = index.dir_paths();
        let filter = Arc::new(Filter::build_with(FilterInput {
            roots: &root_paths,
            dirs: &dirs,
            git_dirs: &git_watch_dirs(&root_paths),
            visibility: fs.visibility,
        }));
        // NOT `dirs`. With ignored entries shown the tree holds thousands of directories the
        // watcher must not take a descriptor on — see `cide_fs::filter`'s module note, which
        // owns that trade — and `watch_dirs` is where the two lists are told apart.
        let watched = index.watch_dirs(&filter);

        *fs.index.write() = index;
        *fs.filter.write() = Arc::clone(&filter);
        drop(guard);

        let config = WatchConfig {
            roots: root_paths,
            dirs: watched,
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

/// Every git directory the watcher has to cover for these roots.
///
/// # Why this join lives in the app
///
/// `cide-fs` can resolve a root's `.git` on its own — `cide_fs::filter::git_dir` parses the
/// `gitdir:` line — and for an ordinary checkout that is the whole answer. It is half of one
/// for a linked worktree, whose state is split: `HEAD`, `index` and `ORIG_HEAD` sit in
/// `<common>/worktrees/<name>/`, while `refs/**` and `packed-refs` sit in the common
/// directory. Watching only the first means a commit in that worktree moves a ref nobody is
/// watching, so the git panel and the branch readout never hear about it — and cide's own
/// agent worktrees under `.claude/worktrees/` are exactly that shape.
///
/// Answering it properly needs libgit2, and `cide-fs` must not link it: the two crates walk
/// different things and are testable apart precisely because neither depends on the other.
/// The app depends on both already, and joining domain crates is what this layer is for — the
/// same reason the walk's sink is the picker's injector three functions above.
///
/// The cost is one `discover` per index. That is the cheapest git question there is (a
/// `Repository::discover` per root plus a submodule enumeration) and it runs on the blocking
/// worker that has just walked every inode in the tree.
///
/// **A `git init` performed after this runs stays uncovered until the next index.** There is
/// no repository to discover at this moment and no event that could tell us one appeared —
/// `.git` is pruned by the walk and refused by the filter. Re-indexing the project is the
/// recovery, and it is the same recovery as for any other change to the root set.
fn git_watch_dirs(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    // `discover` already deduplicates by canonical work tree, so two roots inside one
    // repository yield one entry — but a superproject and its submodules yield several, and a
    // linked worktree contributes a common directory that another root may contribute too.
    // `Filter::build_with` deduplicates as well; doing it here too costs nothing on a list
    // this short and keeps the value this function *returns* honest for any future caller.
    for repo in cide_git::repo::discover(roots) {
        match cide_git::repo::watch_dirs(&repo.root) {
            Ok(dirs) => {
                for dir in dirs {
                    if !out.contains(&dir) {
                        out.push(dir);
                    }
                }
            }
            // A root on an unmounted share, or a repository deleted between `discover` and
            // here. `discover` swallows the same class of error for the same reason: the
            // other roots still have working watchers and a project that opens is worth more
            // than an error page.
            Err(err) => {
                tracing::debug!(root = %repo.root.display(), %err, "no watchable git directory");
            }
        }
    }
    out
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
    /// What this project's index was walked with.
    ///
    /// Immutable for the life of the entry: [`FsRegistry::claim`] drops and rebuilds the whole
    /// entry when the user's setting moves, rather than mutating this and leaving an arena that
    /// disagrees with it. That is what makes [`ProjectFs::filter`] — handed to the content
    /// search and to `Index::apply` — provably the same answer the walk used.
    visibility: Visibility,
    index: RwLock<Index>,
    /// The synthetic groups drawn *after* the index's rows — *External Libraries*, and whatever
    /// comes next.
    ///
    /// A second tree beside the first rather than a synthetic root inside it. `cide_fs::groups`
    /// has the four couplings that decide it; the short version is that `Index::dir_paths()` is
    /// the watcher's watch list and the walk's sink is the picker's injector, so a dependency
    /// source grafted in would be watched and searched — 2,223 crate directories and 2.9 GB of
    /// `~/.cargo/registry` on this machine. Beside it, "never watched, never indexed" is true
    /// because there is no code that could do either.
    groups: crate::groups::ProjectGroups,
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

    // --- M12: symbols -------------------------------------------------------------------
    //
    // A **second** matcher rather than a second column on the first: `Ctrl+P` and
    // `Ctrl+Alt+Shift+N` answer different queries at the same time, and `Matcher::query` holds
    // one query per matcher. Sharing would make each keystroke in one overlay reset the other.
    /// The symbol picker's candidates. May lag [`Self::symbol_store`] — see `crate::symbols`.
    ///
    /// Lazy — built on first use — where the file matcher above is eager, and the difference
    /// is who fills it: the walk fills `matcher` the moment a project opens, while this one
    /// is documented empty until somebody opens the symbol picker. Each `NucleoMatcher` owns
    /// a rayon pool (`cide_search::MATCH_WORKERS` threads and their scoring slabs), so paying
    /// for it at project open bought threads that mostly idled for the life of the process.
    symbol_matcher: OnceLock<Arc<NucleoMatcher>>,
    /// The authoritative index the matcher is derived from.
    symbol_store: RwLock<crate::symbols::SymbolStore>,
    /// Candidates the matcher still holds that the store no longer backs.
    ///
    /// Drives the rebuild threshold. The matcher is append-only, so this only ever grows until a
    /// rebuild clears it.
    symbols_stale: AtomicU32,
    symbols_indexing: AtomicBool,
    /// A symbol walk has been started for this project at least once.
    ///
    /// What makes `symbol_query` answer `NotIndexed` — "nobody looked" — rather than an empty
    /// frame, and what stops a watcher burst from parsing files for a project whose symbol index
    /// nobody has ever asked for.
    symbols_started: AtomicBool,

    // --- M16: the library scope of the file picker ---------------------------------------
    //
    // A **third** matcher, for the reason the symbol one is a second, plus one that is specific
    // to this: nucleo is append-only. A user who turns *Search libraries* off cannot have 32,000
    // candidates taken back out, so a single matcher would mean the toggle only ever went one
    // way. Two matchers and `cide_search::merge` is what makes it a toggle at all.
    //
    // The four couplings `cide_fs::groups` lists are all still intact, and that is the point:
    // this touches `Index` nowhere. `dir_paths()` — the watcher's watch list — `Filter::build`,
    // `show_roots` and `path_of` are the project's alone. "Library sources are never watched"
    // stays true because there is still no code that could watch them.
    /// Candidates from the resolved dependency packages. Empty until somebody asks — and
    /// lazy for `symbol_matcher`'s reason: its pool should not exist until they do.
    library_matcher: OnceLock<Arc<NucleoMatcher>>,
    libraries_indexing: AtomicBool,
    /// A library walk has run to completion for this project.
    ///
    /// What makes [`Self::index_libraries`] idempotent, and what lets the picker tell "nobody
    /// asked yet" from "asked, and this project has no dependencies" — which are the same empty
    /// matcher and need different words on screen.
    libraries_walked: AtomicBool,
    /// How many packages the walk covered, for the empty-answer sentence.
    library_packages: AtomicU32,
}

impl ProjectFs {
    fn new(roots: Vec<Root>, visibility: Visibility) -> Self {
        Self {
            index: RwLock::new(Index::empty(roots.clone(), visibility)),
            // Empty, and it stays empty until the first tree read probes for a manifest. Opening
            // a project must cost nothing here — see `crate::libraries`.
            groups: crate::groups::ProjectGroups::new(),
            filter: RwLock::new(Arc::new(Filter::build(
                &roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
                Vec::new(),
                visibility,
            ))),
            roots,
            visibility,
            matcher: Arc::new(NucleoMatcher::new()),
            watcher: Mutex::new(None),
            watch_status: Mutex::new(WatchStatus {
                backend: WatchBackend::Native,
                reason: None,
                watched_dirs: 0,
            }),
            indexing: AtomicBool::new(false),
            walked: AtomicBool::new(false),
            symbol_matcher: OnceLock::new(),
            symbol_store: RwLock::new(crate::symbols::SymbolStore::new()),
            symbols_stale: AtomicU32::new(0),
            symbols_indexing: AtomicBool::new(false),
            symbols_started: AtomicBool::new(false),
            library_matcher: OnceLock::new(),
            libraries_indexing: AtomicBool::new(false),
            libraries_walked: AtomicBool::new(false),
            library_packages: AtomicU32::new(0),
        }
    }

    // --- M12: symbols ---------------------------------------------------------------------

    /// The matcher, built on first use. Construction takes no other lock, so it cannot
    /// interleave with the lock-order rule `NucleoMatcher` documents.
    fn symbol_matcher_arc(&self) -> &Arc<NucleoMatcher> {
        self.symbol_matcher
            .get_or_init(|| Arc::new(NucleoMatcher::new()))
    }

    pub fn symbol_matcher(&self) -> &NucleoMatcher {
        self.symbol_matcher_arc()
    }

    pub fn symbol_store(&self) -> &RwLock<crate::symbols::SymbolStore> {
        &self.symbol_store
    }

    pub fn is_symbol_indexing(&self) -> bool {
        self.symbols_indexing.load(Ordering::Acquire)
    }

    /// Has a symbol walk ever been started for this project?
    ///
    /// What separates "nobody looked" from "looked and found nothing" — `symbol_query` answers
    /// [`cide_lang::SymbolError::NotIndexed`] while this is false, so the overlay shows
    /// *Indexing…* rather than an empty list over a repository full of functions.
    pub fn symbols_started(&self) -> bool {
        self.symbols_started.load(Ordering::Acquire)
    }

    /// Walk this project and build its symbol index. Blocking; idempotent.
    ///
    /// A second call while a walk is running, or after one finished, reports the index that
    /// exists rather than starting a second walk over the same tree — the same rule
    /// [`FsRegistry::claim`] applies to the file index, and for the same reason: re-walking would
    /// clear the matcher the first caller's overlay is reading from.
    pub fn index_symbols(&self) -> cide_ipc::SymbolIndexStatus {
        if self.symbols_indexing.swap(true, Ordering::AcqRel) || self.symbols_started() {
            return self.symbol_status();
        }
        // Cleared by a guard, so a panic in the walk cannot leave the flag set for ever — which
        // would make every later call take the branch above and report an empty index as final.
        struct Guard<'a>(&'a AtomicBool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = Guard(&self.symbols_indexing);

        let roots: Vec<cide_lang::WalkRoot> = self
            .roots
            .iter()
            .map(|root| cide_lang::WalkRoot {
                path: root.path.clone(),
                label: root
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned()),
            })
            .collect();
        // The same ignore decision the file tree and the content search use, injected rather than
        // recomputed — three implementations of "is this file ignored" is how a picker starts
        // offering rows the explorer does not show.
        let filter = self.filter();
        let admits = |path: &std::path::Path, is_dir: bool| filter.admits(path, is_dir);
        let cancel = std::sync::atomic::AtomicBool::new(false);

        cide_lang::walk_symbols(
            &roots,
            cide_lang::Limits::default(),
            Some(&admits),
            &cancel,
            &mut |file| {
                let (candidates, stale) = self.symbol_store.write().insert(file);
                for candidate in candidates {
                    self.symbol_matcher().push(candidate);
                }
                if stale > 0 {
                    self.symbols_stale.fetch_add(stale, Ordering::Relaxed);
                }
            },
        );

        self.symbols_started.store(true, Ordering::Release);
        self.symbol_status()
    }

    // --- M16: the library scope of the file picker ------------------------------------------

    /// Built on first use, like [`Self::symbol_matcher_arc`].
    fn library_matcher_arc(&self) -> &Arc<NucleoMatcher> {
        self.library_matcher
            .get_or_init(|| Arc::new(NucleoMatcher::new()))
    }

    pub fn library_matcher(&self) -> &NucleoMatcher {
        self.library_matcher_arc()
    }

    pub fn is_indexing_libraries(&self) -> bool {
        self.libraries_indexing.load(Ordering::Acquire)
    }

    /// Has a library walk finished for this project?
    ///
    /// What separates "nobody asked" from "asked, and this project depends on nothing" — the
    /// same empty matcher, and two different sentences on screen.
    pub fn libraries_walked(&self) -> bool {
        self.libraries_walked.load(Ordering::Acquire)
    }

    /// How many packages the last walk covered. Zero before one has run.
    pub fn library_packages(&self) -> u32 {
        self.library_packages.load(Ordering::Acquire)
    }

    /// Walk this project's resolved dependency packages into the library matcher.
    ///
    /// **Blocking, and idempotent.** Call it from `spawn_blocking`; `cmd::picker` is the only
    /// caller in the app and does.
    ///
    /// # What is indexed, and what deliberately is not
    ///
    /// Only the packages *this project resolves* — `cargo metadata --frozen`'s dependency graph
    /// plus the SDK row — and never a dependency cache. Measured on this repository: **593
    /// packages, 31,865 files**, walked in ~130 ms warm. The whole of `~/.cargo/registry/src` on
    /// the same machine is 127,055 files and `~/go/pkg/mod` is 347,777, neither of which is
    /// bounded by anything about the project the user has open. That distinction is the feature:
    /// "index the libraries" and "index everything this machine has ever built" differ by two
    /// orders of magnitude, and only the first is a picker.
    ///
    /// **No watcher, and no `Filter` rebuild.** Library sources are read-only and immutable —
    /// a registry crate at a version does not change — so there is nothing to watch, and adding
    /// 6,586 directories to inotify to learn that would cost more than the walk. The staleness
    /// that does happen is `cargo add`/`cargo update`, which the lockfile stamp already notices
    /// (`libraries::stale`) and which invalidates the group; [`Self::forget_libraries`] is what
    /// that path calls.
    ///
    /// # Why it can block on `cargo metadata`, and why that is affordable
    ///
    /// A user who has never opened *External Libraries* has no resolved packages, so this asks
    /// `ProjectGroups::resolve_now` for them — the same blocking resolution `fs_reveal` already
    /// performs for *Select Opened File*, on the same argument: the gesture explicitly asked for
    /// this, it happens at most once per project per process, and the alternative is not
    /// "faster", it is an empty answer.
    ///
    /// The user sees it happen. `picker_index_libraries` returns immediately after claiming the
    /// walk and the overlay polls with `running: true` while the counter climbs — the same shape
    /// `symbols_index` and `symbol_query` have shipped with since M12.
    pub fn index_libraries(&self) -> bool {
        /*
         * `libraries_walked` is checked BEFORE the claim, and that order is the whole
         * correctness of this pair.
         *
         * It read `swap(true, …) || libraries_walked()`. `swap` STORES unconditionally and
         * returns the old value, so once a walk had finished the second operand took the branch
         * — after the first had already set the flag, and before the clearing guard below was
         * constructed. Every later call therefore latched `libraries_indexing` to true and left
         * it there for the life of the process, and the picker's overlay polls that flag: the
         * scope reported *indexing* for ever, on a project whose libraries were already indexed.
         * `FilePicker` re-issues this on every mount while the scope is on, so reopening Ctrl+P
         * once was enough.
         *
         * Asking the cheap, idempotent question first also makes the claim mean what it says:
         * whoever wins the swap is the one doing a walk, and is holding the guard.
         */
        if self.libraries_walked() {
            return false;
        }
        if self.libraries_indexing.swap(true, Ordering::AcqRel) {
            return false;
        }
        // Cleared by a guard, so a panic in the walk cannot leave the flag set for ever — which
        // would make every later call take the branch above and report a half-filled matcher as
        // final. The same shape as `IndexingGuard` and `index_symbols`'s, and for the same
        // reason: the consequence of getting it wrong is silent and permanent.
        struct Guard<'a>(&'a AtomicBool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = Guard(&self.libraries_indexing);

        // Resolve if nobody has. `resolve_now` claims, fills and answers false when somebody
        // else is already resolving — in which case the entries below are whatever is there,
        // and the walk simply covers fewer packages than it might have. A `Resolving` group is
        // a group somebody is about to fulfil, and `libraries_walked` stays false, so the next
        // press of the toggle picks up the rest.
        let roots = self.root_paths();
        if self.groups.entries(crate::libraries::GROUP_ID).is_empty() {
            self.groups.resolve_now(&roots);
        }

        // A note row — *"Standard library sources are not installed"*, or a Go module the cache
        // does not hold — has no directory and contributes nothing. `dir` is what says the path
        // is real; `Entry::note` sets neither.
        let packages: Vec<(String, PathBuf)> = self
            .groups
            .entries(crate::libraries::GROUP_ID)
            .into_iter()
            .filter_map(|entry| {
                let path = entry.path?;
                // The name *and* the version, which is `detail` — `serde 1.0.229`. Without the
                // version two rows from two builds of the same crate are indistinguishable,
                // which is the failure the chip exists for. `detail` is also where a per-row
                // note lands (`v1.1.1 · not downloaded`), and that reads correctly in a chip.
                let label = match entry.detail {
                    Some(detail) if !detail.is_empty() => format!("{} {detail}", entry.name),
                    _ => entry.name.clone(),
                };
                Some((label, path))
            })
            .collect();

        // The package's *directory name* as the walk's label, not the display label: it is what
        // becomes the `rel` prefix, so a row reads `serde-1.0.229/src/de/mod.rs` and a user can
        // narrow to one crate by typing its name. The display label goes on the candidate's
        // `source` instead, where it is drawn and not matched.
        let roots: Vec<Root> = packages
            .iter()
            .map(|(_, path)| Root::new(path.clone()))
            .collect();
        let sources: Vec<String> = packages.iter().map(|(label, _)| label.clone()).collect();

        self.library_packages
            .store(roots.len() as u32, Ordering::Release);

        let matcher = Arc::clone(self.library_matcher_arc());
        Index::walk_roots(
            &roots,
            // `threads: 1`, and the default is the trap. See `Index::walk_roots`: 593 roots at
            // the default `threads: 0` costs 1.24 s against 130 ms here, all of it in spawning
            // and joining 32 walker threads per package directory.
            BuildOptions {
                threads: 1,
                ..BuildOptions::default()
            },
            true,
            &move |batch: &[WalkItem]| {
                for item in batch.iter().filter(|i| !i.is_dir) {
                    // `item.root` is the index into `roots`, which is parallel to `sources` by
                    // construction. A miss is impossible and is treated as "no chip" rather than
                    // as a panic: a row with no provenance is a degraded row, and a panic here
                    // would poison the guard above and disable the feature for the process.
                    let source = sources.get(item.root as usize).cloned();
                    let value = item.path.to_string_lossy().into_owned();
                    matcher.push(match source {
                        Some(source) => Candidate::from_source(item.rel.clone(), value, source),
                        None => Candidate::new(item.rel.clone(), value),
                    });
                }
            },
        );

        /*
         * Last, and only on this path — the same rule `Indexing::run` states about `walked`. It
         * means "this project's libraries have been walked", and setting it earlier would make a
         * walk that panicked permanently unrepeatable with no way to ask again.
         *
         * **And only when there was something to walk.** The comment above says a `Resolving`
         * group leaves `libraries_walked` false so the next press picks up the rest — that was
         * the intent and not the behaviour: the store ran unconditionally. So a user who expanded
         * *External Libraries* in the tree (which forks `cargo metadata` on its own thread) and
         * pressed the picker's toggle inside that window got zero packages, `resolve_now`
         * refusing because somebody else held the claim, and the flag latched anyway — the
         * library scope was then permanently empty for that project, with no gesture that could
         * ask again. Which is the exact opposite of what the paragraph above promises.
         *
         * Zero packages is not always transient: a Go-only project, or a Rust one whose
         * `rust-src` component is absent, legitimately resolves to nothing. Those cases are not
         * distinguished here on purpose — a repeated walk of an empty set costs one `entries()`
         * call, while a wrongly-latched flag costs the feature.
         */
        if !roots.is_empty() {
            self.libraries_walked.store(true, Ordering::Release);
        }
        true
    }

    /// Drop the library candidates and allow a fresh walk.
    ///
    /// Called when the lockfile stamp says the resolution is stale — `cargo add`, `cargo
    /// update` — because the packages the matcher holds are then a set that no longer describes
    /// this project. `NucleoMatcher::clear` replaces the injector under its own lock, so a walk
    /// still pushing into the old one is filling a queue nobody reads rather than corrupting the
    /// new one.
    ///
    /// It does **not** re-walk. The user asked for libraries once; whether they still want them
    /// is answered by the toggle being on, and the next query re-walks through the ordinary lazy
    /// path. Re-walking here would put a 130 ms walk on the tail of a `cargo build` in a
    /// terminal pane.
    pub fn forget_libraries(&self) {
        if !self.libraries_walked.swap(false, Ordering::AcqRel) {
            return;
        }
        self.library_matcher().clear();
        self.library_packages.store(0, Ordering::Release);
    }

    /// Re-parse a handful of files a watcher burst touched.
    ///
    /// Only ever called when [`Self::symbols_started`] — a project nobody has asked for symbols in
    /// must not start parsing because somebody ran `cargo build`.
    pub fn refresh_symbols(&self, paths: &[PathBuf]) {
        let filter = self.filter();
        for path in paths {
            if cide_lang::Lang::of_path(path).is_none() {
                continue;
            }
            if !path.is_file() {
                // Deleted, or moved out. Forgetting it is what stops the picker offering rows for
                // a file that is gone.
                let stale = self.symbol_store.write().remove(&path.to_string_lossy());
                self.symbols_stale.fetch_add(stale, Ordering::Relaxed);
                continue;
            }
            if !filter.admits(path, false) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let Some(lang) = cide_lang::Lang::of_path(path) else {
                continue;
            };
            let Some((symbols, _)) =
                cide_lang::outline_symbols(lang, &text, cide_lang::Limits::default())
            else {
                continue;
            };
            let rel = self
                .roots
                .iter()
                .find_map(|root| path.strip_prefix(&root.path).ok())
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            let (candidates, stale) = self.symbol_store.write().insert(cide_lang::WalkedFile {
                path: path.to_string_lossy().into_owned(),
                rel,
                lang,
                symbols,
            });
            for candidate in candidates {
                self.symbol_matcher().push(candidate);
            }
            self.symbols_stale.fetch_add(stale, Ordering::Relaxed);
        }
        self.rebuild_symbols_if_stale();
    }

    /// Rebuild the matcher from the store once enough of it is dead weight.
    ///
    /// The only thing that ever actually removes a stale candidate — the matcher is append-only,
    /// so `resolve` hides them and this is what reclaims them. A tenth of the index, floored at
    /// 2,000, so a small project does not rebuild on every save and a large one does not carry an
    /// unbounded tail through a long editing session.
    fn rebuild_symbols_if_stale(&self) {
        let total = self.symbol_store.read().symbols();
        let threshold = (total / 10).max(2_000);
        if self.symbols_stale.load(Ordering::Acquire) < threshold {
            return;
        }
        // `clear` then one `extend`: `NucleoMatcher::clear` replaces the injector under the same
        // lock precisely so a concurrent pusher cannot fill a queue nobody reads. Same sequence
        // `Indexing::run` uses for the file matcher.
        self.symbol_matcher().clear();
        let candidates = self.symbol_store.read().candidates();
        self.symbol_matcher().extend(&mut candidates.into_iter());
        self.symbols_stale.store(0, Ordering::Release);
    }

    fn symbol_status(&self) -> cide_ipc::SymbolIndexStatus {
        let store = self.symbol_store.read();
        cide_ipc::SymbolIndexStatus {
            indexing: self.is_symbol_indexing(),
            files: store.files(),
            symbols: store.symbols(),
            truncated: store.truncated,
        }
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| r.path.clone()).collect()
    }

    pub fn matcher(&self) -> &NucleoMatcher {
        &self.matcher
    }

    /// The ignore rules this project's tree and watcher consult.
    ///
    /// Shared as an `Arc` rather than cloned: [`Filter::build`] costs a `stat` per visited
    /// directory — a few thousand on a large repository — and the content search was paying
    /// that on every query, rebuilding an identical `Filter` from the same `dir_paths()` this
    /// one was built from. Cloning the matchers instead would have kept both the second copy
    /// and the cost; the point of the accessor is that the ignore decision is made once.
    ///
    /// A caller that takes this mid-walk gets the pre-walk filter, which is the one the tree
    /// and the watcher are using at that moment: [`Indexing::run`] installs the finished one
    /// before it starts watching, so the two can never disagree about which files exist.
    pub fn filter(&self) -> Arc<Filter> {
        Arc::clone(&self.filter.read())
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

    /// This project's synthetic groups. See the field.
    pub fn groups(&self) -> &crate::groups::ProjectGroups {
        &self.groups
    }

    /// Every directory the disk-changing `fs_*` handlers may act inside.
    ///
    /// The roots, **plus** the scratch drawer. Deliberately a second list rather than a wider
    /// [`Self::root_paths`]: that one means *project roots* to `relativeTo`, to `isRootPath`,
    /// to `Filter::build` and to `pasteTargetFor`, and widening it would make a scratch file
    /// appear to live in the project — with a relative path in the status trail and a row the
    /// paste target could land in.
    ///
    /// *External Libraries* contributes nothing here, and that is the point of asking the
    /// groups rather than hard-coding the drawer: a rename inside `~/.cargo/registry` would
    /// break every project on the machine that depends on the crate, so that group answers
    /// with no writable directory at all.
    pub fn writable_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.root_paths();
        paths.extend(self.groups.writable_dirs());
        paths
    }

    /// Every path the file tree can hang a row from: the roots, plus each group's top-level
    /// children.
    ///
    /// The **containment** question, not the writability one, and the two lists are genuinely
    /// different in both directions. A dependency source is revealable and not writable (that is
    /// the whole of `writable_dirs`' comment); a scratch is both; a project root is both. So this
    /// is a third list rather than a widening of either, for the same reason `writable_paths` is
    /// not a widened [`Self::root_paths`] — one list meaning two things is how a menu comes to
    /// offer a verb the handler refuses.
    ///
    /// What asks: the status bar's path trail, which makes a segment clickable exactly when some
    /// entry here contains it. See [`cide_fs::groups::Groups::reveal_roots`] for why the
    /// frontend cannot derive the group half itself.
    pub fn reveal_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.root_paths();
        paths.extend(self.groups.reveal_roots());
        paths
    }

    pub fn status(&self) -> FsStatus {
        let index = self.index.read();
        FsStatus {
            indexing: self.indexing.load(Ordering::Acquire),
            files: index.files(),
            dirs: index.dirs(),
            // The **composed** count, matching `fs_tree_count`. `files` and `dirs` deliberately
            // do not move: they describe the walk, and a dependency source was never walked.
            // `rows` is the scroller's range, so it has to include the group rows or a resolution
            // that finished would leave rows the virtualizer refuses to ask for.
            rows: (index.count() + self.groups.count()) as u32,
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
            // Keep the symbol index in step — but only for a project that has one. A project
            // nobody has pressed Ctrl+Alt+Shift+N in must not start parsing because somebody ran
            // `cargo build`.
            if fs.symbols_started() {
                let touched: Vec<PathBuf> = change
                    .paths
                    .iter()
                    .map(PathBuf::from)
                    .filter(|p| cide_lang::Lang::of_path(p).is_some())
                    .collect();
                if !touched.is_empty() {
                    // On a blocking worker, never here. This callback runs on the watcher thread
                    // and also carries the file-tree updates; a `cargo fmt` over 400 files is 400
                    // parses, and doing them inline would stall every later burst behind them.
                    let fs = Arc::clone(&fs);
                    tauri::async_runtime::spawn_blocking(move || fs.refresh_symbols(&touched));
                }
            }
            /*
             * The language servers, before the webview is told anything. (M18)
             *
             * Order is not load-bearing — these are two independent consumers — but the *call* is:
             * without it a file an agent rewrote reaches the tree, the picker and `cide-lang`, and
             * stops there. rust-analyzer and gopls learn nothing, their diagnostics keep the line
             * numbers they were published with, and clicking one lands wherever that line now is.
             * That is the reported bug, and this line is the fix for its first half.
             *
             * Unfiltered by language on purpose: `ProjectDiagnostics::files_changed` owns the
             * question of which paths matter to which server — including the build manifests,
             * which `cide_lang::Lang::of_path` above deliberately does not know about.
             */
            events.files_changed(project, &change.paths);
            events.changed(project, &change);
        }
        WatchEvent::Status(status) => {
            *fs.watch_status.lock() = status;
            events.status(project, &fs.status());
        }
    }
}
