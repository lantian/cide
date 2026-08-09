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
//! * Dotfiles are ignored wholesale, which is `WalkBuilder`'s `hidden(true)` default and the
//!   behaviour the file tree wants. The exception is the handful of paths under `.git` that
//!   [`Filter::git_paths`] names, which are watched on purpose.

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
    /// Explicitly watched paths under a git directory, which the dotfile rule would
    /// otherwise reject.
    git_paths: Vec<PathBuf>,
}

impl Filter {
    /// Build the matchers for a set of roots, given every directory the walk visited.
    ///
    /// `dirs` is read for `.gitignore` files with one `stat` each. That is a few thousand
    /// syscalls on a large tree, done once, off the walk's critical path — cheap next to
    /// re-deriving the rules per event, which is the alternative.
    pub fn build<'a>(roots: &[PathBuf], dirs: impl IntoIterator<Item = &'a Path>) -> Self {
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
        }
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
        if self.is_git_path(path) {
            return true;
        }
        let Some(root) = self.root_of(path) else {
            return false;
        };
        if path == root {
            return true;
        }
        // `WalkBuilder::hidden(true)`: any dot-prefixed component takes the whole path out.
        if let Ok(rel) = path.strip_prefix(root)
            && rel.components().any(is_hidden_component)
        {
            return false;
        }

        // Deepest first — a `.gitignore` nearer the file wins, including when it whitelists
        // something a shallower one ignored. `matched_path_or_any_parents` also applies the
        // rule that nothing inside an ignored directory can be brought back.
        let mut dir = path.parent();
        while let Some(d) = dir {
            if let Some(gi) = self.per_dir.get(d) {
                match matched_under(gi, path, is_dir) {
                    Match::Ignore(_) => return false,
                    Match::Whitelist(_) => return true,
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
                Match::Ignore(_) => return false,
                Match::Whitelist(_) => return true,
                Match::None => {}
            }
        }

        // `matched`, not `matched_path_or_any_parents`: `Gitignore::global()` roots its
        // matcher at the process's *current directory*, and the parents form asserts that
        // the path is under the matcher's root — a panic, in the watcher thread, for any
        // project that is not below the cwd. The cost is that a global rule naming a
        // directory does not propagate to that directory's contents, which for the `*.swp`
        // and `.DS_Store` a global ignore file actually contains is no cost at all.
        !self.global.matched(path, is_dir).is_ignore()
    }

    /// The root this path belongs to, longest first so a nested root wins.
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

fn is_hidden_component(c: Component<'_>) -> bool {
    match c {
        Component::Normal(name) => name.as_encoded_bytes().first() == Some(&b'.'),
        _ => false,
    }
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
        let filter = Filter::build(&roots, vec![dir.path()]);

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
        let filter = Filter::build(&roots, vec![dir.path(), &dir.join("web")]);

        assert!(!filter.admits(&dir.join("dist"), true));
        assert!(filter.admits(&dir.join("web/dist"), true));
    }

    #[test]
    fn dotfiles_are_out_but_the_watched_git_paths_are_in() {
        let dir = scratch("filter-hidden");
        std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let filter = Filter::build(&[dir.to_path_buf()], vec![dir.path()]);

        assert!(!filter.admits(&dir.join(".env"), false));
        assert!(!filter.admits(&dir.join(".git/objects/ab/cd"), false));
        assert!(filter.admits(&dir.join(".git/HEAD"), false));
        assert!(filter.admits(&dir.join(".git/refs/heads/main"), false));
        assert!(filter.is_git_path(&dir.join(".git/refs/heads/main")));
    }

    #[test]
    fn a_path_outside_every_root_is_rejected() {
        let dir = scratch("filter-outside");
        std::fs::create_dir_all(&dir).unwrap();
        let filter = Filter::build(&[dir.join("project")], vec![]);
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
        let filter = Filter::build(&[root], vec![]);
        assert!(filter.git_paths().contains(&real.join("HEAD")));
    }
}
