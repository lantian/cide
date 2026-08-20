//! The ignore decision, shared by the walk and the watcher.
//!
//! # Why this exists
//!
//! The walk gets its ignore rules from [`ignore::WalkBuilder`], which reads `.gitignore` at
//! every level as it descends. The watcher gets raw paths from inotify and has no walk to
//! ask. If the two disagree, the disagreement is visible and expensive in exactly one
//! direction: a `cargo build` writes tens of thousands of files under `target/`, and a
//! watcher that does not know `target/` is ignored turns that into a storm of tree updates
//! for a directory the user cannot even see.
//!
//! So the matchers are built once, from the directories the walk actually visited, and both
//! sides consult the same object. "The same matchers" is meant literally: the per-directory
//! `.gitignore` files here are the ones found in the directories the walk descended into,
//! which are by construction the directories the walk did not consider ignored.
//!
//! # Where this deliberately differs from git
//!
//! * `.gitignore` files **above** a project root are not consulted. `WalkBuilder::parents`
//!   is turned off to match. Replicating it would mean walking an unbounded ancestor chain
//!   on every event, and a cide project root is a project, not an arbitrary subdirectory.
//! * Dotfiles and ignored files are shown or not according to [`Visibility`], which is a user
//!   setting. `.git` is the one thing neither half of that setting can bring back — see
//!   [`Visibility`] for why — apart from the handful of paths under it that
//!   [`Filter::watched_paths`] names, which are watched on purpose and never drawn. That
//!   list is not only git's — see [`Filter::watched_paths`] for the other reason a path is
//!   on it, and why the reasons are checked before anything else.
//!
//! # Two questions, not one: [`Filter::admits`] and [`Filter::watchable`]
//!
//! They were the same function until ignored files could be shown, and separating them is the
//! whole of how *show ignored files* is affordable. `admits` answers "does the tree contain
//! this", `watchable` answers "does the watcher watch and report this", and `watchable` is the
//! stricter of the two by exactly one rule: **an ignored path is never watched, even when it
//! is shown**.
//!
//! The asymmetry is deliberate and it is the cheap half of a trade that has no free option:
//!
//! * A watch is per *directory* (see `crate::watch`), so watching a shown `target/` costs one
//!   inotify descriptor per directory in it — thousands, against a per-user
//!   `fs.inotify.max_user_watches` that is 8192 on some distributions — and then one event per
//!   file `cargo build` writes, which is the storm this crate was written to avoid.
//! * The cost of *not* watching it is that rows under an ignored directory are the walk's
//!   snapshot: a build that rewrites `target/debug/` does not move them until the project is
//!   indexed again. That is a stale corner of a subtree the user opted into seeing, and it is
//!   visibly better than a file tree that repaints every two seconds for the length of a build.
//!
//! Nothing else in the crate is allowed to re-derive either answer: `Index` asks `admits`, the
//! watcher asks `watchable`, and the app's watch list is `dir_paths()` put through `watchable`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// The files under a git directory that a branch, staging or fetch change touches, paired
/// with whether the watch has to cover what is *underneath* them.
///
/// `HEAD` covers checkout, `index` covers staging, `ORIG_HEAD` and `FETCH_HEAD` cover the
/// operations that move a branch out from under the user (a rebase, a pull), `packed-refs`
/// covers every ref at once, and `refs` covers commits, branch creation and fetches.
///
/// # `refs` is recursive, and the two entries that stood in for that are gone
///
/// This list read `["HEAD", "index", "refs", "refs/heads", "refs/tags"]`, every one of them
/// non-recursive, and that is a watcher which stops working after a restart. A branch called
/// `feature/login` lives at `refs/heads/feature/login` — *inside* `refs/heads/feature/`,
/// a directory on nobody's watch list — so a commit on it produced no ref event. It appeared
/// to work in the session that created the branch, because `crate::watch`'s event loop
/// watches any newly created directory it is told about, and then silently stopped working on
/// the next launch, when `refs/heads/feature/` already existed and nothing put it on the list.
/// "Works until you restart" is the worst shape a watcher bug has.
///
/// `refs/remotes` had the same hole and was worse: that directory is created at clone time
/// and essentially never during a session, so a fetch that moved only remote-tracking refs
/// was missed **always**. Adding `refs/remotes` here by name would have fixed nothing — the
/// watch would see `refs/remotes/origin/` being created and not one ref inside it. That is
/// exactly the trap the `refs/heads` entry fell into, so the fix is not another name.
///
/// So `refs` is watched **recursively** and the two subdirectory entries are dropped as
/// subsumed. The cost is real and worth stating, because inotify has no recursive mode:
/// `notify` walks the tree and takes one descriptor per directory beneath the root. The count
/// therefore scales with loose-ref *directories*, not with refs — a repository with 50,000
/// refs has packed them, and packed refs are one file with zero directories. The bad case
/// that remains is tens of thousands of *loose* refs, and it degrades through the knob that
/// already exists: `crate::watch` turns `notify::ErrorKind::MaxFilesWatch` into a
/// `PollWatcher` with the reason carried to the UI. A second, private cap here would be a
/// second answer to the same question, drifting from the first.
///
/// # `packed-refs`, and why the `exists()` guard is not optional
///
/// `packed-refs` matters most in the shape that is otherwise darkest: a fresh clone, and any
/// repository after `git gc` or `git pack-refs`, keeps its refs *only* there — so it is the
/// one file that moves when a ref moves, and without it such a repository reports nothing.
/// [`Filter::build_with`] guards every entry with `path.exists()`, which means a fresh
/// `git init` (no `packed-refs` yet) gets no watch for it until the project is indexed again.
/// That guard cannot be dropped: `inotify_add_watch` on a missing path is `ENOENT`, so the
/// alternative is not a deferred watch, it is a failed one plus a log line.
const GIT_WATCHED: [(&str, bool); 6] = [
    ("HEAD", false),
    ("index", false),
    ("ORIG_HEAD", false),
    ("FETCH_HEAD", false),
    ("packed-refs", false),
    ("refs", true),
];

/// Paths under a git directory that are never admitted and never watched, whatever else says.
///
/// Nothing in this crate needs the guard yet, and landing it before the hazard is the point.
/// [`Filter::is_watched_path`] is a `starts_with` over a list, so the moment anything puts a
/// whole git directory on that list — a plausible-looking one-line "just watch `.git`" —
/// [`Filter::admits`] becomes true for `.git/objects/**`, and `crate::watch`'s
/// new-directory rule then takes a descriptor on up to 256 fanout directories per fetch.
/// That is precisely the inotify exhaustion `crate::watch`'s header exists to prevent, and it
/// would arrive as a silent regression inside an unrelated change.
///
/// * `objects` — one loose object per version of every file ever committed (60,000 in this
///   repository), under a 256-way fanout of directories a `git fetch` writes into.
/// * `lfs` — git-lfs's local object cache: the same shape, frequently larger.
/// * `worktrees` — every *other* worktree's private git directory. This entry is the subtle
///   one, because a linked worktree's own git directory **is** `<common>/worktrees/<name>`.
///   [`Filter::is_never_path`] therefore resolves against the *deepest* git directory that
///   contains the path: ours is `<common>/worktrees/<name>`, so its `HEAD` is `HEAD` and
///   admitted, while somebody else's is `worktrees/<theirs>/HEAD` relative to the common
///   directory and refused. Matching the shallowest instead would refuse the one file a
///   linked worktree most needs watched.
const GIT_NEVER: [&str; 3] = ["objects", "lfs", "worktrees"];

/// The directory neither half of [`Visibility`] can uncover.
const GIT_DIR: &str = ".git";

/// The project's own directory: `config.json`, `agents/*.md`, `tasks.json`.
///
/// Unlike everything else cide persists, this one is **committed** — the user hand-edits it and
/// a teammate's `git pull` rewrites it — so cide has to notice it changing under a running app.
/// The dotfile rule alone would make that impossible; see [`Filter::watched_paths`].
const CIDE_DIR: &str = ".cide";

/// Which of the two populations the walk used to drop wholesale the tree actually shows.
///
/// Two independent booleans rather than one "show everything" flag, because they cost
/// completely different things and the user's two questions are unrelated:
///
/// * **`hidden`** is dotfiles — `.claude`, `.github`, `.env`. There are tens of them in a
///   repository, they are project files like any other, and the reported bug was that cide
///   could not show `.claude` at all. It defaults **on** in
///   `cide_ipc::ExplorerSettings`, because the cost is a rounding error and the absence is a
///   surprise.
/// * **`ignored`** is everything `.gitignore` covers — `target/`, `node_modules/`, `dist/`.
///   On this repository alone that is ~200,000 entries against ~1,500 tracked ones, and every
///   one of them becomes an arena node, a `Ctrl+P` candidate and a row the scrollbar has to
///   span. It defaults **off**, and the Settings screen says what turning it on costs.
///
/// # `.git` is not on this list, and that is a decision rather than an omission
///
/// A `.git` directory is a database, not content: it holds one loose object per version of
/// every file ever committed — 60,000 of them in this repository — none of which can be
/// usefully opened, renamed or deleted from a file tree, and all of which would be indexed by
/// the picker and watched by the watcher. IDEA hides it, every editor hides it, and the
/// watcher already watches the handful of paths inside it that mean something (see
/// [`GIT_WATCHED`]) without drawing a row for any of them. So `.git` is excluded
/// structurally: a component
/// named `.git` is refused by [`Filter::admits`] and pruned by the walk, whatever `hidden`
/// says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visibility {
    /// Show dot-prefixed entries.
    pub hidden: bool,
    /// Show entries the ignore rules cover. See the cost note above.
    pub ignored: bool,
}

impl Visibility {
    /// Neither population — what every walk in this workspace did before the setting existed.
    ///
    /// This is [`Default`], and it is deliberately **not** the app's default: `cide-ipc`'s
    /// `ExplorerSettings` turns `hidden` on, and the app passes the user's setting explicitly
    /// at every walk it starts. Two defaults that must agree is a drift bug waiting to happen,
    /// so these two are allowed to differ *and* the app is never allowed to fall back to this
    /// one. It exists for the walks that are nobody's file tree — a dependency package's
    /// sources under *External Libraries*, a scratch directory — where the conservative answer
    /// is the right one and there is no user setting to consult.
    pub const CONSERVATIVE: Self = Self {
        hidden: false,
        ignored: false,
    };
}

impl Default for Visibility {
    fn default() -> Self {
        Self::CONSERVATIVE
    }
}

/// One explicitly watched path, and how much of it the watch has to cover.
///
/// A struct rather than a bare `PathBuf` because `refs` needs a *recursive* watch and every
/// other entry must not have one — see [`GIT_WATCHED`] for what a non-recursive `refs` cost,
/// and [`GIT_NEVER`] for what an over-eager recursive one would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedPath {
    /// The file or directory to take a descriptor on.
    pub path: PathBuf,
    /// Whether the watch must also cover the directories underneath `path`.
    pub recursive: bool,
    /// Whether a change here means "the branch readout and the git panel should refresh" —
    /// `cide_ipc::FsChange::git`. False for the project's own `.cide/`, which is on the same
    /// list for an unrelated reason. See [`Filter::is_git_path`].
    pub git: bool,
}

/// Everything a [`Filter`] is built from, named rather than positional.
///
/// A struct because the argument that had to be added is the one that reads worst
/// positionally: a third `&[PathBuf]` next to `roots` and `dirs`, at a call site where
/// swapping two of them compiles and produces a filter that silently watches nothing.
pub struct FilterInput<'a> {
    /// The project's roots.
    pub roots: &'a [PathBuf],
    /// Every directory the walk visited, read for a per-directory `.gitignore`.
    pub dirs: &'a [PathBuf],
    /// Every git directory whose metadata the watcher must cover.
    ///
    /// Supplied by the caller rather than derived here, because only a caller that can open a
    /// repository knows the answer. [`git_dir`] resolves a root's `.git` to exactly *one*
    /// directory, and for a linked worktree that is `<common>/worktrees/<name>` — which holds
    /// `HEAD`, `index` and `ORIG_HEAD` and **no `refs` at all**, because `refs/**` and
    /// `packed-refs` live in the common directory. Deriving from roots therefore half-watches
    /// every linked worktree: the `exists()` guard quietly drops the ref entries and a commit
    /// in that worktree produces no ref event, ever. cide's own agent worktrees under
    /// `.claude/worktrees/` are that case, so it is not hypothetical.
    ///
    /// In the app this is `cide_git::repo::discover` mapped through
    /// `cide_git::repo::watch_dirs`, joined in `cide-app` because that is the only layer
    /// allowed to know about both crates. Duplicates are expected — several project roots
    /// inside one repository resolve to one git directory — and [`Filter::build_with`]
    /// deduplicates them.
    ///
    /// A `git init` performed *after* the walk is not covered until the next index; there is
    /// no repository to discover at the moment this list is built.
    pub git_dirs: &'a [PathBuf],
    /// Which populations the tree shows. Must be the same value the walk that produced `dirs`
    /// was given — see [`Filter::build`].
    pub visibility: Visibility,
}

/// Ignore rules, reusable from any thread.
#[derive(Debug)]
pub struct Filter {
    roots: Vec<PathBuf>,
    /// One entry per visited directory that contains a `.gitignore`.
    per_dir: HashMap<PathBuf, Gitignore>,
    /// `$root/.git/info/exclude`, keyed by root.
    excludes: HashMap<PathBuf, Gitignore>,
    /// `core.excludesFile` / `$XDG_CONFIG_HOME/git/ignore`.
    global: Gitignore,
    /// The paths watched explicitly because the rules below would otherwise reject them.
    /// Two reasons put something here, and only two — see [`Filter::watched_paths`].
    watched_paths: Vec<WatchedPath>,
    /// The git directories those paths were derived from, deduplicated. Kept because
    /// [`Filter::is_never_path`] needs to know where a git directory *starts* in order to say
    /// what the first component under it is.
    git_dirs: Vec<PathBuf>,
    /// Which populations the tree shows. Read by [`Filter::admits`] and by nothing else —
    /// [`Filter::watchable`] deliberately does not consult `ignored`.
    visibility: Visibility,
}

impl Filter {
    /// Build the matchers from an explicit [`FilterInput`].
    ///
    /// `dirs` is read for `.gitignore` files with one `stat` each. That is a few thousand
    /// syscalls on a large tree, done once, off the walk's critical path — cheap next to
    /// re-deriving the rules per event, which is the alternative.
    pub fn build_with(input: FilterInput<'_>) -> Self {
        let FilterInput {
            roots,
            dirs,
            git_dirs,
            visibility,
        } = input;

        let (global, err) = Gitignore::global();
        if let Some(err) = err {
            tracing::debug!(%err, "global gitignore was read with complaints");
        }

        let mut per_dir = HashMap::new();
        for dir in dirs {
            let file = dir.join(".gitignore");
            if !file.is_file() {
                continue;
            }
            let mut builder = GitignoreBuilder::new(dir);
            if let Some(err) = builder.add(&file) {
                tracing::debug!(%err, path = %file.display(), "skipping an unreadable .gitignore");
                continue;
            }
            match builder.build() {
                Ok(gi) => {
                    per_dir.insert(dir.to_path_buf(), gi);
                }
                Err(err) => {
                    tracing::debug!(%err, path = %file.display(), "unusable .gitignore")
                }
            }
        }

        // A linked worktree contributes two directories and several project roots inside one
        // repository contribute the same one twice; either way a duplicate is a duplicate
        // inotify descriptor and a duplicate `starts_with` on every event that arrives.
        // Order-preserving rather than a `BTreeSet`, because the deepest-match rule in
        // `is_never_path` reads better against a list the caller can predict, and there are
        // never more than a handful of these.
        let mut git_dirs_unique: Vec<PathBuf> = Vec::new();
        for dir in git_dirs {
            if !git_dirs_unique.contains(dir) {
                git_dirs_unique.push(dir.clone());
            }
        }

        let mut excludes = HashMap::new();
        let mut watched_paths = Vec::new();
        for root in roots {
            // The project's own `.cide/`, and deliberately **not** guarded by `exists()` the
            // way the git paths below are. The event that matters most is `.cide/`
            // *appearing* — a teammate's `git pull`, a `git checkout` of a branch that has
            // one — and a list built from what happened to be on disk when the project was
            // indexed could never report it. A watch on a directory that is not there simply
            // fails, which `crate::watch` already treats as ordinary (a directory can vanish
            // between the walk and the watch), and the create arrives on the root's own watch.
            watched_paths.push(WatchedPath {
                path: root.join(CIDE_DIR),
                recursive: false,
                git: false,
            });

            // `info/exclude` is still resolved from the root's *own* git directory, which for
            // a linked worktree is `<common>/worktrees/<name>` and holds no `info/` — so a
            // linked worktree's `info/exclude` is not read. Left as it was on purpose: that
            // is a rule about *ignoring*, the file is per-worktree in git's own model, and
            // `git_dirs` is a flat list with no root to attribute an entry back to. The
            // watcher half — the three defects this pass is about — does not depend on it.
            let Some(git_dir) = git_dir(root) else {
                continue;
            };
            let exclude = git_dir.join("info/exclude");
            if exclude.is_file() {
                let mut builder = GitignoreBuilder::new(root);
                if builder.add(&exclude).is_none()
                    && let Ok(gi) = builder.build()
                {
                    excludes.insert(root.clone(), gi);
                }
            }
        }

        // Once per git directory, not once per root: a linked worktree's `HEAD` and the
        // common directory's `HEAD` are both real files and both worth a descriptor, and
        // `exists()` is what keeps the ones a given layout does not have off the list.
        for git_dir in &git_dirs_unique {
            for (name, recursive) in GIT_WATCHED {
                let path = git_dir.join(name);
                if path.exists() {
                    watched_paths.push(WatchedPath {
                        path,
                        recursive,
                        git: true,
                    });
                }
            }
        }

        Self {
            roots: roots.to_vec(),
            per_dir,
            excludes,
            global,
            watched_paths,
            git_dirs: git_dirs_unique,
            visibility,
        }
    }

    /// [`Filter::build_with`] with the git directories **derived** from the roots.
    ///
    /// For callers that have no repository knowledge and need none: a dependency package's
    /// sources under *External Libraries*, a content search's corpus, a test. What they have
    /// in common is that nothing watches git metadata for them, so [`git_dir`]'s one-directory
    /// answer is enough — and it is *not* enough for anything that does, which is why the app
    /// calls [`Filter::build_with`] with the list `cide_git` computed. See
    /// [`FilterInput::git_dirs`] for what the difference costs a linked worktree.
    ///
    /// `visibility` must be the same value the walk that produced `dirs` was given. It is a
    /// parameter rather than a default so that the one caller who knows — the app, holding the
    /// user's settings — has to say, and so that a caller who does not know cannot silently
    /// disagree with its own walk: a `Filter` that hides what the walk showed deletes those
    /// rows again on the first watcher burst that rescans their directory.
    pub fn build<'a>(
        roots: &[PathBuf],
        dirs: impl IntoIterator<Item = &'a Path>,
        visibility: Visibility,
    ) -> Self {
        // Collected rather than streamed, which costs one `PathBuf` clone per visited
        // directory. Against the `stat` per directory `build_with` is about to spend on the
        // same list, that is noise — and the app, the one caller with a large `dirs`, already
        // holds a `Vec<PathBuf>` from `Index::dir_paths` and calls `build_with` directly.
        let dirs: Vec<PathBuf> = dirs.into_iter().map(Path::to_path_buf).collect();
        let git_dirs: Vec<PathBuf> = roots.iter().filter_map(|r| git_dir(r)).collect();
        Self::build_with(FilterInput {
            roots,
            dirs: &dirs,
            git_dirs: &git_dirs,
            visibility,
        })
    }

    /// What this filter was built to show.
    pub fn visibility(&self) -> Visibility {
        self.visibility
    }

    /// The paths cide watches explicitly, because the rules in [`Filter::admits`] would
    /// otherwise reject them before any ignore matcher ran.
    ///
    /// Two reasons put something on this list, and they are the only two:
    ///
    /// * **git metadata** — the [`GIT_WATCHED`] paths under a git directory, which the branch
    ///   readout and the git panel read. Watched on purpose and never drawn: the walk prunes
    ///   `.git`, so no row under it ever exists to update.
    /// * **the project's own `.cide/`** ([`CIDE_DIR`]) — `config.json`, `agents/*.md` and
    ///   `tasks.json`. That directory is committed, so a teammate's `git pull` can rewrite it
    ///   under a running app, and without this entry cide would never hear about it:
    ///   [`Filter::admits`] refuses a dot-prefixed component *before* any gitignore matcher
    ///   runs, so with *Show hidden files* off the watcher would report nothing under
    ///   `.cide/` at all — and with it on, a `.gitignore` line saying `.cide/` would still
    ///   take it out. The list is checked first precisely so neither can.
    ///
    /// **This list answers [`Filter::watchable`] and nothing else.** It used to answer
    /// [`Filter::admits`] too, through a `WatchOnly` verdict that meant "watched, drawn by
    /// nobody" — which is what made `.cide/` invisible in the file tree with no setting able to
    /// reach it. Both are now ordinary as far as *drawing* goes: git metadata never produces a
    /// row because the walk prunes `.git`, and `.cide/` is a dotfile like `.claude/` beside it.
    /// What the list still buys is the unconditional half — watched whatever the ignore rules and
    /// *Show hidden files* say, which is what keeps the Tasks panel hearing about a `git pull`.
    pub fn watched_paths(&self) -> &[WatchedPath] {
        &self.watched_paths
    }

    /// Whether a path is one of [`Filter::watched_paths`], or lives under one of them.
    pub fn is_watched_path(&self, path: &Path) -> bool {
        self.watched_paths.iter().any(|w| path.starts_with(&w.path))
    }

    /// Whether a path is inside one of [`GIT_NEVER`]'s directories.
    ///
    /// Resolved against the **deepest** git directory containing the path, the way
    /// [`Filter::root_of`] resolves a root, and for a sharper reason: a linked worktree's own
    /// git directory *is* `<common>/worktrees/<name>`, so matching the common directory first
    /// would read its `HEAD` as "under `worktrees/`" and refuse the file the worktree most
    /// needs watched. Deepest-first makes the same rule say "another worktree's private
    /// directory" for `<common>/worktrees/<someone else>/HEAD`, which is what it means.
    fn is_never_path(&self, path: &Path) -> bool {
        let Some(git_dir) = self
            .git_dirs
            .iter()
            .filter(|d| path.starts_with(d))
            .max_by_key(|d| d.as_os_str().len())
        else {
            return false;
        };
        let Ok(rel) = path.strip_prefix(git_dir) else {
            return false;
        };
        // Bytes rather than `to_string_lossy`: this runs on every event the watcher sees, and
        // a non-UTF-8 component is not a reason to allocate a `String` per path.
        matches!(
            rel.components().next(),
            Some(Component::Normal(name))
                if GIT_NEVER.iter().any(|never| name.as_encoded_bytes() == never.as_bytes())
        )
    }

    /// The git half of [`Filter::is_watched_path`], which is a distinction two callers need.
    ///
    /// `cide_ipc::FsChange::git` means "the branch readout and the git panel should refresh",
    /// so a write to `.cide/tasks.json` must not raise it: a task write is not a commit. And
    /// `Index::apply` asks the same question to skip the paths the walk never made a row for;
    /// `.cide/` has no row either — it is [`Verdict::WatchOnly`] — so there the answer costs
    /// one `by_path` lookup that misses and returns, and nothing visible turns on which of the
    /// two predicates that caller uses.
    ///
    /// Told apart by a flag on the entry rather than by its name: the two reasons a path is on
    /// the list are known where the entry is created, and a rule that re-derived them from the
    /// path (`!p.ends_with(".cide")`) would need revisiting the day a third reason appears.
    pub fn is_git_path(&self, path: &Path) -> bool {
        self.watched_paths
            .iter()
            .any(|w| w.git && path.starts_with(&w.path))
    }

    /// Whether the tree should contain, and the watcher should report, this path.
    ///
    /// `is_dir` is what the caller believes: for a path that has just been deleted nobody
    /// can know, and the caller passes `false`. The consequence is bounded — a rule written
    /// as `build/` matches a directory only — and the alternative is dropping deletions,
    /// which is worse than occasionally reporting one that was ignored.
    pub fn admits(&self, path: &Path, is_dir: bool) -> bool {
        match self.classify(path) {
            Verdict::Refused => false,
            Verdict::Always => true,
            // The whole of what *show ignored files* does on this side of the crate: the
            // gitignore question is still asked — `is_ignored` below is what paints the rows
            // olive and what keeps them out of the watcher — it just stops being a veto.
            Verdict::Ask => self.visibility.ignored || !self.is_ignored(path, is_dir),
        }
    }

    /// Whether the **watcher** should watch this directory, or report this path.
    ///
    /// [`Filter::admits`] and one extra rule: an ignored path is never watched. See the module
    /// note for the trade — descriptors and a build's worth of events against a subtree that
    /// updates on the next index rather than live.
    pub fn watchable(&self, path: &Path, is_dir: bool) -> bool {
        /*
         * Asked **before** the verdict, and that is what lets `.cide/` be an ordinary dotfile in
         * the tree while still being watched unconditionally.
         *
         * The guarantee is unchanged and is the reason this is not simply `admits`: a project
         * whose `.gitignore` names `.cide/` — cide's own did — must not thereby become a project
         * whose Tasks panel never hears about a `git pull`, and neither must a user who turned
         * *Show hidden files* off. Neither path reaches [`Filter::is_ignored`] or the dotfile
         * rule. It used to be spelled as a `WatchOnly` verdict, which coupled the two answers and
         * so could only give `.cide/` both or neither.
         */
        if self.is_git_path(path) || self.is_watched_path(path) {
            return true;
        }
        match self.classify(path) {
            Verdict::Refused => false,
            Verdict::Always => true,
            Verdict::Ask => !self.is_ignored(path, is_dir),
        }
    }

    /// The part of the decision [`Visibility`] has no say in.
    fn classify(&self, path: &Path) -> Verdict {
        // [`GIT_NEVER`] before everything, including the watched list. That order *is* the
        // guard: the watched list is matched with `starts_with`, so anything that ever puts a
        // whole git directory on it would otherwise make `.git/objects/**` `Always` — admitted
        // by the tree and, worse, descended into by `crate::watch`'s new-directory rule, one
        // descriptor per fanout directory. See `GIT_NEVER`.
        if self.is_never_path(path) {
            return Verdict::Refused;
        }
        // Git metadata, before the dotfile rule below would refuse it. `.cide/` is deliberately
        // *not* short-circuited here any more — see the block that replaced it.
        if self.is_git_path(path) {
            return Verdict::Always;
        }
        /*
         * `.cide/` used to short-circuit to `WatchOnly` here — watched, and drawn by nobody.
         *
         * > *".cide folder isn't visible in file tree/git tree"*
         *
         * The reasoning was that its contents "are the project's configuration and its task
         * tracker, which are not the user's source". True, and not a reason to hide them: they
         * are hand-edited files the user is expected to open, committed to the repository, and
         * sitting beside `.claude/` and `.github/`, which the tree draws. `show_hidden_files`
         * defaults to **on**, so the dotfile rule was never what kept `.cide/` out — this branch
         * was, and no setting could reach it.
         *
         * Falling through means it is now an ordinary dotfile: drawn when hidden files are, kept
         * out when they are not, tinted when a project's `.gitignore` names it. The old note's
         * strongest point survives the change rather than opposing it — it warned that admitting
         * `.cide/` would turn task comments into `Ctrl+Shift+F` hits *"inside files the tree does
         * not draw, which is the worst version of that"*. The tree draws them now, so the hits
         * lead somewhere.
         *
         * The watch is unaffected: `watchable` answers `is_watched_path` before it asks for a
         * verdict, so the Tasks panel still hears about a `git pull` with hidden files off and in
         * a project whose `.gitignore` names the directory. That separation is the whole reason
         * the two questions are two functions.
         */
        let Some(root) = self.root_of(path) else {
            return Verdict::Refused;
        };
        if path == root {
            return Verdict::Always;
        }
        if let Ok(rel) = path.strip_prefix(root) {
            for component in rel.components() {
                let Component::Normal(name) = component else {
                    continue;
                };
                // `.git` whatever the setting says — see `Visibility`. Checked per component
                // rather than on the last one so that `…/.git/objects/ab/cd` is refused too.
                if name == GIT_DIR {
                    return Verdict::Refused;
                }
                // `WalkBuilder::hidden(true)`: any dot-prefixed component takes the whole path
                // out. A *component*, not the file name: a file inside `.claude/` is hidden
                // even though its own name is ordinary, which is what the walk does and
                // therefore what the watcher has to do.
                if !self.visibility.hidden && name.as_encoded_bytes().first() == Some(&b'.') {
                    return Verdict::Refused;
                }
            }
        }
        Verdict::Ask
    }

    /// Whether the ignore rules cover this path, with no regard for whether it is *shown*.
    ///
    /// Separate from [`Filter::admits`] because with *show ignored files* on, the tree needs
    /// the paths and the watcher needs the verdict about them, and one function cannot answer
    /// both.
    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        // Deepest first — a `.gitignore` nearer the file wins, including when it whitelists
        // something a shallower one ignored. `matched_path_or_any_parents` also applies the
        // rule that nothing inside an ignored directory can be brought back.
        let Some(root) = self.root_of(path) else {
            return false;
        };
        let mut dir = path.parent();
        while let Some(d) = dir {
            if let Some(gi) = self.per_dir.get(d) {
                match matched_under(gi, path, is_dir) {
                    Match::Ignore(_) => return true,
                    Match::Whitelist(_) => return false,
                    Match::None => {}
                }
            }
            if d == root {
                break;
            }
            dir = d.parent();
        }

        if let Some(gi) = self.excludes.get(root) {
            match matched_under(gi, path, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }

        // `matched`, not `matched_path_or_any_parents`: `Gitignore::global()` roots its
        // matcher at the process's *current directory*, and the parents form asserts that
        // the path is under the matcher's root — a panic, in the watcher thread, for any
        // project that is not below the cwd. The cost is that a global rule naming a
        // directory does not propagate to that directory's contents, which for the `*.swp`
        // and `.DS_Store` a global ignore file actually contains is no cost at all.
        self.global.matched(path, is_dir).is_ignore()
    }

    /// The root this path belongs to, longest first so a nested root wins.
    ///
    /// `None` means "outside every root", which both public answers treat as a refusal.
    fn root_of(&self, path: &Path) -> Option<&PathBuf> {
        self.roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.as_os_str().len())
    }
}

/// `matched_path_or_any_parents`, with the precondition it asserts checked rather than
/// assumed.
///
/// The assert inside `ignore` is a panic on a path that is not under the matcher's root.
/// Every caller here passes an ancestor's matcher, so it should be unreachable — but this
/// runs on the watcher thread, where a panic costs the user every future file change with no
/// message, and a returned `Match::None` costs one wrongly-admitted path.
fn matched_under<'a>(
    gi: &'a Gitignore,
    path: &Path,
    is_dir: bool,
) -> Match<&'a ignore::gitignore::Glob> {
    if !path.starts_with(gi.path()) {
        tracing::debug!(
            path = %path.display(),
            root = %gi.path().display(),
            "ignore matcher asked about a path outside its root"
        );
        return Match::None;
    }
    gi.matched_path_or_any_parents(path, is_dir)
}

/// What the visibility-independent half of the decision concluded.
///
/// An enum rather than two `bool`s so the two public answers cannot drift: both match on
/// every arm, and the compiler names the omission if a fifth is ever added. [`Verdict::WatchOnly`]
/// is why it is an enum at all — two `bool`s would have made "watched but not shown" a state
/// somebody had to remember to construct.
enum Verdict {
    /// Outside every root, under `.git`, or hidden with `hidden` off. Nobody shows it.
    Refused,
    /// A root itself, or one of the **git** paths in [`Filter::watched_paths`]. Nobody may
    /// filter it out — and nothing is lost by that, because the walk never produces a row
    /// under `.git` for [`Filter::admits`] to be asked about in the first place.
    Always,
    /// An ordinary path: the ignore rules and [`Visibility::ignored`] decide.
    Ask,
}

/// The real git directory for a root, or `None` when the root is not in a repository.
///
/// `.git` is a *file* rather than a directory in a linked worktree and in a submodule, and it
/// holds `gitdir: <path>` pointing at the real one. Treating `.git` as a directory there finds
/// no `HEAD` and silently watches nothing, so the `gitdir:` parsing stays: a **submodule**
/// needs it, and a submodule is the case this function still answers completely — its git
/// directory is `<super>/.git/modules/<name>`, which holds its `HEAD`, its `index` and its
/// `refs` together.
///
/// # What this cannot answer, and what an earlier comment here claimed
///
/// It used to say the repository cide is developed in *is* a linked worktree. That was
/// written from inside one of the agent worktrees under `.claude/worktrees/`; in the primary
/// checkout `git rev-parse --git-dir` and `--git-common-dir` are both `.git`.
///
/// The correction matters because a linked worktree's state is **split** and one path cannot
/// name both halves: `HEAD`, `index` and `ORIG_HEAD` are per-worktree, in
/// `<common>/worktrees/<name>/`, while `refs/**` and `packed-refs` live in the common
/// directory. This function returns the first of those, so a `Filter` built only from it has
/// no `refs` entry at all — [`Filter::build_with`]'s `exists()` guard drops it — and a commit
/// in a linked worktree produces no ref event ever. That is why [`FilterInput::git_dirs`] is
/// supplied by the caller, from `cide_git::repo::watch_dirs`, which asks libgit2 for both.
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let meta = std::fs::symlink_metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    let target = Path::new(target);
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        root.join(target)
    };
    resolved.is_dir().then_some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    #[test]
    fn a_gitignored_path_is_rejected_and_a_whitelisted_one_survives() {
        let dir = scratch("filter-basic");
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n*.log\n!keep.log\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();

        let roots = vec![dir.to_path_buf()];
        let filter = Filter::build(&roots, vec![dir.path()], Visibility::CONSERVATIVE);

        assert!(filter.admits(&dir.join("src/main.rs"), false));
        assert!(!filter.admits(&dir.join("target"), true));
        assert!(!filter.admits(&dir.join("target/debug/x.o"), false));
        assert!(!filter.admits(&dir.join("noise.log"), false));
        assert!(filter.admits(&dir.join("keep.log"), false));
    }

    #[test]
    fn a_deeper_gitignore_overrides_a_shallower_one() {
        let dir = scratch("filter-nested");
        std::fs::create_dir_all(dir.join("web/dist")).unwrap();
        std::fs::write(dir.join(".gitignore"), "dist/\n").unwrap();
        std::fs::write(dir.join("web/.gitignore"), "!dist/\n").unwrap();

        let roots = vec![dir.to_path_buf()];
        let filter = Filter::build(
            &roots,
            vec![dir.path(), &dir.join("web")],
            Visibility::CONSERVATIVE,
        );

        assert!(!filter.admits(&dir.join("dist"), true));
        assert!(filter.admits(&dir.join("web/dist"), true));
    }

    #[test]
    fn dotfiles_are_out_but_the_watched_git_paths_are_in() {
        let dir = scratch("filter-hidden");
        std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let filter = Filter::build(
            &[dir.to_path_buf()],
            vec![dir.path()],
            Visibility::CONSERVATIVE,
        );

        assert!(!filter.admits(&dir.join(".env"), false));
        assert!(!filter.admits(&dir.join(".git/objects/ab/cd"), false));
        assert!(filter.admits(&dir.join(".git/HEAD"), false));
        assert!(filter.admits(&dir.join(".git/refs/heads/main"), false));
        assert!(filter.is_git_path(&dir.join(".git/refs/heads/main")));
    }

    /// The other reason a path is on the watched list, and the harder one: `.cide/` is
    /// committed, so the project's own `.gitignore` may well name it — cide's own did, on a
    /// premise that was already stale when this was written.
    #[test]
    fn the_project_s_cide_directory_is_watched_and_never_shown() {
        let dir = scratch("filter-cide");
        std::fs::create_dir_all(dir.join(".cide/agents")).unwrap();
        std::fs::create_dir_all(dir.join(".other")).unwrap();
        std::fs::write(dir.join(".gitignore"), ".cide/\n").unwrap();
        std::fs::write(dir.join(".cide/tasks.json"), "{}").unwrap();
        std::fs::write(dir.join(".cide/agents/dev.md"), "").unwrap();
        std::fs::write(dir.join(".other/x"), "").unwrap();

        let filter = Filter::build(
            &[dir.to_path_buf()],
            vec![dir.path()],
            Visibility::CONSERVATIVE,
        );

        assert!(
            filter.watchable(&dir.join(".cide"), true),
            "watched, which is the whole point: a teammate's `git pull` rewrites this \
             directory and the Tasks panel has to hear about it"
        );
        assert!(
            filter.watchable(&dir.join(".cide/tasks.json"), false),
            "and the `.cide/` line in the project's own .gitignore does not take it back \
             out — `watchable` answers the watched list before it asks for a verdict, so it \
             never reaches `is_ignored`"
        );
        assert!(
            filter.watchable(&dir.join(".cide/agents/dev.md"), false),
            "at any depth: the list is matched with `starts_with`"
        );

        // Under `CONSERVATIVE` — hidden files off — `.cide/` is out, exactly as `.claude/` and
        // `.github/` are. That is the dotfile rule doing it, not a special case, which is the
        // whole change: the old `WatchOnly` verdict refused it whatever the settings said.
        assert!(!filter.admits(&dir.join(".cide/tasks.json"), false));
        assert!(!filter.admits(&dir.join(".cide/agents/dev.md"), false));
        assert!(!filter.admits(&dir.join(".cide"), true));

        // And with hidden files on — which is the **default** — it is drawn. This is the
        // reported bug: `show_hidden_files` was already on and `.cide/` still never appeared,
        // because no setting could reach the branch that refused it.
        let shown = Filter::build(
            &[dir.to_path_buf()],
            vec![dir.path()],
            Visibility {
                hidden: true,
                ignored: true,
            },
        );
        assert!(
            shown.admits(&dir.join(".cide"), true),
            "the directory is a row now — it holds config.json, agents/*.md and tasks.json, \
             which are committed, hand-edited project files sitting beside `.claude/`"
        );
        assert!(shown.admits(&dir.join(".cide/tasks.json"), false));
        assert!(
            shown.watchable(&dir.join(".cide/tasks.json"), false),
            "…and it is still watched, which is the half that must not have changed"
        );

        assert!(
            !filter.admits(&dir.join(".other/x"), false),
            "the escape hatch is a list of two things, not an amnesty for dot directories"
        );
        assert!(!filter.watchable(&dir.join(".other/x"), false));

        assert!(filter.is_watched_path(&dir.join(".cide/tasks.json")));
        assert!(
            !filter.is_git_path(&dir.join(".cide/tasks.json")),
            "`FsChange::git` refreshes the branch readout and the git panel; a task write is \
             not a commit"
        );
    }

    /// The two seeds share a list and not a verdict, and every other test here would still
    /// pass if a refactor folded them together.
    #[test]
    fn the_git_paths_keep_always_and_cide_does_not_share_it() {
        let dir = scratch("filter-verdicts");
        std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(dir.join(".cide")).unwrap();

        let filter = Filter::build(
            &[dir.to_path_buf()],
            vec![dir.path()],
            Visibility::CONSERVATIVE,
        );

        assert!(matches!(
            filter.classify(&dir.join(".git/HEAD")),
            Verdict::Always
        ));
        assert!(matches!(
            filter.classify(&dir.join(".git/refs/heads/main")),
            Verdict::Always
        ));
        assert!(
            filter.admits(&dir.join(".git/HEAD"), false),
            "unchanged, and it can afford to be: the walk never produces a row under `.git` \
             for `admits` to be asked about"
        );

        // `.cide/` is an ordinary dotfile now: refused under `CONSERVATIVE`, which has hidden
        // files off, and admitted once they are on. It used to classify as `WatchOnly` — watched
        // and drawn by nobody — which is what made it invisible in the tree with no setting able
        // to reach it. See the block in `classify`.
        assert!(matches!(
            filter.classify(&dir.join(".cide")),
            Verdict::Refused
        ));
        assert!(matches!(
            filter.classify(&dir.join(".cide/tasks.json")),
            Verdict::Refused
        ));
        assert!(matches!(
            filter.classify(&dir.join(".env")),
            Verdict::Refused
        ));
    }

    /// The entry cannot be conditional on the directory existing when the project was
    /// indexed: `.cide/` *arriving* is the event it exists to catch.
    #[test]
    fn cide_is_on_the_watched_list_before_the_directory_exists() {
        let dir = scratch("filter-cide-absent");
        let filter = Filter::build(
            &[dir.to_path_buf()],
            vec![dir.path()],
            Visibility::CONSERVATIVE,
        );

        assert!(!dir.join(".cide").exists());
        assert!(watches(&filter, &dir.join(".cide")));
        assert!(filter.watchable(&dir.join(".cide"), true));
        assert!(filter.watchable(&dir.join(".cide/config.json"), false));
        assert!(!filter.admits(&dir.join(".cide/config.json"), false));
    }

    /// The reported bug: `.claude` could not be shown at all.
    #[test]
    fn showing_hidden_files_uncovers_dotfiles_but_never_the_git_directory() {
        let dir = scratch("filter-show-hidden");
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::create_dir_all(dir.join(".git/objects/ab")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let filter = Filter::build(
            &[dir.to_path_buf()],
            vec![dir.path()],
            Visibility {
                hidden: true,
                ignored: false,
            },
        );

        assert!(filter.admits(&dir.join(".claude"), true));
        assert!(
            filter.admits(&dir.join(".claude/settings.json"), false),
            "a file inside a dot directory too — the rule is per component, and one that only \
             looked at the file's own name would show the directory and none of its contents"
        );
        assert!(
            filter.watchable(&dir.join(".claude/settings.json"), false),
            "and it is watched, because it is not ignored: hidden and ignored are separate axes"
        );
        assert!(
            !filter.admits(&dir.join(".git/objects/ab/cd"), false),
            "`.git` stays out whatever the setting says — see `Visibility`"
        );
        assert!(
            filter.admits(&dir.join(".git/HEAD"), false),
            "except the five watched paths, which are what tell the tree a commit happened"
        );
    }

    /// The other half, and the expensive one: `target/` shown, and still not watched.
    #[test]
    fn showing_ignored_files_admits_them_and_the_watcher_still_refuses_them() {
        let dir = scratch("filter-show-ignored");
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n*.log\n!keep.log\n").unwrap();

        let shown = Visibility {
            hidden: true,
            ignored: true,
        };
        let filter = Filter::build(&[dir.to_path_buf()], vec![dir.path()], shown);

        assert!(filter.admits(&dir.join("target"), true));
        assert!(filter.admits(&dir.join("target/debug/x.o"), false));
        assert!(filter.admits(&dir.join("noise.log"), false));
        assert!(filter.admits(&dir.join("src/main.rs"), false));

        assert!(
            !filter.watchable(&dir.join("target"), true),
            "the row exists and the watch does not: one inotify descriptor per directory under \
             `target/` is the ENOSPC this crate was written to avoid, and a cargo build would \
             then deliver one event per object file"
        );
        assert!(!filter.watchable(&dir.join("target/debug/x.o"), false));
        assert!(!filter.watchable(&dir.join("noise.log"), false));
        assert!(
            filter.watchable(&dir.join("src/main.rs"), false),
            "everything that is not ignored is watched exactly as before"
        );
        assert!(
            filter.watchable(&dir.join("keep.log"), false),
            "including a whitelisted path inside an ignored glob"
        );
        assert_eq!(filter.visibility(), shown);
    }

    #[test]
    fn a_path_outside_every_root_is_rejected() {
        let dir = scratch("filter-outside");
        std::fs::create_dir_all(&dir).unwrap();
        let filter = Filter::build(&[dir.join("project")], vec![], Visibility::CONSERVATIVE);
        assert!(!filter.admits(&dir.join("elsewhere/file.rs"), false));
    }

    #[test]
    fn a_worktree_git_file_resolves_to_the_real_git_dir() {
        let dir = scratch("filter-worktree");
        let real = dir.join("real/worktrees/wt");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let root = dir.join("checkout");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".git"), format!("gitdir: {}\n", real.display())).unwrap();

        assert_eq!(git_dir(&root).as_deref(), Some(real.as_path()));
        let filter = Filter::build(
            std::slice::from_ref(&root),
            vec![],
            Visibility::CONSERVATIVE,
        );
        assert!(watches(&filter, &real.join("HEAD")));
        assert!(
            !watches(&filter, &dir.join("real/refs")),
            "and this is the half `Filter::build` cannot see: a linked worktree's git \
             directory holds no `refs`, so deriving from the root alone watches HEAD and no \
             ref. `FilterInput::git_dirs` is how the common directory gets on the list"
        );

        // Which `build_with` fixes, given both halves — the shape `cide_git::repo::watch_dirs`
        // hands the app.
        let common = dir.join("real");
        std::fs::create_dir_all(common.join("refs/heads")).unwrap();
        std::fs::write(common.join("packed-refs"), "").unwrap();
        let filter = Filter::build_with(FilterInput {
            roots: &[root],
            dirs: &[],
            git_dirs: &[real.clone(), common.clone()],
            visibility: Visibility::CONSERVATIVE,
        });
        assert!(watches(&filter, &real.join("HEAD")));
        assert!(watches(&filter, &common.join("refs")));
        assert!(watches(&filter, &common.join("packed-refs")));
        assert!(
            filter
                .watched_paths()
                .iter()
                .any(|w| w.path == common.join("refs") && w.recursive),
            "and recursively, or `refs/heads/feature/login` is invisible again: {:?}",
            filter.watched_paths()
        );
        assert!(
            filter.is_git_path(&real.join("HEAD")),
            "the worktree's own HEAD is a git change, not somebody else's private directory — \
             `GIT_NEVER`'s `worktrees` entry is resolved against the deepest git directory"
        );
        assert!(
            !filter.admits(&common.join("worktrees/other/HEAD"), false),
            "while another worktree's private directory is refused, from the same rule"
        );
    }

    /// Whether the filter took an explicit watch on exactly this path.
    fn watches(filter: &Filter, path: &Path) -> bool {
        filter.watched_paths().iter().any(|w| w.path == path)
    }
}
