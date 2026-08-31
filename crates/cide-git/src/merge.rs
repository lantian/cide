//! `git merge <name>`: the branch picker's *Merge into current branch*. (M24)
//!
//! # Why this is not `pull.rs`, and not `worktree::integrate`
//!
//! [`crate::pull`]'s `merge_pull` has the right *body* — ADR 0009's real sequencer state,
//! `ORIG_HEAD` before anything moves, conflicts reported as `Ok` — but the wrong *message
//! oracle*: a pull's merge title comes from `fmt-merge-msg` reading `FETCH_HEAD`, which names
//! the remote-side branch and its URL, and `pull.rs`'s own doc warns that reproducing
//! `git merge`'s message there would be wrong on every pull. The reverse is equally true here,
//! so the two message functions live in the file whose oracle they match, and each one's doc
//! points at the other.
//!
//! [`crate::worktree::integrate`] is the other near miss: it already merges a named branch
//! into HEAD, but it predates ADR 0009 — the merge runs in memory, a conflict is a refusal
//! that writes nothing, there is no `MERGE_HEAD` for the resolver or for `git merge --abort`,
//! and it never settles the changelist sidecar. Modelling on it would have rebuilt the exact
//! state this crate spent M20 getting rid of. (Moving `integrate` onto this entry point is the
//! obvious follow-up; it is not done here so the agent-worktree flow keeps its own report
//! shape until someone owns that change.)
//!
//! # The source is resolved exactly as named
//!
//! `branch::find` — checkout's resolver — maps `origin/feature` onto an existing local
//! `feature`, because *checking out* a remote-tracking ref means "give me a local branch that
//! tracks it". Merge means no such thing: `git merge origin/feature` merges the remote-tracking
//! ref itself, and hijacking it onto a same-named local would merge different commits exactly
//! when the two have drifted apart — the one moment the difference matters. So this module
//! does its own two-step lookup, local then remote-tracking, and remembers which arm won
//! because the message names the kind.

use std::path::Path;

use cide_ipc::git::{GitError, MergeOutcome};
use git2::{BranchType, Oid, Repository};

use crate::pull::{
    PULL_COMMIT_CAP, diff_totals, settle, short_oid, taken_commits, write_orig_head,
};
use crate::{Result, Wrap, branch, repo as repo_mod, status};

/// Merge `name` — a local or remote-tracking branch — into the checked-out branch.
///
/// Fast-forwards when the current branch is strictly behind (git's default), writes a real
/// two-parent merge commit otherwise, and on conflict leaves git's own state on disk —
/// `MERGE_HEAD`, index stages 1/2/3, markers — reported as **`Ok` with `conflicts` non-empty**,
/// the same contract `pull` established: the gesture did what it was asked, and what remains
/// is work for the resolver, not a refusal for the red toast.
///
/// Written as one straight line for `pull_with`'s reason: the *sequence* is the guarantee.
/// Every refusal happens while the repository is exactly as the user left it.
pub fn merge_into_head(root: &Path, name: &str) -> Result<MergeOutcome> {
    let repo = repo_mod::open(root)?;

    // 1-2. A half-finished operation, and an index with unresolved entries. Starting a second
    // merge over either is one of the refusals ADR 0009 deliberately kept.
    if let Some(operation) = repo_mod::operation_in_progress(&repo) {
        return Err(GitError::OperationInProgress { operation });
    }
    let unresolved = repo_mod::conflicted_paths(&repo)?;
    if !unresolved.is_empty() {
        return Err(GitError::Conflicted { paths: unresolved });
    }

    // 3. Somewhere to merge *into*. The UI hides the action on a detached or unborn HEAD, but
    // the popup's list can be minutes stale; the backstop refusal is the truth.
    let head = status::branch_info(&repo)?;
    if head.unborn {
        return Err(GitError::Unborn);
    }
    if head.detached {
        return Err(GitError::DetachedHead { head: head.head });
    }
    let local_branch = repo
        .find_branch(&head.head, BranchType::Local)
        .map_err(|_| GitError::NoSuchBranch {
            name: head.head.clone(),
        })?;
    let Some(local_oid) = local_branch.get().target() else {
        return Err(GitError::NoSuchBranch { name: head.head });
    };

    // 4. The source, exactly as named — see the module doc for why `branch::find` is not used.
    let (source_oid, source_is_remote) = find_source(&repo, name)?;

    // 5. `behind == 0` is *already contains*, whatever `ahead` says: the source is an ancestor
    // (or the branch itself — merging your own name lands here too), so there is nothing to
    // take and nothing is written. Same test, same reason, as `pull_with` step 7.
    let (ahead, behind) = repo.graph_ahead_behind(local_oid, source_oid).wrap()?;
    if source_oid == local_oid || behind == 0 {
        return report(
            &repo,
            name,
            &head.head,
            false,
            local_oid,
            local_oid,
            source_oid,
            0,
            Vec::new(),
        );
    }

    // 6. Unrelated histories, before either arm — git's own *refusing to merge unrelated
    // histories*, asked here so it cannot surface as a raw `GitError::Git` from deep inside.
    if repo.merge_base(local_oid, source_oid).is_err() {
        return Err(GitError::UnrelatedHistories {
            branch: head.head,
            upstream: name.to_string(),
        });
    }

    let target = repo.find_commit(source_oid).wrap()?;
    // The dirty-tree refusal, before anything is written, under **both** arms. The refusal
    // names the *source* ref rather than the current branch: the sentence built from it is
    // "merging <x> would overwrite…", and the branch that is not moving is not the news.
    let in_the_way = branch::blockers(&repo, &target)?;
    if !in_the_way.is_empty() {
        return Err(GitError::CheckoutWouldOverwrite {
            branch: name.to_string(),
            paths: in_the_way,
        });
    }

    if ahead == 0 {
        // 7a. Fast-forward: `git merge`'s default when the branch has nothing of its own.
        // The report is built before the working tree moves — `pull::fast_forward`'s
        // argument: failing here leaves the tree where it was, failing after the checkout
        // would report a merge that had actually happened as failed.
        let outcome = report(
            &repo,
            name,
            &head.head,
            true,
            local_oid,
            source_oid,
            source_oid,
            behind as u32,
            Vec::new(),
        )?;
        write_orig_head(&repo, local_oid);
        let mut builder = git2::build::CheckoutBuilder::new();
        builder.safe();
        repo.checkout_tree(target.as_object(), Some(&mut builder))
            .wrap()?;
        repo.reference(
            &format!("refs/heads/{}", head.head),
            source_oid,
            true,
            "cide: merge",
        )
        .wrap()?;
        settle(root, &repo);
        return Ok(outcome);
    }

    // 7b. A true merge — `merge_pull`'s body, minus the upstream. Every comment on that
    // function's choices (SAFE checkout, default `MergeOptions` = rename detection on,
    // `[ours, theirs]` parent order) holds here unchanged.
    write_orig_head(&repo, local_oid);
    let annotated = repo.find_annotated_commit(source_oid).wrap()?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.safe();
    repo.merge(&[&annotated], None, Some(&mut checkout))
        .wrap()?;

    // Overwrite libgit2's `MERGE_MSG` (`Merge commit '<full oid>'` plus a `#Conflicts:`
    // block) with git's own sentence, before anything can read it — the conflicted path
    // concludes through `conflict::cont`, which commits whatever `MERGE_MSG` holds, and a
    // conflicted merge must not end with a different message from a clean one.
    let message = merge_message(name, source_is_remote, &head.head);
    let _ = std::fs::write(repo.path().join("MERGE_MSG"), &message);

    let index = repo.index().wrap()?;
    let conflicts = repo_mod::conflicts_of(&index)?;
    if !conflicts.is_empty() {
        // Left exactly as it is, and reported as success — `merge_pull`'s contract, at
        // length, in its own comment. `MERGE_HEAD` on disk is what makes this resumable
        // after a restart, legible to `git status` in a pane, and abandonable with
        // `git merge --abort` by somebody who would rather not use the resolver.
        settle(root, &repo);
        return report(
            &repo,
            name,
            &head.head,
            false,
            local_oid,
            local_oid,
            source_oid,
            behind as u32,
            conflicts,
        );
    }

    // Clean. Conclude it here rather than leaving `MERGE_HEAD` for the panel: a merge with
    // nothing to decide is not a state to make somebody click through.
    let signature = repo.signature().wrap()?;
    let ours = repo.find_commit(local_oid).wrap()?;
    let tree_oid = repo.index().wrap()?.write_tree().wrap()?;
    let tree = repo.find_tree(tree_oid).wrap()?;
    let created = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &tree,
            // `[ours, theirs]`, in that order — the reverse is a valid commit that is wrong
            // in a way only `git log --first-parent` reveals, months later.
            &[&ours, &target],
        )
        .wrap()?;
    repo.cleanup_state().wrap()?;
    settle(root, &repo);
    report(
        &repo,
        name,
        &head.head,
        false,
        local_oid,
        created,
        source_oid,
        behind as u32,
        Vec::new(),
    )
}

/// Resolve `name` the way `git merge` does: a local branch, else a remote-tracking one.
///
/// The order is the point — an existing local `feature` wins over `origin/feature` only when
/// the user typed `feature`, never by prefix-stripping. `peel_to_commit` rather than
/// `get().target()` so a symbolic remote ref (`origin/HEAD`) resolves to the commit it points
/// at instead of failing on a ref with no direct target.
fn find_source(repo: &Repository, name: &str) -> Result<(Oid, bool)> {
    if let Ok(local) = repo.find_branch(name, BranchType::Local)
        && let Ok(commit) = local.get().peel_to_commit()
    {
        return Ok((commit.id(), false));
    }
    if let Ok(remote) = repo.find_branch(name, BranchType::Remote)
        && let Ok(commit) = remote.get().peel_to_commit()
    {
        return Ok((commit.id(), true));
    }
    Err(GitError::NoSuchBranch {
        name: name.to_string(),
    })
}

/// The report, shared by all four exits.
///
/// `advanced` and `commits` count what the merge takes — the source's commits hidden behind
/// the pre-merge tip — even on the conflicted exit, where `old == new` and the diff totals are
/// honestly zero: the merge still brings those commits in once it is concluded, and the
/// sentence never shows the count while conflicts lead.
#[allow(clippy::too_many_arguments)]
fn report(
    repo: &Repository,
    source: &str,
    branch: &str,
    fast_forward: bool,
    old: Oid,
    new: Oid,
    source_oid: Oid,
    behind: u32,
    conflicts: Vec<String>,
) -> Result<MergeOutcome> {
    let old_tree = repo.find_commit(old).wrap()?.tree().wrap()?;
    let new_tree = repo.find_commit(new).wrap()?.tree().wrap()?;
    let (files_changed, insertions, deletions) = diff_totals(repo, &old_tree, &new_tree)?;
    let (commits, more_commits) = taken_commits(repo, source_oid, old, behind, PULL_COMMIT_CAP)?;
    Ok(MergeOutcome {
        source: source.to_string(),
        branch: branch.to_string(),
        fast_forward,
        old_oid: short_oid(old),
        new_oid: short_oid(new),
        advanced: behind,
        files_changed,
        insertions,
        deletions,
        commits,
        more_commits,
        conflicts,
    })
}

/// The title `git merge` writes.
///
/// The deliberate sibling of [`crate::pull`]'s `merge_message`, whose oracle is `git pull` —
/// see its doc for why the two must differ. This one reproduces `fmt-merge-msg` fed a local
/// ref instead of `FETCH_HEAD`:
///
/// * `Merge branch 'x'` for a local source, `Merge remote-tracking branch 'origin/x'` for a
///   remote-tracking one — the *kind* is in the sentence, which is why [`find_source`] has to
///   report which arm won.
/// * Never an ` of <url>` clause: that clause is `FETCH_HEAD`'s record of where the objects
///   came from, and a merge of a ref already in this repository came from nowhere.
/// * The ` into <branch>` clause is omitted for `master` and for `main` and for nothing else —
///   the same hardcoded `fmt-merge-msg.c` pair `pull.rs` documents (not `init.defaultBranch`),
///   pinned differentially in `tests/merge.rs` against the real binary.
///
/// No body: `merge.log` defaults to false.
fn merge_message(source: &str, source_is_remote: bool, into: &str) -> String {
    let kind = if source_is_remote {
        "remote-tracking branch"
    } else {
        "branch"
    };
    let mut out = format!("Merge {kind} '{source}'");
    if into != "master" && into != "main" {
        out.push_str(&format!(" into {into}"));
    }
    out.push('\n');
    out
}
