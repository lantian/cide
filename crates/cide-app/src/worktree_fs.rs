//! The Explorer's view of one agent checkout under `.cide/worktrees/`. (M207)
//!
//! The project's own index cannot show a checkout. `.cide/worktrees` is in the default
//! `explorer.indexExcludedFolders`, so the walk records it as a browse node — paged, never
//! watched, never in the picker — and that is right for the *project's* tree: a checkout is a
//! second copy of the whole repository, and folding it into the index would double the picker,
//! the watch list and the memory a project costs for every agent that ever ran. The Git panel's
//! worktree selector (`chrome/WorktreeSelect.tsx`) re-points the changes tree at a checkout; this
//! is the same gesture for the file tree, and it needs a tree to re-point at.
//!
//! # A second, smaller index rather than a re-rooted `ProjectFs`
//!
//! [`WorktreeFs`] is an `Index`, a `Filter` and a `Watcher` over the checkout and nothing
//! else. No file matcher (Ctrl+P stays the project's — a checkout's files are the project's files
//! at another commit, and offering both would put every path in the picker twice), no symbols, no
//! synthetic groups (*External Libraries* and *Scratches* describe the project, not a checkout),
//! and no language-server or `.cide/` notifications: `dotcide::classify` is anchored to the
//! project root and routes a checkout's own `.cide/tasks.json` to nothing, and the language
//! servers were started for the project's roots. A `ProjectFs` per checkout was the alternative
//! and it loses on exactly those parts: its constructor reserves 32 MiB and builds a matcher with
//! its own thread pool before anything is walked.
//!
//! # The exclusions are compiled again, against the checkout
//!
//! `Exclusions::compile` matches every rule relative to the deepest root holding a path and never
//! matches a root itself. Handing this walk the *project's* compiled policy would make the checkout
//! — which sits at `<root>/.cide/worktrees/<name>` — an excluded folder of the project root, and
//! the walk would stop at its first directory: an empty tree, with no error anywhere. Compiled
//! against `[checkout]`, the same rules exclude only the checkout's own nested `.cide/worktrees`
//! and `.cide/tasks`, which is what they mean there too.
//!
//! # Who owns a view, and how long it lives
//!
//! The project's [`ProjectFs`] holds at most [`MAX_VIEWS`], most recently used first. There is no
//! open or close command: every tree command that names a worktree goes through [`view`], which
//! answers the view it has or builds one. That is what makes the lifetime self-healing — a view
//! evicted by the cap, dropped with a `ProjectFs` that a settings change replaced, or lost to a
//! checkout Integrate removed, is simply rebuilt (or refused) by the next call, and the Explorer
//! re-asks on the untagged `cide://fs-status` a re-walk emits. Two, because the choice of worktree
//! is per window and two windows can disagree; a third window's choice costs a re-walk when the
//! first comes back, which is a second of disk, not a wrong answer.
//!
//! Dropping the `ProjectFs` drops its views and their watchers — `fs_close` does that on a
//! blocking worker for the project and so, by ownership, for its checkouts.
//!
//! # The trust boundary
//!
//! The path arrives from the webview. [`cide_git::worktree::resolve`] is the gate every other
//! worktree-scoped command already uses: only a checkout cide made under one of this project's
//! roots answers. Anything else — a user's own `git worktree add`, `/`, another project's
//! checkout — is refused as [`FsError::OutsideProject`], before a single directory is read.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use cide_fs::budget::Budget;
use cide_fs::filter::FilterInput;
use cide_fs::{
    BuildOptions, Exclusions, Filter, FsError, Index, Root, Visibility, WatchConfig, WatchEvent,
    Watcher,
};
use cide_ipc::{FsChange, FsStatus, ProjectId, TreeRow, WatchBackend, WatchStatus};
use parking_lot::{Mutex, RwLock};

use crate::files::{FsEvents, ProjectFs};

/// How many checkouts one project keeps an index for at once. See the module note.
pub const MAX_VIEWS: usize = 2;

/// One checkout's tree: an index, the filter it was walked with, and a watcher.
pub struct WorktreeFs {
    /// The checkout, canonical — exactly what `cide_git::worktree::resolve` answered, which is
    /// also `WorktreeInfo.repo.root` on the wire. The frontend's value is therefore the key, and
    /// every row this index serves is spelled under it.
    root: PathBuf,
    generation: u64,
    /// A sibling of the project's budget: counted against the same global limit, so a checkout
    /// cannot take the memory the setting promised the projects.
    budget: Arc<Budget>,
    visibility: Visibility,
    exclusions: Arc<Exclusions>,
    index: RwLock<Index>,
    filter: RwLock<Arc<Filter>>,
    /// `Option` for the reason `ProjectFs::watcher` is one: it exists only once the walk has
    /// produced a directory list, and dropping it is how watching stops.
    watcher: Mutex<Option<Watcher>>,
    watch_status: Mutex<WatchStatus>,
    walking: AtomicBool,
}

impl WorktreeFs {
    fn new(
        root: PathBuf,
        visibility: Visibility,
        rules: &[String],
        budget: Arc<Budget>,
    ) -> Result<Self, FsError> {
        static GENERATION: AtomicU64 = AtomicU64::new(1);
        let exclusions =
            Arc::new(Exclusions::compile(vec![root.clone()], rules).map_err(FsError::InvalidPath)?);
        let roots = vec![Root::new(root.clone())];
        Ok(Self {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            index: RwLock::new(Index::empty_controlled(
                roots,
                visibility,
                Arc::clone(&budget),
                Arc::clone(&exclusions),
            )),
            filter: RwLock::new(Arc::new(
                Filter::build(std::slice::from_ref(&root), Vec::new(), visibility)
                    .with_exclusions(Arc::clone(&exclusions)),
            )),
            root,
            budget,
            visibility,
            exclusions,
            watcher: Mutex::new(None),
            watch_status: Mutex::new(WatchStatus {
                backend: WatchBackend::Native,
                reason: None,
                watched_dirs: 0,
            }),
            walking: AtomicBool::new(true),
        })
    }

    /// The checkout this view shows.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Does this checkout hold `path`? A plain prefix test — both sides are spelled under the
    /// canonical root, since every path the tree hands back came from this index.
    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }

    pub fn with_index<T>(&self, f: impl FnOnce(&Index) -> T) -> T {
        f(&self.index.read())
    }

    pub fn with_index_mut<T>(&self, f: impl FnOnce(&mut Index) -> T) -> T {
        f(&mut self.index.write())
    }

    pub fn filter(&self) -> Arc<Filter> {
        Arc::clone(&self.filter.read())
    }

    pub fn count(&self) -> usize {
        self.with_index(|index| index.count())
    }

    pub fn rows(&self, offset: usize, len: usize) -> Vec<TreeRow> {
        self.with_index(|index| index.rows(offset, len))
    }

    pub fn status(&self) -> FsStatus {
        let index = self.index.read();
        FsStatus {
            generation: self.generation,
            index_bytes: self.budget.used(),
            index_limit: self.budget.reason(),
            indexing: self.walking.load(Ordering::Acquire),
            files: index.files(),
            dirs: index.dirs(),
            rows: index.count() as u32,
            watch: self.watch_status.lock().clone(),
        }
    }

    /// Fold a change cide made itself into this index — the same `Index::apply` the watcher
    /// would run a few hundred milliseconds later, so a *New File…* in the checkout's tree shows
    /// its row before the command returns. Only the paths this checkout holds are applied;
    /// `apply` rescans each path's parent, and a parent outside the root is not this index's.
    pub fn fold(&self, change: &FsChange) {
        let paths: Vec<PathBuf> = change
            .paths
            .iter()
            .filter(|path| self.contains(path))
            .cloned()
            .collect();
        if paths.is_empty() {
            return;
        }
        let mine = FsChange {
            paths,
            truncated: change.truncated,
            git: change.git,
        };
        let filter = self.filter();
        let _ = self.with_index_mut(|index| index.apply(&mine, &filter));
    }

    /// Stop the watcher and refuse further work. Called when the cap evicts this view and when
    /// the project's own entry is cancelled.
    pub(crate) fn cancel(&self) {
        self.budget.cancel();
        let watcher = self.watcher.lock().take();
        drop(watcher);
    }

    /// Walk the checkout and start watching it. **Blocking** — the caller is a tree command
    /// already on the blocking pool.
    ///
    /// The shape of `Indexing::run` without the picker's sink: the walk's batches go nowhere,
    /// which is what keeps a checkout out of Ctrl+P by construction rather than by a filter.
    fn walk(self: &Arc<Self>, events: &Arc<dyn FsEvents>, project: ProjectId) {
        let roots = vec![self.root.clone()];
        let index = Index::build_controlled(
            vec![Root::new(self.root.clone())],
            BuildOptions {
                visibility: self.visibility,
                exclusions: Arc::clone(&self.exclusions),
                ..BuildOptions::default()
            },
            Arc::clone(&self.budget),
            &|_| {},
        );
        if self.budget.cancelled() {
            return;
        }
        let dirs = index.dir_paths();
        let filter = Arc::new(
            Filter::build_controlled(
                FilterInput {
                    roots: &roots,
                    dirs: &dirs,
                    // The checkout's own admin directory (`<common>/worktrees/<name>`, its `HEAD`
                    // and `index`) and the common one (`refs/**`): a commit or a `git add` in the
                    // checkout moves one of them, and that burst is what re-tags the rows.
                    git_dirs: &crate::files::git_watch_dirs(&roots),
                    visibility: self.visibility,
                },
                &|| self.budget.cancelled(),
            )
            .with_exclusions(Arc::clone(&self.exclusions)),
        );
        let watched = index.watch_dirs(&filter);
        let links = index.watch_links(&filter);
        if self.budget.cancelled() {
            return;
        }
        *self.index.write() = index;
        *self.filter.write() = Arc::clone(&filter);

        let config = WatchConfig {
            budget: Arc::clone(&self.budget),
            files: self.with_index(|index| index.file_paths()),
            roots,
            dirs: watched,
            links,
            ..WatchConfig::default()
        };
        // A `Weak` for `files.rs`'s reason: the view owns the watcher, and an `Arc` in its
        // closure would be a cycle that kept a dropped checkout's inotify descriptors for ever.
        let watcher = Watcher::start(config, filter, {
            let events = Arc::clone(events);
            let weak = Arc::downgrade(self);
            move |event| on_watch_event(&events, project, &weak, event)
        });
        {
            let mut slot = self.watcher.lock();
            if self.budget.cancelled() {
                drop(slot);
                drop(watcher);
                return;
            }
            *slot = Some(watcher);
        }
        self.walking.store(false, Ordering::Release);
        events.view_status(project, &self.root, &self.status());
    }
}

/// Fold one watcher event into the view and tell the webview, tagged with the checkout.
fn on_watch_event(
    events: &Arc<dyn FsEvents>,
    project: ProjectId,
    weak: &Weak<WorktreeFs>,
    event: WatchEvent,
) {
    let Some(view) = weak.upgrade() else {
        return;
    };
    if view.budget.cancelled() {
        return;
    }
    match event {
        WatchEvent::Changed(change) => {
            let filter = view.filter();
            let _ = view.with_index_mut(|index| index.apply(&change, &filter));
            if view.budget.cancelled() {
                return;
            }
            events.view_status(project, &view.root, &view.status());
            events.view_changed(project, &view.root, &change);
        }
        WatchEvent::Status(status) => {
            *view.watch_status.lock() = status;
            events.view_status(project, &view.root, &view.status());
        }
    }
}

/// The view of `path` for this project — the one already held, or a new one walked here.
///
/// **Blocking** when it walks: the first call naming a checkout returns once the checkout has been
/// read, so the tree's first `fs_tree_count` answers with the whole tree rather than zero and a
/// re-read a second later. A second call racing the first gets the view mid-walk (zero rows,
/// `indexing: true`) and the walk's end reaches it as a tagged `cide://fs-status`.
pub fn view(
    fs: &Arc<ProjectFs>,
    events: &Arc<dyn FsEvents>,
    project: ProjectId,
    path: &Path,
) -> Result<Arc<WorktreeFs>, FsError> {
    let refused = || FsError::OutsideProject(path.display().to_string());
    if let Some(held) = fs.touch_view(path) {
        // One `stat` per tree call. Integrate removes a checkout it merged, and a view of a
        // directory that is gone would keep serving the rows of a tree nobody can open.
        if held.root.is_dir() {
            return Ok(held);
        }
        fs.forget_view(&held.root);
        return Err(refused());
    }
    let root = cide_git::worktree::resolve(&fs.root_paths(), path).ok_or_else(refused)?;
    let visibility = fs.with_index(|index| index.visibility());
    let rules = fs.exclusions.rules().to_vec();
    let budget = fs.budget.sibling();
    // Built over the canonical spelling, never the caller's: the exclusions are compiled against
    // this root, and a symlinked spelling would leave every rule matching nothing.
    let key = root.clone();
    let (view, fresh) = fs.adopt_view(key, || WorktreeFs::new(root, visibility, &rules, budget))?;
    if fresh {
        events.view_status(project, &view.root, &view.status());
        view.walk(events, project);
    }
    Ok(view)
}

/// The views `ProjectFs` holds, most recently used first. Kept here so the cap's arithmetic is
/// next to the note that explains it.
#[derive(Default)]
pub struct Views(Mutex<Vec<Arc<WorktreeFs>>>);

impl Views {
    pub fn touch(&self, path: &Path) -> Option<Arc<WorktreeFs>> {
        let mut held = self.0.lock();
        let at = held.iter().position(|view| view.root == path)?;
        let view = held.remove(at);
        held.insert(0, Arc::clone(&view));
        Some(view)
    }

    /// Insert the view for `root` unless one is already there; answer it and whether it is new.
    ///
    /// `make` runs under the lock, and is cheap — an empty index and a filter over no
    /// directories. The walk happens after, outside it.
    pub fn adopt(
        &self,
        root: PathBuf,
        make: impl FnOnce() -> Result<WorktreeFs, FsError>,
    ) -> Result<(Arc<WorktreeFs>, bool), FsError> {
        let evicted;
        let answer;
        {
            let mut held = self.0.lock();
            if let Some(at) = held.iter().position(|view| view.root == root) {
                let view = held.remove(at);
                held.insert(0, Arc::clone(&view));
                return Ok((view, false));
            }
            let view = Arc::new(make()?);
            debug_assert_eq!(
                view.root, root,
                "a view is keyed by the root it was built over"
            );
            held.insert(0, Arc::clone(&view));
            evicted = if held.len() > MAX_VIEWS {
                held.split_off(MAX_VIEWS)
            } else {
                Vec::new()
            };
            answer = (view, true);
        }
        // Outside the lock: cancelling joins a watcher thread.
        for view in evicted {
            view.cancel();
        }
        Ok(answer)
    }

    pub fn forget(&self, root: &Path) {
        let gone: Vec<_> = {
            let mut held = self.0.lock();
            let (gone, kept) = std::mem::take(&mut *held)
                .into_iter()
                .partition(|view: &Arc<WorktreeFs>| view.root == root);
            *held = kept;
            gone
        };
        for view in gone {
            view.cancel();
        }
    }

    pub fn all(&self) -> Vec<Arc<WorktreeFs>> {
        self.0.lock().clone()
    }

    pub fn cancel_all(&self) {
        let all = std::mem::take(&mut *self.0.lock());
        for view in all {
            view.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::view as open;
    use super::*;
    use crate::files::FsRegistry;
    use std::sync::atomic::AtomicU32;

    /// Counts the tagged events, and remembers which checkout each was tagged with.
    #[derive(Default)]
    struct Tagged {
        untagged: AtomicU32,
        tags: Mutex<Vec<PathBuf>>,
    }

    impl FsEvents for Tagged {
        fn status(&self, _: ProjectId, _: &FsStatus) {
            self.untagged.fetch_add(1, Ordering::Relaxed);
        }
        fn changed(&self, _: ProjectId, _: &FsChange) {
            self.untagged.fetch_add(1, Ordering::Relaxed);
        }
        fn view_status(&self, _: ProjectId, worktree: &Path, _: &FsStatus) {
            self.tags.lock().push(worktree.to_path_buf());
        }
        fn view_changed(&self, _: ProjectId, worktree: &Path, _: &FsChange) {
            self.tags.lock().push(worktree.to_path_buf());
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "cide tests")
            .env("GIT_AUTHOR_EMAIL", "tests@cide.invalid")
            .env("GIT_COMMITTER_NAME", "cide tests")
            .env("GIT_COMMITTER_EMAIL", "tests@cide.invalid")
            .output()
            .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository with one commit, indexed the way `fs_index` indexes a project: hidden and
    /// ignored files shown, and the default excluded folders — `.cide/worktrees` among them —
    /// compiled against the project's roots.
    fn project(
        tag: &str,
    ) -> (
        cide_fs::testing::Scratch,
        FsRegistry,
        ProjectId,
        Arc<ProjectFs>,
        Arc<dyn FsEvents>,
        Arc<Tagged>,
    ) {
        let dir = cide_fs::testing::scratch(tag);
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "gc.auto", "0"]);
        git(&dir, &["config", "maintenance.auto", "false"]);
        git(&dir, &["config", "user.name", "cide tests"]);
        git(&dir, &["config", "user.email", "tests@cide.invalid"]);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "base"]);

        let registry = FsRegistry::default();
        let project = ProjectId::new();
        let roots = vec![dir.to_path_buf()];
        let rules = vec![".cide/worktrees".to_string(), ".cide/tasks".to_string()];
        let exclusions = Arc::new(Exclusions::compile(roots.clone(), &rules).unwrap());
        let tagged = Arc::new(Tagged::default());
        let events: Arc<dyn FsEvents> = tagged.clone();
        let visibility = Visibility {
            hidden: true,
            ignored: true,
        };
        registry
            .claim_configured(project, roots, visibility, exclusions, false, None)
            .unwrap()
            .run(Arc::clone(&events), project);
        let fs = registry.get(project).unwrap();
        (dir, registry, project, fs, events, tagged)
    }

    #[test]
    fn a_checkout_is_its_own_tree_and_its_root_is_never_excluded() {
        let (dir, _registry, project, fs, events, tagged) = project("wt-view");
        let checkout = cide_git::worktree::ensure(&dir, "developer-t-1")
            .unwrap()
            .path;
        std::fs::write(checkout.join("only-here.rs"), "").unwrap();
        // A checkout's own nested `.cide/worktrees` is excluded inside it, as in a project.
        std::fs::create_dir_all(checkout.join(".cide/worktrees/nested")).unwrap();
        std::fs::write(checkout.join(".cide/worktrees/nested/deep.rs"), "").unwrap();

        // The project's own index never walked into the checkout: that is the premise.
        assert!(
            !fs.with_index(|index| index.file_paths())
                .iter()
                .any(|p| p.starts_with(&checkout)),
            "the project index holds nothing of the checkout"
        );

        let view = open(&fs, &events, project, &checkout).expect("an agent checkout is viewable");
        assert_eq!(view.root(), checkout);
        let files = view.with_index(|index| index.file_paths());
        assert!(files.contains(&checkout.join("only-here.rs")), "{files:?}");
        assert!(files.contains(&checkout.join("src/main.rs")), "{files:?}");
        assert!(
            !files
                .iter()
                .any(|p| p.starts_with(checkout.join(".cide/worktrees"))),
            "the checkout's own nested worktrees stay excluded: {files:?}"
        );
        let names: Vec<String> = view.rows(0, 64).into_iter().map(|row| row.name).collect();
        assert!(
            names.contains(&"src".to_string()) && names.contains(&"only-here.rs".to_string()),
            "{names:?}"
        );
        let status = view.status();
        assert!(!status.indexing && status.rows as usize == view.count());
        assert!(
            tagged.tags.lock().iter().all(|tag| *tag == checkout) && !tagged.tags.lock().is_empty(),
            "the walk's status is tagged with the checkout"
        );
        assert!(fs.writable_paths().contains(&checkout));

        // The second call is the same view, not a second walk.
        let again = open(&fs, &events, project, &checkout).unwrap();
        assert!(Arc::ptr_eq(&again, &view));
    }

    #[test]
    fn anything_but_one_of_this_projects_checkouts_is_refused() {
        let (dir, _registry, project, fs, events, _) = project("wt-refuse");
        let elsewhere = cide_fs::testing::scratch("wt-refuse-theirs");
        let theirs = elsewhere.join("hotfix");
        git(
            &dir,
            &[
                "worktree",
                "add",
                "-q",
                theirs.to_str().unwrap(),
                "-b",
                "hf",
            ],
        );
        for path in [
            dir.join("src"),
            dir.to_path_buf(),
            theirs,
            PathBuf::from("/"),
        ] {
            assert!(
                matches!(
                    open(&fs, &events, project, &path),
                    Err(FsError::OutsideProject(_))
                ),
                "{} is not a checkout cide made",
                path.display()
            );
        }
        assert!(fs.views().is_empty());
    }

    #[test]
    fn a_create_in_a_checkout_shows_its_row_before_the_watcher_says_so() {
        let (dir, _registry, project, fs, events, _) = project("wt-fold");
        let checkout = cide_git::worktree::ensure(&dir, "developer-t-2")
            .unwrap()
            .path;
        let view = open(&fs, &events, project, &checkout).unwrap();
        let created =
            crate::cmd::fs::create_entry(&fs, &checkout.join("src"), "new.rs", false).unwrap();
        assert_eq!(created, checkout.join("src/new.rs"));
        view.with_index_mut(|index| index.expand(&checkout.join("src")));
        assert!(
            view.rows(0, 64).iter().any(|row| row.path == created),
            "folded into the checkout's tree by the command, not left to the watcher"
        );
        let renamed = checkout.join("src/renamed.rs");
        crate::cmd::fs::rename_entry(&fs, &created, &renamed).unwrap();
        let paths: Vec<PathBuf> = view.rows(0, 64).into_iter().map(|row| row.path).collect();
        assert!(
            paths.contains(&renamed) && !paths.contains(&created),
            "{paths:?}"
        );
    }

    #[test]
    fn the_cap_keeps_the_two_most_recent_and_a_dropped_one_is_rebuilt() {
        let (dir, _registry, project, fs, events, _) = project("wt-cap");
        let a = cide_git::worktree::ensure(&dir, "a").unwrap().path;
        let b = cide_git::worktree::ensure(&dir, "b").unwrap().path;
        let c = cide_git::worktree::ensure(&dir, "c").unwrap().path;
        let first = open(&fs, &events, project, &a).unwrap();
        open(&fs, &events, project, &b).unwrap();
        open(&fs, &events, project, &c).unwrap();
        let held: Vec<PathBuf> = fs.views().iter().map(|v| v.root().to_path_buf()).collect();
        assert_eq!(
            held,
            [c.clone(), b.clone()],
            "most recent first, the oldest evicted"
        );
        assert!(
            first.budget.cancelled(),
            "an evicted view stops its watcher"
        );
        let rebuilt = open(&fs, &events, project, &a).unwrap();
        assert!(
            !Arc::ptr_eq(&rebuilt, &first),
            "the next call walks it again"
        );
        assert!(rebuilt.count() > 0);
    }

    #[test]
    fn a_removed_checkout_is_forgotten_and_closing_the_project_drops_its_views() {
        let (dir, registry, project, fs, events, _) = project("wt-gone");
        let checkout = cide_git::worktree::ensure(&dir, "gone").unwrap().path;
        let held = open(&fs, &events, project, &checkout).unwrap();
        std::fs::remove_dir_all(&checkout).unwrap();
        assert!(matches!(
            open(&fs, &events, project, &checkout),
            Err(FsError::OutsideProject(_))
        ));
        assert!(held.budget.cancelled() && fs.views().is_empty());

        let other = cide_git::worktree::ensure(&dir, "kept").unwrap().path;
        let open = open(&fs, &events, project, &other).unwrap();
        drop(registry.remove(project));
        assert!(
            open.budget.cancelled(),
            "a view never outlives its project's entry"
        );
    }
}
