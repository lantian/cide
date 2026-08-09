//! Finding repositories under a project's roots, and naming them stably.
//!
//! A project may hold several roots and each root may contain submodules, so "the repo" is
//! never a single thing. Discovery flattens all of that into one ordered list: every root's
//! repository first, then its submodules depth-first, deduplicated by canonical work tree so
//! two roots inside one repo produce one entry.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cide_ipc::RepoId;
use cide_ipc::git::{GitError, RepoInfo};
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

/// Look one repository up by id among `roots`.
pub fn find(roots: &[PathBuf], repo: RepoId) -> Result<RepoInfo> {
    discover(roots)
        .into_iter()
        .find(|r| r.id == repo)
        .ok_or(GitError::NoSuchRepo { repo })
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
