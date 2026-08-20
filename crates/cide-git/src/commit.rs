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
    /*
     * M18 — the amend target is checked here, against the repository, before a byte is written.
     *
     * `None` — the commit panel's Amend checkbox — is a no-op: that checkbox is *about* HEAD and
     * cannot name anything else. The log's Amend sets it, and the check is against the
     * repository rather than against the row the menu was opened on, because HEAD can move
     * between the two.
     */
    require_amend_head(&repo, request.amend, request.amend_of.as_deref())?;

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
        //
        // **An amend is admitted with nothing selected, because that is a reword.** Changing only
        // the message is the commonest amend there is, and the guard above refused it: nothing is
        // ticked, so `selections` is empty and the request never reached the amend path. Widening
        // the guard rather than deleting it keeps what it is actually for — an *ordinary* commit
        // of an empty changelist, which would clear the index to HEAD and report a failure,
        // having already destroyed the thing the user might have wanted.
        //
        // What a reword then does is exactly right by construction: `rebuild_index` with no
        // selections leaves the index at HEAD's tree, and `head.amend` writes that tree under a
        // new message. The worktree is not touched, so unstaged work survives a reword — and in
        // `use_staging_area` mode this branch is not taken at all, so a hand-built `git add -p`
        // selection is never in reach of it.
        if selections.is_empty() && merge_heads.is_empty() && !request.amend {
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

/// Refuse an amend of anything but `HEAD`.
///
/// # Why this is a refusal and not a rewrite
///
/// Amending a commit that is not `HEAD` is a history rewrite — an interactive rebase — and
/// cide cannot finish one: the diff panes are read-only against `HEAD`, so a rebase that hit a
/// conflict would leave the user in a state nothing in the app can resolve. It is exactly the
/// argument [`crate::branch::pull`] makes for being fast-forward-only, and the honest answer is
/// the same: say what happened and stop.
///
/// # Why the check is here rather than in the menu
///
/// The menu already offers *Amend* only on the `HEAD` row. That is necessary and not
/// sufficient, because it is a race the log cannot win on its own: between the menu opening and
/// the confirmation being clicked, a `git commit` in a bash pane — or a colleague's push
/// followed by a pull — moves `HEAD`. On a repository somebody else is pushing to, that is a
/// matter of seconds. So the oid the user saw travels with the request and is compared *here*,
/// where the answer is read from the repository rather than from a list drawn a second ago.
///
/// And it lives inside [`commit`] rather than in an amend function of its own, because a second
/// amend implementation is a second place where "the original author is kept" can stop being
/// true.
fn require_amend_head(repo: &Repository, amend: bool, expected: Option<&str>) -> Result<()> {
    let (true, Some(expected)) = (amend, expected) else {
        return Ok(());
    };
    let head = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|_| GitError::Unborn)?
        .id()
        .to_string();
    if head != expected {
        // Both oids, because "stale" is not something the user can act on and "you asked to
        // amend a1b2c3d4 but HEAD is now e5f6a7b8" is.
        return Err(GitError::NotHead {
            oid: expected.to_string(),
            head,
        });
    }
    Ok(())
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
        diff::check_rev(selection, &file)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `git` in `dir` with the user's own configuration kept out of it.
    ///
    /// The same per-command environment `repo.rs`'s unit tests use, and for the same reason: a
    /// library test cannot point `HOME` at a scratch directory without racing every other test
    /// in the binary.
    fn git(dir: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cide-git-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    /// The race the guard exists for, in one function: the oid the menu was drawn from is no
    /// longer `HEAD`.
    ///
    /// Driven directly rather than through [`commit`] because the guard is the whole of the
    /// behaviour and this is where it lives. `CommitRequest::amend_of` is on the wire now and the
    /// log's *Amend…* sets it, so the end-to-end half is in `tests/commit_actions.rs`.
    #[test]
    fn amending_anything_but_head_is_refused() {
        let dir = scratch("amend-of");
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "first"]);
        let first = git(&dir, &["rev-parse", "HEAD"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "second"]);
        let second = git(&dir, &["rev-parse", "HEAD"]);

        let repo = Repository::open(&dir).expect("open");
        assert_eq!(
            require_amend_head(&repo, true, Some(&first)),
            Err(GitError::NotHead {
                oid: first.clone(),
                head: second.clone(),
            }),
            "amending a commit that HEAD has moved past is an interactive rebase, and cide \
             cannot finish one"
        );
        assert_eq!(
            require_amend_head(&repo, true, Some(&second)),
            Ok(()),
            "amending HEAD is the whole point of the feature"
        );
        // No expectation travelled with the request, so there is nothing to check against —
        // the pre-M18 behaviour, kept so an older frontend does not start failing.
        assert_eq!(require_amend_head(&repo, true, None), Ok(()));
        // And not an amend at all: the guard must not fire on an ordinary commit that happens
        // to carry a stale oid.
        assert_eq!(require_amend_head(&repo, false, Some(&first)), Ok(()));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
