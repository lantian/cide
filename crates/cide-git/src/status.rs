//! The changes tree: what the 420px commit tool window draws.
//!
//! One [`RepoChanges`] per repository, roots before their submodules, with every changed
//! path filed under the changelist that owns it. Untracked and ignored entries are kept in
//! their own buckets rather than in a changelist, matching IDEA's `Unversioned` and `Ignore`
//! groups — they are not commit candidates.
//!
//! An untracked path cannot be moved *into* a changelist either, and that is this module's
//! doing rather than the sidecar's: `live` below excludes `Untracked` and `Ignored`, and
//! [`changelist::Sidecar::reconcile`] drops every assignment outside it, so filing one
//! succeeds and is gone by the next walk. The panel's answer is to add the file to git first
//! — `stage::stage`, then the move — which takes it out of `Untracked` and therefore into
//! `live`. See `filing_an_untracked_path_after_adding_it_sticks` in `tests/staging.rs` for
//! the sequence and `ui/src/sidebar/GitPanel/dragDrop.ts` for the gesture that runs it.
//!
//! Both sides of the index are reported per path. Collapsing them into one status is what
//! makes a partially-staged file render as a lie, and the tri-state checkbox in the tree is
//! a function of the pair.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cide_ipc::git::{
    BranchInfo, ChangeEntry, ChangelistView, ChangesTree, FileState, RepoChanges, RepoInfo,
};
use git2::{Repository, Status, StatusOptions};

use crate::{Result, Wrap, changelist, repo as repo_mod};

/// Whether ignored files are walked at all.
///
/// Off by default because a repository with a large `target/` or `node_modules/` turns a
/// status refresh into a full tree walk, and the `Ignore` group is collapsed in the mock. The
/// panel asks for it when the user expands that group.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatusRequest {
    pub include_ignored: bool,
}

/// Every repository under `roots`, with its changes.
///
/// A repository that cannot be read is skipped with a log line rather than failing the whole
/// tree: a project with four roots, one on an unmounted share, still has three working
/// repositories and a panel showing them is more use than an error page.
pub fn changes_tree(roots: &[PathBuf], request: StatusRequest) -> Result<ChangesTree> {
    let mut repos = Vec::new();
    for info in repo_mod::discover(roots) {
        match repo_changes(&info, request) {
            Ok(changes) => repos.push(changes),
            Err(error) => {
                tracing::warn!(root = %info.root.display(), %error, "skipping unreadable repository")
            }
        }
    }
    Ok(ChangesTree { repos })
}

/// One repository's changes.
pub fn repo_changes(info: &RepoInfo, request: StatusRequest) -> Result<RepoChanges> {
    let repo = repo_mod::open(&info.root)?;
    let entries = collect(&repo, request)?;

    let live: BTreeSet<String> = entries
        .iter()
        .filter(|e| !matches!(e.worktree, FileState::Untracked | FileState::Ignored))
        .map(|e| e.path.clone())
        .collect();

    // Reconciling here rather than on every mutation is deliberate: this is the one place
    // that knows which paths still have changes, and without it the sidecar accumulates one
    // dead assignment per file ever filed into a named list. The write only happens when
    // something was actually dropped — this function runs on a 150 ms debounce while the user
    // types, and an unconditional save would be a disk write per keystroke.
    let mut sidecar = changelist::load(&info.root);
    if sidecar.reconcile(&live)
        && let Err(error) = changelist::save(&info.root, &sidecar)
    {
        tracing::warn!(%error, "could not write the reconciled changelists sidecar");
    }

    let mut unversioned = Vec::new();
    let mut ignored = Vec::new();
    let mut conflicts = Vec::new();
    let mut filed: Vec<ChangeEntry> = Vec::new();

    for mut entry in entries {
        entry.changelist = sidecar.owner_of(&entry.path).to_string();
        match (entry.index, entry.worktree) {
            (FileState::Conflicted, _) | (_, FileState::Conflicted) => conflicts.push(entry),
            (_, FileState::Ignored) => ignored.push(entry),
            (_, FileState::Untracked) => unversioned.push(entry),
            _ => filed.push(entry),
        }
    }

    let changelists = sidecar
        .lists
        .iter()
        .map(|list| ChangelistView {
            id: list.id.clone(),
            name: list.name.clone(),
            comment: list.comment.clone(),
            active: list.id == sidecar.active,
            changes: filed
                .iter()
                .filter(|entry| entry.changelist == list.id)
                .cloned()
                .collect(),
        })
        .collect();

    Ok(RepoChanges {
        repo: info.clone(),
        branch: branch_info(&repo)?,
        changelists,
        unversioned,
        ignored,
        conflicts,
        index_changed_externally: changelist::index_changed_externally(&info.root, &repo)?,
        use_staging_area: sidecar.use_staging_area,
    })
}

fn collect(repo: &Repository, request: StatusRequest) -> Result<Vec<ChangeEntry>> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(request.include_ignored)
        .include_unmodified(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true)
        .include_unreadable(false);

    let statuses = repo.statuses(Some(&mut opts)).wrap()?;
    let mut out = Vec::with_capacity(statuses.len());
    for entry in statuses.iter() {
        let Ok(path) = entry.path() else {
            // A path libgit2 could not render as UTF-8. Skipping it is wrong in principle,
            // but every downstream type on the wire is a `String` and inventing a lossy
            // name here would let a commit target a file the user never saw.
            tracing::warn!("skipping a status entry with a non-UTF-8 path");
            continue;
        };
        let status = entry.status();
        let head_to_index = entry.head_to_index();
        let index_to_workdir = entry.index_to_workdir();

        let orig_path = head_to_index
            .as_ref()
            .and_then(|d| d.old_file().path())
            .or_else(|| index_to_workdir.as_ref().and_then(|d| d.old_file().path()))
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| p != path);

        let submodule = [&head_to_index, &index_to_workdir].iter().any(|delta| {
            delta.as_ref().is_some_and(|d| {
                u32::from(d.new_file().mode()) == u32::from(git2::FileMode::Commit)
                    || u32::from(d.old_file().mode()) == u32::from(git2::FileMode::Commit)
            })
        });

        // libgit2 only sets the binary flag once it has looked at the content, which a
        // status walk does not do for every file. This is therefore a hint for the tree row;
        // `FileDiff::binary`, computed from a real diff, is what staging trusts.
        let binary = [&head_to_index, &index_to_workdir]
            .iter()
            .any(|delta| delta.as_ref().is_some_and(|d| d.flags().is_binary()));

        out.push(ChangeEntry {
            path: path.to_string(),
            orig_path,
            index: index_side(status),
            worktree: worktree_side(status),
            staged: head_to_index.is_some(),
            binary,
            submodule,
            changelist: String::new(),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn index_side(status: Status) -> FileState {
    if status.is_conflicted() {
        FileState::Conflicted
    } else if status.is_index_new() {
        FileState::Added
    } else if status.is_index_deleted() {
        FileState::Deleted
    } else if status.is_index_renamed() {
        FileState::Renamed
    } else if status.is_index_typechange() {
        FileState::TypeChange
    } else if status.is_index_modified() {
        FileState::Modified
    } else {
        FileState::Unmodified
    }
}

fn worktree_side(status: Status) -> FileState {
    if status.is_conflicted() {
        FileState::Conflicted
    } else if status.is_wt_new() {
        FileState::Untracked
    } else if status.is_ignored() {
        FileState::Ignored
    } else if status.is_wt_deleted() {
        FileState::Deleted
    } else if status.is_wt_renamed() {
        FileState::Renamed
    } else if status.is_wt_typechange() {
        FileState::TypeChange
    } else if status.is_wt_modified() {
        FileState::Modified
    } else {
        FileState::Unmodified
    }
}

/// Head, upstream and the ahead/behind counts.
pub fn branch_info(repo: &Repository) -> Result<BranchInfo> {
    let operation = repo_mod::operation_in_progress(repo);

    let head = match repo.head() {
        Ok(head) => head,
        Err(e)
            if e.code() == git2::ErrorCode::UnbornBranch
                || e.code() == git2::ErrorCode::NotFound =>
        {
            // A repository with no commits still has a branch name in `.git/HEAD`, and the
            // status bar showing it is the difference between "empty repo on main" and a
            // blank field the user cannot interpret.
            let name = repo
                .find_reference("HEAD")
                .ok()
                .and_then(|r| {
                    r.symbolic_target()
                        .ok()
                        .flatten()
                        .map(|t| short_name(t).to_string())
                })
                .unwrap_or_else(|| "main".to_string());
            return Ok(BranchInfo {
                head: name,
                detached: false,
                upstream: None,
                ahead: 0,
                behind: 0,
                operation,
                unborn: true,
            });
        }
        Err(e) => return Err(e).wrap(),
    };

    let detached = repo.head_detached().unwrap_or(false);
    let name = if detached {
        head.target()
            .map(|oid| oid.to_string()[..8].to_string())
            .unwrap_or_else(|| "HEAD".to_string())
    } else {
        head.shorthand().unwrap_or("HEAD").to_string()
    };

    let mut upstream = None;
    let (mut ahead, mut behind) = (0, 0);
    if !detached
        && let Ok(branch) = repo.find_branch(&name, git2::BranchType::Local)
        && let Ok(remote) = branch.upstream()
    {
        upstream = remote.name().ok().flatten().map(str::to_owned);
        if let (Some(local), Some(other)) = (head.target(), remote.get().target())
            && let Ok((a, b)) = repo.graph_ahead_behind(local, other)
        {
            ahead = a as u32;
            behind = b as u32;
        }
    }

    Ok(BranchInfo {
        head: name,
        detached,
        upstream,
        ahead,
        behind,
        operation,
        unborn: false,
    })
}

/// `refs/heads/main` → `main`.
fn short_name(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
}

/// Every path with a change, for callers that need the live set without the whole tree.
pub fn live_paths(root: &Path) -> Result<BTreeSet<String>> {
    let repo = repo_mod::open(root)?;
    Ok(collect(&repo, StatusRequest::default())?
        .into_iter()
        .map(|e| e.path)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_strips_the_ref_namespace() {
        assert_eq!(short_name("refs/heads/main"), "main");
        assert_eq!(short_name("refs/remotes/origin/main"), "origin/main");
        assert_eq!(short_name("main"), "main");
    }
}
