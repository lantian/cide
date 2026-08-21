//! Revert and cherry-pick: applying an existing commit's patch somewhere else. (M19)
//!
//! # One mechanism, one sign
//!
//! A revert and a cherry-pick differ in exactly one thing — which side of the three-way merge
//! the patch is read from — and libgit2 spells that difference as two functions that take the
//! same arguments. Everything else is shared: resolve a commit, pick a mainline for a merge,
//! compose a tree, refuse on conflict, write. So there is one private [`replay`] and two
//! one-line entry points, and [`cide_ipc::history::ReplayOp`] is the sign. Two implementations
//! would mean two of every refusal below, and a frontend that catches one of them and not the
//! other.
//!
//! # libgit2's stateful `revert` / `cherrypick` are rejected — for the clean path
//!
//! **Read `docs/adr/0009-real-sequencer-state.md` before undoing any of this.** The section
//! below is the original argument and it still holds for a replay that applies cleanly, which is
//! almost all of them. The conflicting path reverses it deliberately, and the ADR records why.
//!
//! [`git2::Repository::revert`] and [`git2::Repository::cherrypick`] do the whole job in one
//! call, and cide uses neither. They write `REVERT_HEAD` / `CHERRY_PICK_HEAD`, set
//! [`git2::RepositoryState`], and check the merged index out into `.git/index` and the working
//! tree — i.e. they leave the repository **half-finished**, waiting for a `--continue` that
//! this panel does not have a button for. Two things follow, and both are worse than the work
//! of composing the result by hand:
//!
//! * Under ADR 0004 `.git/index` is a *derived* artifact that cide rebuilds from the active
//!   changelist at commit time. A sequencer that plants state there is planting it in the one
//!   place the next commit is going to overwrite.
//! * The moment `RepositoryState` is non-clean, [`crate::repo::operation_in_progress`] starts
//!   refusing **every other action in this crate** — commit, checkout, reset, replay. A user
//!   who cherry-picked one commit would find the whole git surface locked, from a panel with
//!   nothing on it that can finish or abort the operation.
//!
//! So the result is composed by hand and **no sequencer state is ever written**. There is
//! nothing to clean up afterwards because nothing was created;
//! `nothing_leaves_a_sequencer_state_behind` in `tests/commit_actions.rs` asserts that after
//! every successful action, both by [`git2::Repository::state`] and by the absence of each
//! file.
//!
//! # The pre-check is the feature, and it is free
//!
//! [`git2::Repository::cherrypick_commit`] (git2 0.21 `src/repo.rs:3205`) and
//! [`git2::Repository::revert_commit`] (`src/repo.rs:3334`) perform the entire three-way merge
//! and hand back an **in-memory [`git2::Index`]**, writing nothing at all. With
//! [`git2::Index::has_conflicts`] and [`git2::Index::conflicts`] that answers *"would this
//! conflict, and where"* before a byte moves — the same shape of answer
//! [`crate::branch::checkout_blockers`] gives a branch switch. It used to decide whether to
//! *refuse*; since M20 it decides whether to write anything at all. See below.
//!
//! The only thing written before the point of no return is the merged tree itself
//! ([`git2::Index::write_tree_to`]), and that is a handful of unreferenced objects in the
//! object database: no ref names them, nothing observes them, and `git gc` removes them. It is
//! also unavoidable — the tree has to exist before it can be compared with `HEAD`'s or checked
//! out.
//!
//! # What happens when it *does* conflict (M20)
//!
//! It used to be a refusal — `ReplayWouldConflict`, naming the files and changing nothing — on
//! the argument that *cide has no conflict-resolution surface, so a refusal that names the files
//! is the only useful one*. That was true, and the refusal was the better of the two answers
//! available at the time. It is no longer: [`crate::conflict`] is the surface, so a refusal
//! would name the files and then send the user somewhere else to do the whole thing again.
//!
//! So the conflicting path now runs libgit2's stateful [`git2::Repository::cherrypick`] /
//! [`git2::Repository::revert`] after all, leaves `CHERRY_PICK_HEAD` or `REVERT_HEAD`, and hands
//! back the paths in [`cide_ipc::history::ReplayOutcome::conflicts`]. The two objections in the
//! section above are answered rather than ignored: [`crate::commit`] takes the staging-area arm
//! and concludes the operation with `cleanup_state`, and `operation_in_progress` is what the
//! panel's merge bar is drawn from rather than a blanket refusal.
//!
//! **The clean path is unchanged**, and that is the case that matters — it is almost every
//! cherry-pick anyone makes. It is still composed by hand, still creates no sequencer state, and
//! `nothing_leaves_a_sequencer_state_behind` still asserts so.
//!
//! # Where this is deliberately more permissive than `git`
//!
//! `git cherry-pick` requires a clean working tree and index outright. cide asks the narrower
//! question [`crate::branch::blockers_against_tree`] asks — *would this overwrite anything* —
//! so an edit to a file the replayed patch does not touch comes along, exactly as it comes
//! along across a branch switch. Refusing on any dirty file would make the log's context menu
//! useless in a repository somebody is working in, which is the only kind that has a history
//! worth looking at.

use std::path::Path;

use cide_ipc::git::{GitError, PulledCommit};
use cide_ipc::history::{ReplayMode, ReplayOp, ReplayOutcome, ReplayRequest};
use git2::{Commit, Oid, Repository, Tree};

use crate::{Result, Wrap, branch, changelist, repo as repo_mod, status};

/// Apply `request.commit`'s patch **inverted** onto `HEAD`.
pub fn revert(root: &Path, request: &ReplayRequest) -> Result<ReplayOutcome> {
    replay(root, ReplayOp::Revert, request)
}

/// Apply `request.commit`'s patch as it stands onto `HEAD`.
pub fn cherry_pick(root: &Path, request: &ReplayRequest) -> Result<ReplayOutcome> {
    replay(root, ReplayOp::CherryPick, request)
}

/// The order below is the design, and every step before the write touches nothing.
///
/// It is written as one straight line rather than as guard functions because the *sequence* is
/// what makes the guarantee: each refusal happens while the repository is still exactly as the
/// user left it, and the first thing that changes anything is the checkout in [`write`].
fn replay(root: &Path, op: ReplayOp, request: &ReplayRequest) -> Result<ReplayOutcome> {
    let repo = repo_mod::open(root)?;

    // 1. A half-finished merge, rebase or sequencer run. Refused rather than joined: the
    //    result would be committed against a HEAD the other operation is about to move, and
    //    that operation has a `--continue` cide cannot drive.
    if let Some(operation) = repo_mod::operation_in_progress(&repo) {
        return Err(GitError::OperationInProgress { operation });
    }
    // 2. Conflicted entries with no operation in progress — a stash apply that went wrong, a
    //    hand-run `read-tree -m`. The merge below would inherit them and report conflicts the
    //    replayed commit had nothing to do with.
    let conflicts = repo_mod::conflicted_paths(&repo)?;
    if !conflicts.is_empty() {
        return Err(GitError::Conflicted { paths: conflicts });
    }
    // 3. There is no `ours` side to merge against on an unborn branch, and no parent for the
    //    commit that would be written.
    let head = repo
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok())
        .ok_or(GitError::Unborn)?;

    // 4. `request.commit` is a full oid from a row the user clicked, not a revspec — but it
    //    can still be gone, because a `gc` or a rewrite may have happened since the row was
    //    drawn. `NoSuchCommit` and not `NoSuchBranch`: calling a commit a branch sends the
    //    reader looking for a ref that was never involved.
    let source = resolve(&repo, &request.commit)?;
    let oid = source.id().to_string();

    // 5. Mainline arithmetic, before libgit2 sees the number. See `mainline_of`.
    let mainline = mainline_of(&source, request.mainline)?;

    // 6. The whole three-way merge, in memory. Nothing on disk moves.
    let mut merged = match op {
        ReplayOp::CherryPick => repo.cherrypick_commit(&source, &head, mainline, None),
        ReplayOp::Revert => repo.revert_commit(&source, &head, mainline, None),
    }
    .wrap()?;
    if merged.has_conflicts() {
        /*
         * Conflicts, so the operation runs **for real** and is left for the resolver. (M20)
         *
         * This used to be `GitError::ReplayWouldConflict`, and the in-memory merge above was
         * thrown away — for the reason this module's header gave: there was nothing in the app
         * that could finish a half-applied cherry-pick, so a refusal naming the files was the
         * only useful answer. `cide_git::conflict` is that surface, so the refusal became the
         * lesser answer: it told the user which files were the problem and then made them go
         * and do the whole thing again somewhere else.
         *
         * The pre-check is not wasted. It is still what decides *whether* to write anything at
         * all, and the clean path below still composes the result by hand and creates no
         * sequencer state — which is the case that matters, because it is almost all of them.
         *
         * `mainline` is not passed on: libgit2's stateful calls take it in their options, and
         * `mainline_of` has already refused every value they would reject.
         */
        drop(merged);
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.safe();
        match op {
            ReplayOp::CherryPick => {
                let mut opts = git2::CherrypickOptions::new();
                opts.mainline(mainline).checkout_builder(checkout);
                repo.cherrypick(&source, Some(&mut opts)).wrap()?;
            }
            ReplayOp::Revert => {
                let mut opts = git2::RevertOptions::new();
                opts.mainline(mainline).checkout_builder(checkout);
                repo.revert(&source, Some(&mut opts)).wrap()?;
            }
        }
        // `ORIG_HEAD` so Abort has somewhere to go back to. libgit2 writes it for `merge` and
        // `rebase` and not for these two, and `cide_git::conflict::abort` resets to it.
        let _ = repo.reference("ORIG_HEAD", head.id(), true, "cide: replay");

        let index = repo.index().wrap()?;
        let conflicts = repo_mod::conflicts_of(&index)?;
        drop(index);
        let _ = changelist::record_index(root, &repo);
        let live = status::live_paths(root).unwrap_or_default();
        let _ = changelist::update(root, |data| {
            let _ = data.reconcile(&live);
            Ok(())
        });
        let message = message_for(op, &source, mainline)?;
        return Ok(ReplayOutcome {
            op,
            source: source.id().to_string(),
            // Nothing is committed yet: the user resolves, then presses Continue.
            created: String::new(),
            summary: message.lines().next().unwrap_or("").to_string(),
            files: conflicts.len() as u32,
            conflicts,
        });
    }

    let tree_oid = merged.write_tree_to(&repo).wrap()?;
    if tree_oid == head.tree_id() {
        // git's own *"the previous cherry-pick is now empty"*, made typed. The alternative is
        // an empty commit, which is a commit the user has to explain in review and which looks
        // exactly like a successful replay in the log.
        return Err(GitError::EmptyReplay { op, oid });
    }
    let tree = repo.find_tree(tree_oid).wrap()?;

    // 7. The same question a branch switch asks, about a tree no commit has. Asked last of the
    //    read-only checks because it is the only one whose answer depends on the merged tree.
    let in_the_way = branch::blockers_against_tree(&repo, &tree)?;
    if !in_the_way.is_empty() {
        return Err(GitError::CheckoutWouldOverwrite {
            branch: short_oid(source.id()),
            paths: in_the_way,
        });
    }

    let files = changed_files(&repo, &head, &tree)?;
    let message = message_for(op, &source, mainline)?;
    let summary = message.lines().next().unwrap_or("").to_string();

    // 8. The point of no return.
    let created = match write(&repo, request.mode, op, &tree, &head, &source, &message) {
        Ok(created) => created,
        Err(error) => {
            // The checkout may already have rewritten `.git/index`, so the fingerprint the
            // sidecar holds describes an index that no longer exists. Leaving it stale would
            // make the *next* commit refuse with `IndexChangedExternally`, pointing at cide's
            // own half-finished act. Same reasoning, same `let _`, as `commit::commit`.
            let _ = changelist::record_index(root, &repo);
            return Err(error);
        }
    };
    changelist::record_index(root, &repo)?;

    // The replayed paths either went into a commit, and so have no pending change any more, or
    // stayed in the working tree, and so are unfiled changes in the active list. Either way no
    // changelist needs *editing* — only explicit assignments are stored — but the assignments
    // that are now dead have to be swept. Same trailer as `commit::commit`, same reason.
    let live = status::live_paths(root).unwrap_or_default();
    let _ = changelist::update(root, |data| {
        let _ = data.reconcile(&live);
        Ok(())
    });

    Ok(ReplayOutcome {
        op,
        source: source.id().to_string(),
        created,
        summary,
        files,
        conflicts: Vec::new(),
    })
}

/// Move the working tree onto `tree`, and under [`ReplayMode::Commit`] `HEAD` with it.
///
/// Tree first, then `HEAD`, the same ordering [`crate::branch`] uses and for the same reason:
/// a failure between the two must leave `HEAD` describing what is actually on disk. The other
/// order gives a repository whose `git status` shows the whole replayed patch as an
/// uncommitted *inverse* change against a commit that already exists — the state hardest to
/// interpret and hardest to undo.
///
/// The checkout is `SAFE`, not forced. It cannot fail on a local change, because
/// `blockers_against_tree` has already established that nothing it will write is locally
/// modified; leaving it `SAFE` means that if that reasoning is ever wrong, libgit2 refuses
/// instead of overwriting.
fn write(
    repo: &Repository,
    mode: ReplayMode,
    op: ReplayOp,
    tree: &Tree<'_>,
    head: &Commit<'_>,
    source: &Commit<'_>,
    message: &str,
) -> Result<String> {
    let mut builder = git2::build::CheckoutBuilder::new();
    builder.safe();
    repo.checkout_tree(tree.as_object(), Some(&mut builder))
        .wrap()?;

    if mode == ReplayMode::WorkingTree {
        /*
         * IDEA's *do not commit*, and `git revert -n` / `git cherry-pick -n`.
         *
         * `HEAD` does not move and **no sequencer file is written** — which is the one place
         * this differs from git, whose `-n` still writes `CHERRY_PICK_HEAD` so that a later
         * `git commit` can pick up the authorship. cide has no such handoff: the change lands
         * as ordinary unfiled changes in the active changelist and the commit panel is what
         * commits it, with the user's own message. Writing the file to imitate git would buy
         * nothing and would lock every other action in this crate behind
         * `operation_in_progress` until somebody ran a `git` command by hand.
         *
         * The checkout above updated `.git/index` for the paths it wrote, so the change is
         * staged — exactly what `git revert -n` leaves behind.
         */
        return Ok(String::new());
    }

    let committer = repo.signature().wrap()?;
    // A cherry-pick keeps the original author, with the person running it as committer: that
    // is what `git cherry-pick` does, and it is what stops a replayed commit from silently
    // claiming somebody else's work. A revert is a new decision by the person making it, so
    // both sides are the runner — also git's behaviour.
    let author = match op {
        ReplayOp::CherryPick => source.author(),
        ReplayOp::Revert => committer.clone(),
    };
    let oid = repo
        .commit(Some("HEAD"), &author, &committer, message, tree, &[head])
        .wrap()?;
    Ok(oid.to_string())
}

/// Which parent libgit2 should treat as the mainline, 1-based, or `0` for "not a merge".
///
/// `0` is libgit2's own encoding of absent (`cherrypick.c:134`, `revert.c:135`), which is why
/// this returns a `u32` rather than an `Option`.
///
/// Every branch here is a refusal git also makes, and the reason for making them *here* rather
/// than letting libgit2 make them is that libgit2's are strings: `"mainline branch is not
/// specified but %s is a merge commit"` is true and unusable — the dialog needs the parents
/// themselves, because *which side do you want to keep* is not a question anybody can answer
/// from a count.
fn mainline_of(source: &Commit<'_>, requested: Option<u32>) -> Result<u32> {
    let parents = source.parent_count();
    let oid = source.id().to_string();

    if parents > 1 {
        let Some(mainline) = requested else {
            return Err(GitError::MergeNeedsMainline {
                oid,
                parents: parent_summaries(source),
            });
        };
        if mainline == 0 || mainline as usize > parents {
            /*
             * Out of range is refused here rather than passed on, because libgit2 answers it
             * with a bare `git_commit_parent` failure that surfaces as `GitError::Git` — a
             * sentence about our internals attached to a button the user pressed.
             *
             * Reported as `MergeNeedsMainline` and not as a variant of its own: the two states
             * are "you have not told me which side to keep" and "the side you named does not
             * exist", and the dialog's correct response to both is identical — offer these
             * parents, by name. A separate variant would be a second arm in every `explain`
             * that rendered the same sentence.
             */
            return Err(GitError::MergeNeedsMainline {
                oid,
                parents: parent_summaries(source),
            });
        }
        return Ok(mainline);
    }

    if requested.is_some() {
        // git's own refusal. Silently ignoring the argument would make a mis-wired button look
        // like it worked, and the resulting commit would be indistinguishable from a correct
        // one until somebody read the diff.
        return Err(GitError::NotAMerge { oid });
    }
    Ok(0)
}

/// The parents of a merge, in git's `-m` order, as the four fields a chooser needs.
fn parent_summaries(source: &Commit<'_>) -> Vec<PulledCommit> {
    source
        .parents()
        .map(|parent| PulledCommit {
            short_oid: short_oid(parent.id()),
            // Same `.ok().flatten()` as `branch::collect`: both refuse non-UTF-8 bytes rather
            // than mangling them, and the short oid beside an empty summary is still enough to
            // run `git show`.
            summary: parent.summary().ok().flatten().unwrap_or("").to_string(),
            author: parent.author().name().unwrap_or("").to_string(),
        })
        .collect()
}

/// The message the new commit carries.
///
/// A cherry-pick keeps the original verbatim. A revert gets git's own generated body, which is
/// reproduced here character for character from `sequencer.c`'s `do_pick_commit`:
///
/// ```text
/// Revert "<subject>"
///
/// This reverts commit <full oid>.
/// ```
///
/// …with `, reversing\nchanges made to <mainline parent oid>` inserted before the full stop
/// when the reverted commit is a merge. That clause is git's and is reproduced because the
/// differential tests in `tests/commit_actions.rs` compare this body against what the real
/// `git revert` writes, and a message that is *nearly* git's is the kind of difference that
/// only shows up in somebody's release notes.
///
/// `<subject>` is the **first line** of the message, which is git's `find_commit_subject`.
/// Deliberately not [`git2::Commit::summary`], which folds a wrapped subject onto one line: the
/// two agree for every ordinary commit and disagree for exactly the commits whose message
/// somebody hand-wrapped.
fn message_for(op: ReplayOp, source: &Commit<'_>, mainline: u32) -> Result<String> {
    let text = message_text(source);
    match op {
        ReplayOp::CherryPick => Ok(text),
        ReplayOp::Revert => {
            let subject = text.lines().next().unwrap_or("");
            let mut out = format!(
                "Revert \"{subject}\"\n\nThis reverts commit {}",
                source.id()
            );
            if mainline > 0 {
                let parent = source.parent(mainline as usize - 1).wrap()?;
                out.push_str(&format!(", reversing\nchanges made to {}", parent.id()));
            }
            out.push_str(".\n");
            Ok(out)
        }
    }
}

/// A commit's message as text.
///
/// Lossy rather than refusing, and rather than [`git2::Commit::message`]'s `Option`, which is
/// `None` for anything that is not UTF-8. A Latin-1 commit message is rare and a cherry-pick
/// that silently produced an **empty** message for one would be worse than a replacement
/// character: the message is the only part of a cherry-picked commit the reviewer reads.
/// [`git2::Repository::commit`] takes a `&str`, so there is no byte-exact path available here
/// short of building the commit buffer by hand.
fn message_text(commit: &Commit<'_>) -> String {
    String::from_utf8_lossy(commit.message_bytes()).into_owned()
}

/// How many files the replay changes, relative to `HEAD`.
fn changed_files(repo: &Repository, head: &Commit<'_>, tree: &Tree<'_>) -> Result<u32> {
    let head_tree = head.tree().wrap()?;
    let diff = repo
        .diff_tree_to_tree(Some(&head_tree), Some(tree), None)
        .wrap()?;
    Ok(diff.deltas().len() as u32)
}

/// A full oid, as a commit. `NoSuchCommit` on anything that does not peel to one.
fn resolve<'repo>(repo: &'repo Repository, rev: &str) -> Result<Commit<'repo>> {
    repo.revparse_single(rev)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
        .ok_or_else(|| GitError::NoSuchCommit {
            rev: rev.to_string(),
        })
}

/// Eight hex digits, the width every other short oid in this crate uses.
fn short_oid(oid: Oid) -> String {
    oid.to_string().chars().take(8).collect()
}
