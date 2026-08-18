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
//!   [`Filter::git_paths`] names, which are watched on purpose and never drawn.
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

/// The files under a git directory that a branch or staging change touches.
///
/// `HEAD` covers checkout, `index` covers staging, and the refs cover commits and fetches.
/// `refs/heads` is listed separately from `refs` on purpose: committing on `main` rewrites
/// `refs/heads/main`, which is a change *inside* `refs/heads` and therefore invisible to a
/// non-recursive watch on `refs` alone. Two extra watch descriptors buy the case that
/// actually happens.
const GIT_WATCHED: [&str; 5] = ["HEAD", "index", "refs", "refs/heads", "refs/tags"];

/// The directory neither half of [`Visibility`] can uncover.
const GIT_DIR: &str = ".git";

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
/// watcher already watches the five paths inside it that mean something (see [`GIT_WATCHED`])
/// without drawing a row for any of them. So `.git` is excluded structurally: a component
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
    /// Explicitly watched paths under a git directory, which the `.git` rule would
    /// otherwise reject.
    git_paths: Vec<PathBuf>,
    /// Which populations the tree shows. Read by [`Filter::admits`] and by nothing else —
    /// [`Filter::watchable`] deliberately does not consult `ignored`.
    visibility: Visibility,
}

impl Filter {
    /// Build the matchers for a set of roots, given every directory the walk visited.
    ///
    /// `dirs` is read for `.gitignore` files with one `stat` each. That is a few thousand
    /// syscalls on a large tree, done once, off the walk's critical path — cheap next to
    /// re-deriving the rules per event, which is the alternative.
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

        let mut excludes = HashMap::new();
        let mut git_paths = Vec::new();
        for root in roots {
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
            for name in GIT_WATCHED {
                let path = git_dir.join(name);
                if path.exists() {
                    git_paths.push(path);
                }
            }
        }

        Self {
            roots: roots.to_vec(),
            per_dir,
            excludes,
            global,
            git_paths,
            visibility,
        }
    }

    /// What this filter was built to show.
    pub fn visibility(&self) -> Visibility {
        self.visibility
    }

    /// The git metadata paths worth an explicit watch.
    pub fn git_paths(&self) -> &[PathBuf] {
        &self.git_paths
    }

    /// Whether a path is one of the watched git files, or lives under one of them.
    pub fn is_git_path(&self, path: &Path) -> bool {
        self.git_paths.iter().any(|p| path.starts_with(p))
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
        match self.classify(path) {
            Verdict::Refused => false,
            Verdict::Always => true,
            Verdict::Ask => !self.is_ignored(path, is_dir),
        }
    }

    /// The part of the decision [`Visibility`] has no say in.
    fn classify(&self, path: &Path) -> Verdict {
        // The five watched paths under a git directory. First, because the `.git` rule below
        // would refuse every one of them.
        if self.is_git_path(path) {
            return Verdict::Always;
        }
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
/// An enum rather than two `bool`s so the two public answers cannot drift: both match on all
/// three arms, and the compiler names the omission if a fourth is ever added.
enum Verdict {
    /// Outside every root, under `.git`, or hidden with `hidden` off. Nobody shows it.
    Refused,
    /// A root itself, or one of the watched git paths. Nobody may filter it out.
    Always,
    /// An ordinary path: the ignore rules and [`Visibility::ignored`] decide.
    Ask,
}

/// The real git directory for a root, or `None` when the root is not in a repository.
///
/// `.git` is a *file* rather than a directory in a linked worktree and in a submodule, and
/// it holds `gitdir: <path>` pointing at the real one. This is not a corner case for cide:
/// the repository this is being written in is checked out as a linked worktree, where
/// `.git` reads `gitdir: /home/…/.git/worktrees/wf_…`. Treating `.git` as a directory there
/// finds no `HEAD` and silently watches nothing.
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
        let filter = Filter::build(&[root], vec![], Visibility::CONSERVATIVE);
        assert!(filter.git_paths().contains(&real.join("HEAD")));
    }
}
