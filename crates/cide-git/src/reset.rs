//! `git reset`, and the preview that has to come before it. (M19)
//!
//! # Why [`preview`] is public
//!
//! For the same reason [`crate::branch::checkout_blockers`] is: the dialog has to be able to
//! say *"Hard — DISCARD 3 changed files"* **and name all three** before the user commits to
//! anything, and that answer has to be assertable against a real repository without performing
//! the reset. A confirmation that says "this will discard your changes" and cannot list them is
//! a confirmation people learn to click through, which makes it worse than none.
//!
//! It is one call and not five because every one of these answers costs a walk or a status
//! pass; a dialog that made five round trips to populate itself would paint five times and
//! could show a combination of states that never existed at once.
//!
//! # The ADR 0004 collision, and how it is answered
//!
//! Changelists are the truth and `.git/index` is derived, so a reset lands in the middle of
//! this crate's central compromise. Three consequences, none of them optional:
//!
//! **The sidecar's changelists need no edits.** Only *explicit* assignments are stored, so a
//! path made dirty again by a soft or mixed reset has no assignment and joins the active list —
//! which is exactly what IDEA does with the changes a reset gives back. Assignments that are
//! now dead (the path has no change any more) are swept by the `reconcile` trailer at the end
//! of [`reset`], the same one [`crate::commit::commit`] runs.
//!
//! **The one sidecar field a reset writes is `index_fingerprint`, and it is written on the
//! failure path too.** A mixed or hard reset rewrites `.git/index`; if the fingerprint is left
//! describing the index from before, the *very next commit* refuses with
//! [`cide_ipc::git::GitError::IndexChangedExternally`] and points the user's guard bar at
//! cide's own act. On the failure path this matters more rather than less, because a reset that
//! failed halfway is exactly when the index is least likely to be what anybody expects.
//!
//! **The external-staging guard fires for Mixed and Hard and not for Soft.**
//!
//! | kind | writes the index | guard | why |
//! | --- | --- | --- | --- |
//! | Soft | no | **no** | nothing staged is lost — the entries are merely staged against a different `HEAD` |
//! | Mixed | yes | yes | the clobber ADR 0004's first mitigation exists for |
//! | Hard | yes, and the worktree | yes | the same, plus the worktree, which the shelve-first offer covers |
//!
//! A guard that fires when nothing is at risk is a guard the user turns off, which is why Soft
//! is exempt rather than uniform.
//!
//! In `use_staging_area` mode [`crate::changelist::require_index_unchanged`] returns `Ok(())`
//! unconditionally — correct, because in that mode the index is the user's and a change to it
//! is not a collision. That makes the *dialog* load-bearing and asymmetric, which is why
//! [`ResetPreview::use_staging_area`] is on the wire: in changelist mode a mixed reset costs a
//! derived index, and in staging-area mode it destroys a hand-built `git add -p` selection.
//!
//! # No `force` that skips the in-progress check
//!
//! [`ResetRequest::force`] proceeds past the staging guard and a dirty tree. It deliberately
//! does **not** proceed past [`crate::repo::operation_in_progress`], and there is no flag that
//! does. A `--hard` during a rebase does not abort the rebase: it leaves the todo list pointing
//! at commits nothing can reach, which is strictly worse than the wedged state the user was
//! already in. `branchModel::explain` already says *"Finish or abort it first."*

use std::path::Path;

use cide_ipc::git::{GitError, PathSelection, PulledCommit};
use cide_ipc::history::{ResetKind, ResetOutcome, ResetPreview, ResetRequest};
use git2::{Commit, Oid, Repository, ResetType, Status, StatusOptions};

use crate::{Result, Wrap, changelist, repo as repo_mod, shelf, status};

/// How many dropped commits the preview describes one by one.
///
/// Ten, and the cap is here rather than in the dialog for the same reason
/// [`crate::branch`]'s `PULL_COMMIT_CAP` is: a reset over four hundred commits must not put
/// four hundred rows on the IPC wire for a component to `slice` down to ten. Everything past
/// it is still reported, as [`ResetPreview::more_dropped`].
const DROPPED_COMMIT_CAP: usize = 10;

/// What each kind of reset would discard, computed without touching anything.
pub fn preview(root: &Path, target: &str) -> Result<ResetPreview> {
    let repo = repo_mod::open(root)?;

    let head_info = status::branch_info(&repo)?;
    if head_info.unborn {
        // There is nothing to move and nothing to drop. Answered here rather than left to
        // produce an empty preview, because a dialog offering to reset an empty repository is
        // offering a no-op behind a red button.
        return Err(GitError::Unborn);
    }
    let head = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|_| GitError::Unborn)?;
    let target = resolve(&repo, target)?;

    // `graph_ahead_behind(local, upstream)` counts local-not-upstream first. Here that is
    // "reachable from HEAD and not from the target" — precisely what the reset drops.
    let (dropped_count, gained_count) = repo.graph_ahead_behind(head.id(), target.id()).wrap()?;

    let mut walk = repo.revwalk().wrap()?;
    walk.push(head.id()).wrap()?;
    walk.hide(target.id()).wrap()?;
    let mut dropped = Vec::new();
    for oid in walk.take(DROPPED_COMMIT_CAP) {
        let commit = repo.find_commit(oid.wrap()?).wrap()?;
        dropped.push(summarise(&commit));
    }

    let survey = Survey::take(&repo)?;
    let sidecar = changelist::load(root);

    Ok(ResetPreview {
        head: head_info.head,
        detached: head_info.detached,
        head_oid: head.id().to_string(),
        target_oid: target.id().to_string(),
        target_summary: target.summary().ok().flatten().unwrap_or("").to_string(),
        commits_dropped: dropped_count as u32,
        // Filled rather than left at zero: a reset *forward* — onto a commit HEAD cannot
        // reach — drops nothing at all, and a dialog whose only sentence was "0 commits would
        // be undone" would be describing the wrong half of what is about to happen.
        commits_gained: gained_count as u32,
        more_dropped: (dropped_count as u32).saturating_sub(dropped.len() as u32),
        dropped,
        shelvable: survey.shelvable(),
        staged: survey.staged,
        dirty: survey.dirty,
        untracked_kept: survey.untracked,
        use_staging_area: sidecar.use_staging_area,
    })
}

/// Move `HEAD` — and, per [`ResetRequest::kind`], the index and the working tree.
pub fn reset(root: &Path, request: &ResetRequest) -> Result<ResetOutcome> {
    let repo = repo_mod::open(root)?;

    // See the module header: there is no `force` past this, on purpose.
    if let Some(operation) = repo_mod::operation_in_progress(&repo) {
        return Err(GitError::OperationInProgress { operation });
    }
    // A conflicted index with no operation in progress — a stash apply that went wrong. The
    // reset would resolve it by fiat, in whichever direction the target happens to point, and
    // the user would have no record of what the two sides had been.
    let conflicts = repo_mod::conflicted_paths(&repo)?;
    if !conflicts.is_empty() {
        return Err(GitError::Conflicted { paths: conflicts });
    }
    let head_info = status::branch_info(&repo)?;
    if head_info.unborn {
        return Err(GitError::Unborn);
    }
    let head_before = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|_| GitError::Unborn)?;
    let target = resolve(&repo, &request.target)?;

    // Conditional on the kind, which is the whole of the table in the module header. `force`
    // is the second click on a dialog that has already listed what is at stake.
    if request.kind != ResetKind::Soft && !request.force {
        changelist::require_index_unchanged(root, &repo)?;
    }

    let (dropped_count, _) = repo
        .graph_ahead_behind(head_before.id(), target.id())
        .wrap()?;
    let head_before_oid = head_before.id();
    let target_oid = target.id();

    /*
     * Everything read from this handle is now a value, and the handle goes away.
     *
     * `shelf::shelve` rolls the working tree back through its own `Repository`, so anything
     * this one has cached about the index is stale the moment it returns. That is the crate's
     * standing rule — no handle outlives the operation that opened it — and here it is not a
     * style point: the reset below would be computed against an index that no longer exists.
     */
    drop(head_before);
    drop(target);
    drop(repo);

    /*
     * Shelve **before** the reset, and let a failed shelve stop it.
     *
     * `shelf::shelve` writes and fsyncs the patch file and only then rolls the working tree
     * back, so a crash between the two leaves the user with both copies rather than neither.
     * That ordering is only worth anything if the reset is downstream of it: shelving after
     * the reset would be shelving a working tree the reset had already emptied, and shelving
     * "in parallel" is not a thing a single-threaded sequence can do. So a shelve that fails
     * returns here, with the repository untouched.
     */
    let shelved = match &request.shelve_first {
        Some(name) => {
            let repo = repo_mod::open(root)?;
            let selections = Survey::take(&repo)?.shelvable();
            drop(repo);
            if selections.is_empty() {
                // Nothing to capture. Not an error: *Shelve them first* is a checkbox that
                // defaults to on, so a reset on a clean tree would otherwise fail with
                // `NothingToCommit` from a control the user never thought about.
                None
            } else {
                Some(shelf::shelve(root, name, &selections)?)
            }
        }
        None => None,
    };

    // Re-opened after the shelve for the reason above.
    let repo = repo_mod::open(root)?;
    // Counted here — after the shelve, before the reset — so it is what the reset itself
    // overwrote. Anything the shelf took is on the shelf, and calling that "discarded" in a
    // toast would be telling the user their work is gone when it is one click from coming
    // back.
    let files_discarded = if request.kind == ResetKind::Hard {
        Survey::take(&repo)?.dirty.len() as u32
    } else {
        0
    };

    let object = repo.find_object(target_oid, None).wrap()?;
    let outcome = repo.reset(&object, reset_type(request.kind), None);

    // On **both** paths. See the module header: a mixed or hard reset writes `.git/index`, and
    // a stale fingerprint makes the next commit refuse and blame the user for cide's write.
    if let Err(error) = outcome {
        let _ = changelist::record_index(root, &repo);
        return Err(error).wrap();
    }
    changelist::record_index(root, &repo)?;

    // No changelist edits: only explicit assignments are stored, so the paths this reset just
    // made dirty again have none and join the active list. What does have to happen is the
    // sweep of assignments whose path no longer has a change — the same trailer
    // `commit::commit` runs, and the reason a named list does not accumulate dead entries.
    let live = status::live_paths(root).unwrap_or_default();
    let _ = changelist::update(root, |data| {
        let _ = data.reconcile(&live);
        Ok(())
    });

    let head_after = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map(|commit| commit.id().to_string())
        .unwrap_or_else(|_| target_oid.to_string());

    Ok(ResetOutcome {
        kind: request.kind,
        head_before: head_before_oid.to_string(),
        head_after,
        commits_dropped: dropped_count as u32,
        files_discarded,
        shelved,
    })
}

/// One status pass, read three ways.
///
/// Taken once and shared by [`preview`] and [`reset`] so that the list the dialog showed and
/// the set the shelf captures cannot disagree — which is exactly how a shelf ends up holding a
/// different set of files from the one the user was just shown.
struct Survey {
    /// Paths whose index entry differs from `HEAD`. What a Mixed reset unstages.
    staged: Vec<String>,
    /// Tracked paths whose **working tree** content differs from `HEAD`. What a Hard reset
    /// overwrites.
    ///
    /// Deliberately wider than "modified since the index": a path that is staged and then
    /// untouched still has worktree content `HEAD` does not, and a `--hard` takes it away
    /// too. A list that omitted those would under-report what the red button costs.
    dirty: Vec<String>,
    /// How many untracked files are present — **counted, not listed**.
    ///
    /// `git reset --hard` leaves untracked files exactly where they are, and the most common
    /// fear at this dialog is that new files are about to vanish. "3 untracked files are not
    /// affected" answers that; listing them beside the files that *are* at stake would imply
    /// the opposite.
    untracked: u32,
}

impl Survey {
    fn take(repo: &Repository) -> Result<Self> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .include_unmodified(false);
        let statuses = repo.statuses(Some(&mut opts)).wrap()?;

        let mut staged = Vec::new();
        let mut dirty = Vec::new();
        let mut untracked = 0u32;
        for entry in statuses.iter() {
            let Ok(path) = entry.path() else {
                // Every type downstream is a `String`; inventing a lossy name here would let a
                // shelve target a file the user never saw. Same rule as `status::collect`.
                tracing::warn!("skipping a status entry with a non-UTF-8 path");
                continue;
            };
            let state = entry.status();
            if state.is_ignored() {
                continue;
            }
            // `WT_NEW` alone is untracked. `INDEX_NEW | WT_NEW` is a file that was `git add`ed
            // and then edited again, which is tracked as far as a reset is concerned — the
            // same distinction `branch::blockers_against_tree` draws.
            if state.contains(Status::WT_NEW) && !state.contains(Status::INDEX_NEW) {
                untracked += 1;
                continue;
            }
            dirty.push(path.to_string());
            if state.intersects(
                Status::INDEX_NEW
                    | Status::INDEX_MODIFIED
                    | Status::INDEX_DELETED
                    | Status::INDEX_RENAMED
                    | Status::INDEX_TYPECHANGE,
            ) {
                staged.push(path.to_string());
            }
        }
        staged.sort();
        dirty.sort();
        Ok(Self {
            staged,
            dirty,
            untracked,
        })
    }

    /// What [`ResetRequest::shelve_first`] would capture.
    ///
    /// The **tracked** half of [`Self::dirty`] and nothing else. Untracked files are excluded
    /// because shelving rolls a path back to `HEAD`, and for an untracked file that means
    /// *deleting* it — which would take away files the reset itself was going to leave alone.
    /// A "shelve first" that quietly removed the user's new scratch file would be the worst
    /// possible reading of a safety checkbox.
    ///
    /// [`PathSelection`]s and not bare paths because that is what the shelf takes, and
    /// rebuilding them at the call site would be a second place that decides what "everything
    /// dirty" means.
    fn shelvable(&self) -> Vec<PathSelection> {
        self.dirty
            .iter()
            .cloned()
            .map(PathSelection::whole)
            .collect()
    }
}

/// [`ResetKind`] as libgit2 spells it.
///
/// A free function and not a `From` impl: both types are foreign to this crate, so the orphan
/// rule rules the impl out — the same reason [`crate::Wrap`] exists.
fn reset_type(kind: ResetKind) -> ResetType {
    match kind {
        ResetKind::Soft => ResetType::Soft,
        ResetKind::Mixed => ResetType::Mixed,
        ResetKind::Hard => ResetType::Hard,
    }
}

fn summarise(commit: &Commit<'_>) -> PulledCommit {
    PulledCommit {
        short_oid: short_oid(commit.id()),
        summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
        author: commit.author().name().unwrap_or("").to_string(),
    }
}

/// A reset target, as a commit. `NoSuchCommit` on anything that does not peel to one — the
/// target comes from a log row, so there is no branch here to be "not found".
fn resolve<'repo>(repo: &'repo Repository, rev: &str) -> Result<Commit<'repo>> {
    repo.revparse_single(rev)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
        .ok_or_else(|| GitError::NoSuchCommit {
            rev: rev.to_string(),
        })
}

fn short_oid(oid: Oid) -> String {
    oid.to_string().chars().take(8).collect()
}
