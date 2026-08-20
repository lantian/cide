//! Finding repositories under a project's roots, and naming them stably.
//!
//! A project may hold several roots and each root may contain submodules, so "the repo" is
//! never a single thing. Discovery flattens all of that into one ordered list: every root's
//! repository first, then its submodules depth-first, deduplicated by canonical work tree so
//! two roots inside one repo produce one entry.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cide_ipc::RepoId;
use cide_ipc::git::{GitError, RepoInfo, RepoPath};
use git2::Repository;

use crate::{Result, Wrap};

/// A repository's stable key: the blake3 of its canonical work-tree path, in hex.
///
/// This is the `<blake3(root)>` in `$XDG_STATE_HOME/cide/repos/<blake3(root)>/`. Hashing
/// rather than escaping the path keeps the directory name a fixed length and free of
/// separators, and canonicalising first means a symlinked checkout and its target share one
/// sidecar rather than silently keeping two sets of changelists.
pub fn repo_key(root: &Path) -> String {
    blake3::hash(canonical(root).as_os_str().as_encoded_bytes()).to_hex()[..32].to_string()
}

/// A deterministic [`RepoId`] for a work tree.
///
/// Derived from the same hash rather than minted fresh, so the id survives a restart and two
/// components that compute it independently agree. The alternative — a registry handing out
/// v4 uuids — needs the registry to be consulted before any id is meaningful, and a
/// `ChangesTree` built on a background thread would then have to wait on it.
pub fn repo_id(root: &Path) -> RepoId {
    let hash = blake3::hash(canonical(root).as_os_str().as_encoded_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    RepoId::from(uuid::Uuid::from_bytes(bytes))
}

/// Resolve symlinks and `..`, falling back to the input when the path does not exist.
///
/// The fallback matters: a root can be removed while a project is open, and every caller
/// here would rather report "not a repository" than panic on an id it could not compute.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Open the repository whose work tree is exactly `root`.
///
/// Deliberately not `discover`: every function in this crate takes the work-tree root that
/// [`discover`] already resolved, and re-discovering from a path the caller believes is a
/// root would silently operate on the parent repository when the root has been deleted.
pub fn open(root: &Path) -> Result<Repository> {
    let repo = Repository::open(root).map_err(|_| GitError::NotARepository {
        path: root.display().to_string(),
    })?;
    if repo.is_bare() {
        return Err(GitError::Bare {
            path: root.display().to_string(),
        });
    }
    Ok(repo)
}

/// The work tree containing `path`, if any.
pub fn discover_root(path: &Path) -> Option<PathBuf> {
    let repo = Repository::discover(path).ok()?;
    repo.workdir().map(canonical)
}

/// Every repository under `roots`, roots before their submodules.
///
/// Errors are not propagated: a project with four roots, one of which is on an unmounted
/// NFS share, still has three working repositories and a panel that shows them is more use
/// than an error page. Anything unreadable is logged and skipped.
pub fn discover(roots: &[PathBuf]) -> Vec<RepoInfo> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();

    for root in roots {
        let Some(work) = discover_root(root) else {
            tracing::debug!(root = %root.display(), "no git repository above this root");
            continue;
        };
        if !seen.insert(work.clone()) {
            continue;
        }
        let info = RepoInfo {
            id: repo_id(&work),
            name: display_name(&work),
            root: work.clone(),
            parent: None,
            is_submodule: false,
        };
        let id = info.id;
        out.push(info);
        if let Ok(repo) = open(&work) {
            collect_submodules(&repo, id, &mut seen, &mut out, 0);
        }
    }
    out
}

/// How deep submodule nesting is followed.
///
/// Submodules can point at each other; libgit2 will happily walk that forever. Eight is well
/// past any real superproject and turns a cycle into a truncated tree rather than a hang.
const MAX_SUBMODULE_DEPTH: usize = 8;

fn collect_submodules(
    repo: &Repository,
    parent: RepoId,
    seen: &mut BTreeSet<PathBuf>,
    out: &mut Vec<RepoInfo>,
    depth: usize,
) {
    if depth >= MAX_SUBMODULE_DEPTH {
        tracing::warn!(depth, "stopping submodule descent; is there a cycle?");
        return;
    }
    let Ok(submodules) = repo.submodules() else {
        return;
    };
    for sub in submodules {
        // An uninitialised submodule is an empty directory. `sub.open()` fails on it, which
        // is the check — there is nothing to show and nothing to commit.
        let Ok(sub_repo) = sub.open() else {
            continue;
        };
        let Some(work) = sub_repo.workdir().map(canonical) else {
            continue;
        };
        if !seen.insert(work.clone()) {
            continue;
        }
        // `name()` fails only on non-UTF-8 bytes; a submodule always has *some* name.
        let name = sub
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|_| display_name(&work));
        let info = RepoInfo {
            id: repo_id(&work),
            root: work.clone(),
            name,
            parent: Some(parent),
            is_submodule: true,
        };
        let id = info.id;
        out.push(info);
        collect_submodules(&sub_repo, id, seen, out, depth + 1);
    }
}

/// The last path component, or the whole path when there is no component (`/`).
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The directories a watcher must cover for this work tree: [`Repository::path`], and
/// [`Repository::commondir`] when they differ — which is exactly a linked worktree, where
/// `HEAD` and the index are per-worktree while refs and `packed-refs` are shared.
///
/// # Why the caller cannot work this out from the root
///
/// `cide-fs` resolves a root's `.git` itself (`cide_fs::filter::git_dir`, which parses the
/// `gitdir: <path>` line a worktree's `.git` *file* holds) and gets **one** directory. For an
/// ordinary checkout and for a submodule that is the whole answer. For a linked worktree it is
/// half of one: `<common>/worktrees/<name>/` holds `HEAD`, `index` and `ORIG_HEAD`, and holds
/// no `refs` and no `packed-refs` at all — git keeps those in the common directory so every
/// worktree of a repository sees one set of branches. A filter built from the resolved gitdir
/// alone therefore watches a worktree's checkout state and none of its refs, and a commit made
/// in it produces no ref event ever. cide's own agent worktrees under `.claude/worktrees/` are
/// that case, so this is a live shape rather than a hypothetical one.
///
/// Two directories rather than a fixed pair, because for the common case they are the same
/// directory and a caller that took `(git_dir, common_dir)` would have to deduplicate at every
/// site. The list is ordered per-worktree first, which is only cosmetic: the watcher takes
/// every entry.
///
/// Bare repositories are refused by [`open`], which is right — there is no work tree to watch.
pub fn watch_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let repo = open(root)?;
    // Both come back with a trailing separator (`/repo/.git/`). Harmless: `Path`'s equality
    // and `starts_with` are component-wise, so the comparison below and every `starts_with`
    // the filter does are unaffected, and `join` produces the same path either way.
    let git = repo.path().to_path_buf();
    let common = repo.commondir().to_path_buf();
    let mut out = vec![git];
    if common != out[0] {
        out.push(common);
    }
    Ok(out)
}

/// Look one repository up by id among `roots`.
pub fn find(roots: &[PathBuf], repo: RepoId) -> Result<RepoInfo> {
    discover(roots)
        .into_iter()
        .find(|r| r.id == repo)
        .ok_or(GitError::NoSuchRepo { repo })
}

/// The repository under `roots` that contains `path`, and the path relative to it.
///
/// The one seam between the two ways this app spells a path. The editor, the file tree, the tab
/// strip and `focusedTabPath` all hold **absolute** paths; everything in `cide_ipc::git` and
/// `cide_ipc::history` speaks **repo-relative** ones. Every caller that crosses that line comes
/// through here.
///
/// In Rust, and not as a prefix comparison in TypeScript, for the reason [`crate::tree_status`]
/// already gives about joining paths on the other side: `canonical` resolves symlinks, and a
/// project root that is a symlink — or a file reached through one — compares unequal to the work
/// tree it is actually inside. The frontend cannot canonicalise, so a prefix test there is
/// correct until the first symlinked checkout and then silently wrong.
///
/// **The innermost repository wins**, so a file inside a submodule resolves to the submodule and
/// not to the superproject. That is the same ordering [`discover`] imposes and the reason
/// `RepoInfo::parent` exists: a submodule has its own index, its own changelists and its own
/// history, and attributing its files to the superproject would put them in the wrong log.
///
/// `None` is an answer and not a failure: a scratch file, a `~/.cargo/registry` source opened by
/// go-to-definition, or a tab from another project is simply not in any of these repositories.
/// It is what tells the caller to hide the blame gutter and say so, rather than to report an
/// error the user cannot act on.
pub fn locate(roots: &[PathBuf], path: &Path) -> Option<RepoPath> {
    let wanted = canonical(path);
    let mut best: Option<(usize, RepoPath)> = None;
    for info in discover(roots) {
        let Some(rel) = relative(&info.root, &wanted) else {
            continue;
        };
        // Longest work-tree prefix, which is what "innermost" means once the paths are
        // canonical. Comparing the *root's* length rather than the relative path's: a shorter
        // remainder does not imply a deeper repository when two roots differ in depth.
        let depth = canonical(&info.root).as_os_str().len();
        if best.as_ref().is_none_or(|(seen, _)| depth > *seen) {
            best = Some((
                depth,
                RepoPath {
                    repo: info.id,
                    path: rel,
                },
            ));
        }
    }
    best.map(|(_, found)| found)
}

/// Repo-relative, slash-separated form of a path inside `root`.
///
/// Returns `None` for a path outside the work tree, which is the caller's cue to refuse
/// rather than to operate on something the repository does not contain.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let path = canonical(path);
    let rel = path.strip_prefix(canonical(root)).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Whatever `.git` says is half-finished: `merge`, `rebase`, `cherry-pick`, `revert`,
/// `bisect`.
///
/// Read from the repository state rather than from the presence of files, so it stays right
/// when libgit2 learns about a state we have not heard of.
pub fn operation_in_progress(repo: &Repository) -> Option<String> {
    use git2::RepositoryState::*;
    let name = match repo.state() {
        Clean => return None,
        Merge => "merge",
        Revert | RevertSequence => "revert",
        CherryPick | CherryPickSequence => "cherry-pick",
        Bisect => "bisect",
        Rebase | RebaseInteractive | RebaseMerge => "rebase",
        ApplyMailbox | ApplyMailboxOrRebase => "am",
    };
    Some(name.to_string())
}

/// The paths libgit2 reports as conflicted.
pub fn conflicted_paths(repo: &Repository) -> Result<Vec<String>> {
    let index = repo.index().wrap()?;
    if !index.has_conflicts() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in index.conflicts().wrap()? {
        let entry = entry.wrap()?;
        // `our` is absent for a delete/modify conflict; `their` is absent for the mirror
        // case. Taking whichever exists is what makes both show up in the list.
        let path = entry
            .our
            .as_ref()
            .or(entry.their.as_ref())
            .or(entry.ancestor.as_ref())
            .map(|e| String::from_utf8_lossy(&e.path).into_owned());
        if let Some(path) = path {
            out.push(path);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_key_is_stable_and_path_shaped() {
        let a = repo_key(Path::new("/tmp/does-not-exist-cide"));
        let b = repo_key(Path::new("/tmp/does-not-exist-cide"));
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, repo_key(Path::new("/tmp/other-cide")));
    }

    #[test]
    fn repo_id_matches_the_key() {
        let path = Path::new("/tmp/does-not-exist-cide");
        assert_eq!(repo_id(path), repo_id(path));
        assert_ne!(repo_id(path), repo_id(Path::new("/tmp/other-cide")));
    }

    /// Run `git` in `dir` with the user's own configuration kept out of it.
    ///
    /// `tests/support` does this by pointing `HOME` at a scratch directory once per process,
    /// which a unit test inside the library cannot do without racing every other test in the
    /// same binary. Per-command environment is the equivalent that needs no `set_var`.
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

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cide-git-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    /// The two shapes, side by side, because the second is only meaningful against the first:
    /// an ordinary checkout must produce **one** directory, or every project would pay for a
    /// case it does not have.
    #[test]
    fn watch_dirs_is_one_directory_for_a_checkout_and_two_for_a_linked_worktree() {
        let dir = scratch("watch-dirs");
        let main = dir.join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "one"]);

        let dirs = watch_dirs(&main).expect("a checkout is watchable");
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        assert_eq!(
            dirs[0].components().collect::<Vec<_>>(),
            main.join(".git").components().collect::<Vec<_>>(),
            "the one directory is the checkout's own `.git`: {dirs:?}"
        );

        let wt = dir.join("wt");
        git(
            &main,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "side"],
        );

        let dirs = watch_dirs(&wt).expect("a linked worktree is watchable");
        assert_eq!(
            dirs.len(),
            2,
            "a linked worktree's state is split and both halves need a watch: {dirs:?}"
        );
        assert_ne!(dirs[0], dirs[1]);
        assert!(
            dirs[0].ends_with("worktrees/wt"),
            "per-worktree first — `HEAD`, `index`, `ORIG_HEAD`: {dirs:?}"
        );
        assert!(
            dirs[0].join("HEAD").is_file(),
            "and it really does hold HEAD: {dirs:?}"
        );
        assert!(
            !dirs[0].join("refs/heads").exists(),
            "and really does not hold the branch refs — which is the whole bug. `refs/` \
             itself is there, empty, for the few refs git does keep per worktree: {dirs:?}"
        );
        assert!(
            dirs[1].join("refs/heads/side").is_file(),
            "the common directory is where the branch this worktree is on actually lives: \
             {dirs:?}"
        );

        // And the main checkout is unchanged by a worktree existing: still one directory.
        assert_eq!(watch_dirs(&main).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A submodule is the case [`crate::repo::watch_dirs`]'s single-directory answer is still
    /// exactly right for: `<super>/.git/modules/<name>` holds HEAD, index *and* refs.
    #[test]
    fn watch_dirs_gives_a_submodule_its_own_single_directory() {
        let dir = scratch("watch-dirs-sub");
        let upstream = dir.join("upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q", "-b", "main"]);
        std::fs::write(upstream.join("lib.txt"), "lib\n").unwrap();
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-qm", "lib"]);

        let super_ = dir.join("super");
        std::fs::create_dir_all(&super_).unwrap();
        git(&super_, &["init", "-q", "-b", "main"]);
        std::fs::write(super_.join("a.txt"), "a\n").unwrap();
        git(&super_, &["add", "."]);
        git(&super_, &["commit", "-qm", "one"]);
        git(
            &super_,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                upstream.to_str().unwrap(),
                "sub",
            ],
        );
        git(&super_, &["commit", "-qm", "add sub"]);

        let dirs = watch_dirs(&super_.join("sub")).expect("a submodule is a work tree");
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        assert!(dirs[0].ends_with("modules/sub"), "{dirs:?}");
        assert!(dirs[0].join("refs/heads").is_dir(), "{dirs:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_refuses_paths_outside_the_root() {
        assert_eq!(
            relative(Path::new("/tmp"), Path::new("/tmp/a/b")),
            Some("a/b".to_string())
        );
        assert_eq!(
            relative(Path::new("/tmp/a"), Path::new("/etc/passwd")),
            None
        );
    }
}
