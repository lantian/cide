//! Committing, in both staging models.
//!
//! # Changelist mode (the default)
//!
//! `.git/index` is a derived artifact. A commit:
//!
//! 1. checks the [external-staging guard](crate::changelist),
//! 2. resolves every selection against the **HEAD → working tree** diff, before touching
//!    anything,
//! 3. resets the index to HEAD,
//! 4. writes exactly the selected changes into it,
//! 5. commits, and re-records the fingerprint.
//!
//! Step 3 is what makes "commit one changelist and the other is untouched" true no matter
//! what the index happened to contain — and it is also why the guard in step 1 exists, since
//! that reset is precisely the clobber a user's `git add` in a bash pane would suffer.
//!
//! Because the reset lands before the writes, a selection's patch is applied to a pre-image
//! that is HEAD, which is the side the HEAD→worktree diff was computed against. That
//! agreement is the whole reason the selections are resolved *first*.
//!
//! # Staging-area mode
//!
//! The index is the truth: it is committed as it stands, no reset, no guard, no changelist.
//! This is IDEA's own opt-out and the plan's stated mitigation for people who want git's
//! model back.
//!
//! # Merges
//!
//! A commit made while `MERGE_HEAD` exists keeps the extra parents and clears the merge
//! state afterwards, so finishing a merge from the panel produces the same commit `git
//! commit` would. Any other in-progress operation — a rebase, a cherry-pick sequence, an
//! `am` — is refused: those have their own `--continue` and forging a commit underneath one
//! leaves the sequencer pointing at a state that no longer exists.

use std::collections::BTreeSet;
use std::path::Path;

use cide_ipc::git::{
    CommitOutcome, CommitRequest, DiffSide, FileState, GitError, PathSelection, Selection,
};
use git2::{ApplyLocation, Diff, Oid, Repository};

use crate::diff::{DiffRequest, RawFile};
use crate::{Result, Wrap, changelist, diff, patch, repo as repo_mod, status};

/// Commit, per [`CommitRequest`].
pub fn commit(root: &Path, request: &CommitRequest) -> Result<CommitOutcome> {
    let repo = repo_mod::open(root)?;

    let conflicts = repo_mod::conflicted_paths(&repo)?;
    if !conflicts.is_empty() {
        return Err(GitError::Conflicted { paths: conflicts });
    }
    let merge_heads = merge_heads(&repo)?;
    if let Some(operation) = repo_mod::operation_in_progress(&repo)
        && operation != "merge"
    {
        return Err(GitError::OperationInProgress { operation });
    }

    let sidecar = changelist::load(root);
    if !sidecar.use_staging_area && !request.force {
        changelist::require_index_unchanged(root, &repo)?;
    }

    let head_tree = diff::head_tree(&repo)?;
    if request.amend && head_tree.is_none() {
        return Err(GitError::Unborn);
    }

    let committed: Vec<String> = if sidecar.use_staging_area {
        // Nothing to build: whatever the user staged is what gets committed.
        staged_paths(&repo)?
    } else {
        let list = request
            .changelist
            .clone()
            .unwrap_or_else(|| sidecar.active.clone());
        let selections = match &request.selections {
            Some(explicit) => explicit.clone(),
            None => default_selections(root, &sidecar, &list)?,
        };
        // Answered before the reset, not after. `rebuild_index` clears the index down to HEAD,
        // so committing an empty changelist would otherwise throw away whatever the index held
        // and *then* report that there was nothing to commit.
        if selections.is_empty() && merge_heads.is_empty() {
            return Err(GitError::NothingToCommit);
        }
        rebuild_index(&repo, head_tree.as_ref(), &selections)?;
        selections.iter().map(|s| s.path.clone()).collect()
    };

    let outcome = match write_commit(&repo, request, &merge_heads, committed.len() as u32) {
        Ok(outcome) => outcome,
        Err(error) => {
            // The index is already whatever `rebuild_index` made it, so the fingerprint from
            // before this call describes an index that no longer exists. Leaving it would
            // raise the external-staging bar on cide's own write.
            let _ = changelist::record_index(root, &repo);
            return Err(error);
        }
    };

    if !merge_heads.is_empty() {
        repo.cleanup_state().wrap()?;
    }

    // The index now equals the tree that was just committed, so this is the fingerprint the
    // next commit will compare against.
    changelist::record_index(root, &repo)?;

    // The committed paths no longer have changes, so their changelist assignments are dead.
    let live = status::live_paths(root).unwrap_or_default();
    let _ = changelist::update(root, |data| {
        let _ = data.reconcile(&live);
        Ok(())
    });

    Ok(outcome)
}

/// Every changed path the changelist owns, excluding untracked and ignored files.
///
/// Untracked files are *not* commit candidates by default even though `owner_of` reports the
/// active list for them: IDEA keeps them in a separate `Unversioned` group precisely so that
/// a commit never sweeps up a stray build artifact. Naming one explicitly in
/// [`CommitRequest::selections`] still commits it.
fn default_selections(
    root: &Path,
    sidecar: &changelist::Sidecar,
    list: &str,
) -> Result<Vec<PathSelection>> {
    sidecar.get(list)?;
    let info = repo_mod::discover(std::slice::from_ref(&root.to_path_buf()))
        .into_iter()
        .find(|info| info.root == repo_mod::canonical(root))
        .ok_or_else(|| GitError::NotARepository {
            path: root.display().to_string(),
        })?;
    let changes = status::repo_changes(&info, status::StatusRequest::default())?;

    let mut out = Vec::new();
    for view in &changes.changelists {
        if view.id != list {
            continue;
        }
        for entry in &view.changes {
            if matches!(entry.worktree, FileState::Untracked | FileState::Ignored) {
                continue;
            }
            out.push(PathSelection::whole(entry.path.clone()));
        }
    }
    Ok(out)
}

/// Reset the index to HEAD and write exactly `selections` into it.
fn rebuild_index(
    repo: &Repository,
    head_tree: Option<&git2::Tree<'_>>,
    selections: &[PathSelection],
) -> Result<()> {
    // Resolved before anything is written: a patch synthesized from a diff taken *after* the
    // reset would have the reset index as its pre-image on one side and the old one on the
    // other, and every offset in it would be wrong.
    let mut plans: Vec<(String, Option<Vec<u8>>)> = Vec::with_capacity(selections.len());
    for selection in selections {
        let request = DiffRequest::new(DiffSide::Combined);
        let Some(file) = diff::file_diff(repo, &selection.path, request)? else {
            return Err(GitError::NoSuchChange {
                path: selection.path.clone(),
            });
        };
        check_rev(selection, &file)?;
        if whole(&file, &selection.selection)? {
            plans.push((file.path.clone(), None));
            continue;
        }
        let chosen = patch::choose(&file, &selection.selection)?;
        plans.push((file.path.clone(), patch::synthesize(&file, &chosen.lines)?));
    }

    let mut index = repo.index().wrap()?;
    match head_tree {
        Some(tree) => index.read_tree(tree).wrap()?,
        // Unborn HEAD: the pre-image is the empty tree.
        None => index.clear().wrap()?,
    }
    index.write().wrap()?;
    drop(index);

    let mut text = Vec::new();
    for (_, patch) in &plans {
        if let Some(patch) = patch {
            text.extend_from_slice(patch);
        }
    }
    if !text.is_empty() {
        let parsed = Diff::from_buffer(&text).map_err(|e| GitError::PatchRejected {
            detail: e.message().to_string(),
            patch: String::from_utf8_lossy(&text).into_owned(),
        })?;
        repo.apply(&parsed, ApplyLocation::Index, None)
            .map_err(|e| GitError::PatchRejected {
                detail: format!("{:?}: {}", e.class(), e.message()),
                patch: String::from_utf8_lossy(&text).into_owned(),
            })?;
    }

    let mut index = repo.index().wrap()?;
    index.read(true).wrap()?;
    let workdir = repo.workdir().ok_or_else(|| GitError::Bare {
        path: repo.path().display().to_string(),
    })?;
    for (path, patch) in &plans {
        if patch.is_some() {
            continue;
        }
        if std::fs::symlink_metadata(workdir.join(path)).is_ok() {
            index.add_path(Path::new(path)).wrap()?;
        } else {
            index.remove_path(Path::new(path)).wrap()?;
        }
    }
    index.write().wrap()
}

fn check_rev(selection: &PathSelection, file: &RawFile) -> Result<()> {
    if let Some(expected) = &selection.rev
        && &file.rev() != expected
    {
        return Err(GitError::StaleSelection {
            path: selection.path.clone(),
        });
    }
    Ok(())
}

fn whole(file: &RawFile, selection: &Selection) -> Result<bool> {
    if matches!(selection, Selection::Whole) || file.change_count() == 0 {
        return Ok(true);
    }
    Ok(patch::choose(file, selection)?.covers_everything)
}

fn write_commit(
    repo: &Repository,
    request: &CommitRequest,
    merge_heads: &[Oid],
    files: u32,
) -> Result<CommitOutcome> {
    let mut index = repo.index().wrap()?;
    let tree_id = index.write_tree().wrap()?;
    let tree = repo.find_tree(tree_id).wrap()?;
    let signature = repo.signature().wrap()?;
    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    let oid = if request.amend {
        let Some(head) = head else {
            return Err(GitError::Unborn);
        };
        // The original author is kept, which is what `git commit --amend` does and what
        // makes amending someone else's commit not silently claim it.
        head.amend(
            Some("HEAD"),
            None,
            Some(&signature),
            None,
            Some(&request.message),
            Some(&tree),
        )
        .wrap()?
    } else {
        if let Some(head) = &head
            && head.tree_id() == tree_id
            && merge_heads.is_empty()
        {
            return Err(GitError::NothingToCommit);
        }
        let mut parents: Vec<git2::Commit<'_>> = Vec::new();
        if let Some(head) = head {
            parents.push(head);
        }
        for oid in merge_heads {
            parents.push(repo.find_commit(*oid).wrap()?);
        }
        let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            &request.message,
            &tree,
            &refs,
        )
        .wrap()?
    };

    let summary = request.message.lines().next().unwrap_or("").to_string();
    Ok(CommitOutcome {
        oid: oid.to_string(),
        summary,
        files,
    })
}

fn merge_heads(repo: &Repository) -> Result<Vec<Oid>> {
    // `mergehead_foreach` needs `&mut Repository`, and everything else here holds `&`.
    // Reopening is cheaper than threading mutability through the whole module for one read.
    let mut repo = Repository::open(repo.path()).wrap()?;
    let mut heads = Vec::new();
    match repo.mergehead_foreach(|oid| {
        heads.push(*oid);
        true
    }) {
        Ok(()) => Ok(heads),
        // No MERGE_HEAD: not a merge, which is the normal case.
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).wrap(),
    }
}

/// Paths whose index entry differs from HEAD — what a staging-area-mode commit contains.
fn staged_paths(repo: &Repository) -> Result<Vec<String>> {
    let head_tree = diff::head_tree(repo)?;
    let index = repo.index().wrap()?;
    let diff = repo
        .diff_tree_to_index(head_tree.as_ref(), Some(&index), None)
        .wrap()?;
    let mut out = BTreeSet::new();
    for delta in diff.deltas() {
        if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
            out.insert(path.to_string_lossy().into_owned());
        }
    }
    Ok(out.into_iter().collect())
}
