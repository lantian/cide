//! Branches: listing them, switching between them, and the refusal that names files.
//!
//! # The refusal is the feature
//!
//! Every git front end can list branches. What separates a usable branch selector from a
//! dropdown is what happens when the switch cannot be made: `git checkout` prints the files
//! that are in the way, and IDEA turns that list into a choice — stash, or bring the changes
//! along. A UI that says *checkout failed* has thrown away the only part of the answer the
//! user can act on, and it is the failure this module is written around.
//!
//! So [`checkout_blockers`] is a public function of its own rather than a detail of
//! [`checkout`]. It answers "which paths would this overwrite" **without touching anything**,
//! which is what lets the popup ask before it acts, the tests assert the rule against a real
//! repository, and a create-and-switch refuse *before* the branch is created rather than
//! after.
//!
//! ## The rule it implements, which is git's
//!
//! A local change blocks a switch only when the file **differs between the two branches**.
//! That is the whole subtlety, and getting it wrong in either direction is bad: refuse on
//! every dirty file and the selector is useless in a working repository; refuse on none and
//! libgit2's own `SAFE` checkout fails later with a message the UI never asked for. Three
//! things block:
//!
//! * a tracked path that is modified/staged/deleted here **and** differs between HEAD and the
//!   target tree — the change would have nowhere to go;
//! * an untracked path that the target tree contains — the switch would write over a file git
//!   does not know about, which is unrecoverable;
//! * anything conflicted, because a half-finished merge is not a state to leave a branch in.
//!
//! A file you edited that is byte-identical on both branches is **not** in the list; it comes
//! along, exactly as `git switch` brings it.
//!
//! # No `--force`, anywhere
//!
//! [`CheckoutMode`] has `Refuse`, `Stash` and `StashAndRestore` and no fourth option.
//! Discarding uncommitted work is what `stage::rollback` is for, behind a confirmation that
//! says so; a status-bar dropdown is the last place it should be one click away. The stash
//! modes are not a lesser force — the work is in `git stash list` afterwards, findable by a
//! `git` the user runs themselves.
//!
//! # Network work shells out, on exactly the same rule as pushing
//!
//! [`fetch`] reuses [`push::route`]: a credential helper or an HTTP(S) remote means the `git`
//! binary does it, because libgit2 cannot drive an interactive helper. See `push.rs` for the
//! full argument — this module is the second caller of that decision, not a second copy of it.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use cide_ipc::git::{
    BranchInfo, BranchList, BranchRef, CheckoutMode, CheckoutOutcome, FetchOutcome, GitError,
    RepoInfo,
};
use git2::{BranchType, Commit, Oid, Repository, Status, StatusOptions, Tree};

use crate::{Result, Wrap, push, repo as repo_mod, stash, status};

// --- listing --------------------------------------------------------------------------------

/// Every branch of one repository, plus what `HEAD` is doing.
pub fn list(info: &RepoInfo) -> Result<BranchList> {
    let repo = repo_mod::open(&info.root)?;
    let head = status::branch_info(&repo)?;
    Ok(BranchList {
        repo: info.clone(),
        local: collect(&repo, BranchType::Local)?,
        remote: collect(&repo, BranchType::Remote)?,
        head,
    })
}

/// One side of the list, most recently committed first.
///
/// Recency, not alphabetical: a repository that has been alive for a year has hundreds of
/// branches and the one wanted next is nearly always among the handful last touched. The
/// popup pins the current branch to the top of what this returns — that is a display
/// decision and it lives in `ui/src/chrome/branchModel.ts` where it can be tested without a
/// repository.
fn collect(repo: &Repository, kind: BranchType) -> Result<Vec<BranchRef>> {
    let mut out = Vec::new();
    for entry in repo.branches(Some(kind)).wrap()? {
        let (branch, _) = entry.wrap()?;
        // `name()` fails on bytes that are not UTF-8. Every downstream type is a `String`,
        // and inventing a lossy name here would let a checkout target a branch whose name
        // the user never saw — the same reason `status::collect` skips such paths.
        let Some(name) = branch.name().ok().flatten().map(str::to_owned) else {
            tracing::warn!("skipping a branch with a non-UTF-8 name");
            continue;
        };
        // `origin/HEAD` is a symbolic ref pointing at another row in this same list. Showing
        // it offers a checkout of "whatever origin's default is", which is a second name for
        // a branch already listed.
        if kind == BranchType::Remote && name.ends_with("/HEAD") {
            continue;
        }
        let Some(oid) = branch.get().target() else {
            continue;
        };
        let commit = repo.find_commit(oid).wrap()?;

        let mut upstream = None;
        let (mut ahead, mut behind) = (0, 0);
        if kind == BranchType::Local
            && let Ok(tracking) = branch.upstream()
        {
            upstream = tracking.name().ok().flatten().map(str::to_owned);
            if let Some(other) = tracking.get().target()
                && let Ok((a, b)) = repo.graph_ahead_behind(oid, other)
            {
                ahead = a as u32;
                behind = b as u32;
            }
        }

        out.push(BranchRef {
            current: branch.is_head(),
            remote: kind == BranchType::Remote,
            name,
            upstream,
            ahead,
            behind,
            tip: short_oid(oid),
            subject: commit.summary().ok().flatten().unwrap_or("").to_string(),
            committed: commit.time().seconds(),
        });
    }
    // Ties broken by name so the order is total: two branches cut from the same commit have
    // the same committer time, and an unstable order makes the popup reshuffle on refresh.
    out.sort_by(|a, b| {
        b.committed
            .cmp(&a.committed)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

fn short_oid(oid: Oid) -> String {
    oid.to_string().chars().take(8).collect()
}

// --- the refusal ------------------------------------------------------------------------------

/// The paths a switch to `revision` would overwrite, sorted. Empty means the switch is safe.
///
/// Touches nothing. See the module header for the rule; the short version is that a local
/// change blocks only when the file differs between HEAD and `revision`.
pub fn checkout_blockers(root: &Path, revision: &str) -> Result<Vec<String>> {
    let repo = repo_mod::open(root)?;
    let target = resolve(&repo, revision)?;
    blockers(&repo, &target)
}

fn blockers(repo: &Repository, target: &Commit<'_>) -> Result<Vec<String>> {
    let head_tree = head_tree(repo);
    let target_tree = target.tree().wrap()?;

    // Every path that is not the same on both sides. Both the old and the new name go in, so
    // a rename between the branches blocks on either end.
    let mut differing: BTreeSet<String> = BTreeSet::new();
    let diff = repo
        .diff_tree_to_tree(head_tree.as_ref(), Some(&target_tree), None)
        .wrap()?;
    for delta in diff.deltas() {
        for file in [delta.old_file(), delta.new_file()] {
            if let Some(path) = file.path() {
                differing.insert(path.to_string_lossy().into_owned());
            }
        }
    }

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .include_unmodified(false);
    let statuses = repo.statuses(Some(&mut opts)).wrap()?;

    let mut out = BTreeSet::new();
    for entry in statuses.iter() {
        let Ok(path) = entry.path() else {
            continue;
        };
        let state = entry.status();
        if state.is_conflicted() {
            out.insert(path.to_string());
            continue;
        }
        // Untracked here means "on disk and in no index entry" — `WT_NEW` on its own.
        // `INDEX_NEW | WT_NEW` is a file that was `git add`ed and then edited again, which is
        // tracked as far as a checkout is concerned.
        if state.contains(Status::WT_NEW) && !state.contains(Status::INDEX_NEW) {
            // Only a problem if the target actually has a file there to write.
            if target_tree.get_path(Path::new(path)).is_ok() {
                out.insert(path.to_string());
            }
            continue;
        }
        if differing.contains(path) {
            out.insert(path.to_string());
        }
    }
    Ok(out.into_iter().collect())
}

/// `HEAD`'s tree, or `None` on an unborn branch — where there is no tree and every path in
/// the target is new.
fn head_tree(repo: &Repository) -> Option<Tree<'_>> {
    repo.head().ok().and_then(|head| head.peel_to_tree().ok())
}

// --- switching --------------------------------------------------------------------------------

/// What a checkout is aiming at.
enum Target<'repo> {
    /// A local branch; `refs/heads/<name>` already exists.
    Local { name: String, tip: Commit<'repo> },
    /// A remote-tracking branch with no local counterpart. Checking it out creates one, which
    /// is what `git switch feature` does when only `origin/feature` exists — and what IDEA's
    /// *Checkout* on a remote row means. Detaching HEAD onto the remote ref instead would put
    /// the user somewhere they cannot commit, from a control labelled *Checkout*.
    TrackRemote {
        remote: String,
        local: String,
        tip: Commit<'repo>,
    },
}

impl Target<'_> {
    fn tip(&self) -> &Commit<'_> {
        match self {
            Self::Local { tip, .. } | Self::TrackRemote { tip, .. } => tip,
        }
    }

    fn local_name(&self) -> &str {
        match self {
            Self::Local { name, .. } => name,
            Self::TrackRemote { local, .. } => local,
        }
    }
}

/// A branch name — local or remote-tracking — as a commit and a plan.
fn find<'repo>(repo: &'repo Repository, name: &str) -> Result<Target<'repo>> {
    if let Ok(branch) = repo.find_branch(name, BranchType::Local) {
        let tip = branch.get().peel_to_commit().wrap()?;
        return Ok(Target::Local {
            name: name.to_string(),
            tip,
        });
    }
    if let Ok(branch) = repo.find_branch(name, BranchType::Remote) {
        let tip = branch.get().peel_to_commit().wrap()?;
        // `origin/feature` → `feature`. The first segment is the remote name; a branch with
        // slashes in it (`origin/feature/login`) keeps the rest.
        let local = name
            .split_once('/')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_else(|| name.to_string());
        // Someone else may already have made the local branch since the list was drawn.
        if let Ok(existing) = repo.find_branch(&local, BranchType::Local) {
            return Ok(Target::Local {
                name: local,
                tip: existing.get().peel_to_commit().wrap()?,
            });
        }
        return Ok(Target::TrackRemote {
            remote: name.to_string(),
            local,
            tip,
        });
    }
    Err(GitError::NoSuchBranch {
        name: name.to_string(),
    })
}

/// Resolve anything a start point may be: a branch, a tag, an oid.
fn resolve<'repo>(repo: &'repo Repository, revision: &str) -> Result<Commit<'repo>> {
    if let Ok(target) = find(repo, revision) {
        return Ok(match target {
            Target::Local { tip, .. } | Target::TrackRemote { tip, .. } => tip,
        });
    }
    repo.revparse_single(revision)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
        .ok_or_else(|| GitError::NoSuchBranch {
            name: revision.to_string(),
        })
}

/// Switch to `name`, doing what `mode` says about local changes that stand in the way.
pub fn checkout(root: &Path, name: &str, mode: CheckoutMode) -> Result<CheckoutOutcome> {
    let repo = repo_mod::open(root)?;
    // A half-finished merge or rebase has index state that a switch would strand. git refuses
    // too, and this way the message names the operation instead of libgit2's error class.
    if let Some(operation) = repo_mod::operation_in_progress(&repo) {
        return Err(GitError::OperationInProgress { operation });
    }
    let target = find(&repo, name)?;

    let in_the_way = blockers(&repo, target.tip())?;
    let stashed = if in_the_way.is_empty() {
        None
    } else {
        match mode {
            CheckoutMode::Refuse => {
                return Err(GitError::CheckoutWouldOverwrite {
                    branch: name.to_string(),
                    paths: in_the_way,
                });
            }
            CheckoutMode::Stash | CheckoutMode::StashAndRestore => {
                // Untracked files are included, because an untracked file the target tree also
                // contains is one of the three things that blocks — leaving it behind would
                // stash the tracked half and then fail on the same switch.
                let message = format!("cide: switching to {name}");
                stash::save(root, &message, true)?;
                Some(message)
            }
        }
    };

    let created_from_remote = switch(&repo, &target)?;

    // Restore *after* the switch, which is the whole of "smart checkout": the changes land on
    // the new branch.
    //
    // `apply` then `drop`, never `pop`. libgit2's `stash_pop` drops the entry whenever the
    // apply *returned* successfully — and an apply that produced conflict markers returns
    // successfully. That is the one outcome where the stash must survive: the merge is a
    // three-way against the branch you just left, its inputs are by definition the files that
    // differ between the branches, and a conflicted working tree with the original nowhere is
    // the worst state this feature could leave anybody in. So conflicts are detected
    // explicitly and the entry is kept.
    let mut restore_failed = None;
    if stashed.is_some() && mode == CheckoutMode::StashAndRestore {
        match stash::apply(root, 0) {
            Err(error) => restore_failed = Some(error.to_string()),
            Ok(()) => {
                // A fresh handle: the apply rewrote `.git/index` underneath `repo`, whose
                // in-memory copy libgit2 would happily keep serving.
                let after = repo_mod::open(root)?;
                let conflicts = repo_mod::conflicted_paths(&after)?;
                if conflicts.is_empty() {
                    stash::drop_entry(root, 0)?;
                } else {
                    restore_failed = Some(format!(
                        "{} left conflict markers in {}; your changes are still in the stash",
                        stashed.as_deref().unwrap_or("the stash"),
                        conflicts.join(", ")
                    ));
                }
            }
        }
    }

    Ok(CheckoutOutcome {
        branch: target.local_name().to_string(),
        created_from_remote,
        stashed: if mode == CheckoutMode::StashAndRestore && restore_failed.is_none() {
            // Popped: there is no stash to tell the user about any more.
            None
        } else {
            stashed
        },
        restore_failed,
    })
}

/// Move the working tree and `HEAD`. Returns the remote branch a local one was created from.
fn switch(repo: &Repository, target: &Target<'_>) -> Result<Option<String>> {
    // Tree first, then `HEAD`: if the checkout fails, `HEAD` still names the branch whose
    // content is on disk. The other order leaves the two disagreeing, which is the state
    // `git status` calls "you have unstaged changes" against a branch you never edited.
    let mut builder = git2::build::CheckoutBuilder::new();
    builder.safe();
    repo.checkout_tree(target.tip().as_object(), Some(&mut builder))
        .wrap()?;

    let created = match target {
        Target::Local { .. } => None,
        Target::TrackRemote { remote, local, tip } => {
            let mut branch = repo.branch(local, tip, false).wrap()?;
            // Without this the new branch has no upstream, so the very next push would need
            // `--set-upstream` and the row would show no ahead/behind counts.
            branch.set_upstream(Some(remote)).wrap()?;
            Some(remote.clone())
        }
    };

    repo.set_head(&format!("refs/heads/{}", target.local_name()))
        .wrap()?;
    Ok(created)
}

// --- creating, renaming, deleting ---------------------------------------------------------------

/// Create `name` at `start_point` (or at `HEAD`). Does **not** switch to it.
///
/// Separate from [`checkout`] so a caller that wants both can run [`checkout_blockers`] first
/// and refuse before anything exists — a create-then-fail leaves a branch the user did not
/// ask for and has to clean up.
pub fn create(root: &Path, name: &str, start_point: Option<&str>) -> Result<()> {
    let repo = repo_mod::open(root)?;
    validate_name(name)?;
    if repo.find_branch(name, BranchType::Local).is_ok() {
        return Err(GitError::BranchExists {
            name: name.to_string(),
        });
    }
    let start = match start_point {
        Some(revision) => resolve(&repo, revision)?,
        None => repo
            .head()
            .and_then(|head| head.peel_to_commit())
            .map_err(|_| GitError::Unborn)?,
    };
    // `force: false`. Moving an existing branch is a different verb and a destructive one;
    // the check above turns it into a message rather than a silent reset of somebody's work.
    repo.branch(name, &start, false).wrap()?;
    Ok(())
}

/// Rename a local branch. Remote-tracking refs are not renameable — they are a cache of the
/// remote's names, and the next fetch would put the old one back.
pub fn rename(root: &Path, from: &str, to: &str) -> Result<()> {
    let repo = repo_mod::open(root)?;
    validate_name(to)?;
    let mut branch =
        repo.find_branch(from, BranchType::Local)
            .map_err(|_| GitError::NoSuchBranch {
                name: from.to_string(),
            })?;
    if repo.find_branch(to, BranchType::Local).is_ok() {
        return Err(GitError::BranchExists {
            name: to.to_string(),
        });
    }
    branch.rename(to, false).wrap()?;
    Ok(())
}

/// Delete a local branch.
///
/// Refuses the current branch, and refuses one whose commits are on no other branch unless
/// `force`. The second check is the one that matters: deleting a merged branch loses a name,
/// deleting an unmerged one loses commits, and only the user can say whether that is intended.
///
/// Deleting the branch **on the remote** is deliberately not here. It is a push of an empty
/// refspec, it is not undoable by the person who ran it, and it belongs behind a gesture that
/// says "delete on origin" rather than behind the same *Delete* as a local ref.
pub fn delete(root: &Path, name: &str, force: bool) -> Result<()> {
    let repo = repo_mod::open(root)?;
    let mut branch =
        repo.find_branch(name, BranchType::Local)
            .map_err(|_| GitError::NoSuchBranch {
                name: name.to_string(),
            })?;
    if branch.is_head() {
        return Err(GitError::BranchIsCurrent {
            name: name.to_string(),
        });
    }
    if !force && !is_merged(&repo, &branch)? {
        return Err(GitError::BranchNotMerged {
            name: name.to_string(),
        });
    }
    branch.delete().wrap()
}

/// Whether every commit on `branch` is reachable from `HEAD`.
///
/// This is `git branch -d`'s test, not `--merged`'s full survey: the question the delete
/// button has to answer is "would this lose commits from where I am standing", and walking
/// every ref to see whether some other branch happens to contain them is both slower and a
/// different question.
fn is_merged(repo: &Repository, branch: &git2::Branch<'_>) -> Result<bool> {
    let Some(tip) = branch.get().target() else {
        return Ok(true);
    };
    let Ok(head) = repo.head().and_then(|head| head.peel_to_commit()) else {
        // Unborn HEAD reaches nothing, so nothing is merged into it.
        return Ok(false);
    };
    if head.id() == tip {
        return Ok(true);
    }
    repo.graph_descendant_of(head.id(), tip).wrap()
}

/// Reject a name `git check-ref-format` would reject, before libgit2 does.
///
/// libgit2 answers with a generic reference error, and the popup has an input box that wants
/// to say "branch names cannot contain a space" while the user is still typing.
fn validate_name(name: &str) -> Result<()> {
    let full = format!("refs/heads/{name}");
    if name.trim().is_empty() || !git2::Reference::is_valid_name(&full) {
        return Err(GitError::InvalidBranchName {
            name: name.to_string(),
        });
    }
    Ok(())
}

// --- network ------------------------------------------------------------------------------------

/// `git fetch`, by the same two routes as [`push::push`].
pub fn fetch(root: &Path, remote: Option<&str>) -> Result<FetchOutcome> {
    let repo = repo_mod::open(root)?;
    let head = status::branch_info(&repo)?;
    let name = match remote {
        Some(name) => name.to_string(),
        None => push::default_remote(&repo, &head.upstream),
    };
    fetch_with(&repo, root, &name)
}

fn fetch_with(repo: &Repository, root: &Path, remote: &str) -> Result<FetchOutcome> {
    match push::route(repo, remote) {
        push::Route::Binary => {
            let mut command = Command::new("git");
            command.current_dir(root).arg("fetch").arg(remote);
            // Same reason as `push_via_binary`: these commands are synchronous, and a `git`
            // that blocks on a tty prompt the user cannot see freezes the window for ever.
            command.env("GIT_TERMINAL_PROMPT", "0");
            let output = command.output().wrap()?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !output.status.success() {
                return Err(GitError::Fetch { output: text });
            }
            Ok(FetchOutcome {
                remote: remote.to_string(),
                shelled_out: true,
                output: text,
                advanced: 0,
            })
        }
        push::Route::Libgit2 => {
            let mut handle = repo.find_remote(remote).wrap()?;
            let mut messages = String::new();
            {
                let mut callbacks = git2::RemoteCallbacks::new();
                callbacks.sideband_progress(|bytes| {
                    messages.push_str(&String::from_utf8_lossy(bytes));
                    true
                });
                let mut options = git2::FetchOptions::new();
                options.remote_callbacks(callbacks);
                // An empty refspec list means "the remote's configured ones", which is what
                // a bare `git fetch origin` does.
                handle
                    .fetch::<&str>(&[], Some(&mut options), None)
                    .map_err(|e| GitError::Fetch {
                        output: format!("{:?}: {}", e.class(), e.message()),
                    })?;
            }
            Ok(FetchOutcome {
                remote: remote.to_string(),
                shelled_out: false,
                output: messages,
                advanced: 0,
            })
        }
    }
}

/// Fetch, then fast-forward the current branch onto its upstream.
///
/// **Fast-forward only.** cide has no conflict-resolution surface — the diff panes are
/// read-only against HEAD — so a pull that has to merge would drop the user into a conflicted
/// working tree with nothing in the app able to finish it. A divergence is reported as
/// [`GitError::NotFastForward`] with both counts, which is the information needed to choose
/// between merge and rebase in a terminal that is one keystroke away.
pub fn pull(root: &Path, remote: Option<&str>) -> Result<FetchOutcome> {
    let outcome = fetch(root, remote)?;

    // Reopened after the fetch on purpose: the fetch wrote refs, and this crate's rule is
    // that no `Repository` handle outlives the operation that opened it.
    let repo = repo_mod::open(root)?;
    let head = status::branch_info(&repo)?;
    if head.unborn || head.detached {
        return Err(GitError::NoUpstream { branch: head.head });
    }
    let branch = repo
        .find_branch(&head.head, BranchType::Local)
        .map_err(|_| GitError::NoSuchBranch {
            name: head.head.clone(),
        })?;
    let upstream = branch.upstream().map_err(|_| GitError::NoUpstream {
        branch: head.head.clone(),
    })?;
    let (Some(local_oid), Some(remote_oid)) = (branch.get().target(), upstream.get().target())
    else {
        return Err(GitError::NoUpstream { branch: head.head });
    };

    if local_oid == remote_oid {
        return Ok(outcome);
    }
    let (ahead, behind) = repo.graph_ahead_behind(local_oid, remote_oid).wrap()?;
    if ahead > 0 {
        return Err(GitError::NotFastForward {
            branch: head.head,
            ahead: ahead as u32,
            behind: behind as u32,
        });
    }

    let target = repo.find_commit(remote_oid).wrap()?;
    let in_the_way = blockers(&repo, &target)?;
    if !in_the_way.is_empty() {
        // The same refusal as a checkout, because it is the same operation: moving the
        // working tree onto a different commit.
        return Err(GitError::CheckoutWouldOverwrite {
            branch: head.head,
            paths: in_the_way,
        });
    }

    let mut builder = git2::build::CheckoutBuilder::new();
    builder.safe();
    repo.checkout_tree(target.as_object(), Some(&mut builder))
        .wrap()?;
    repo.reference(
        &format!("refs/heads/{}", head.head),
        remote_oid,
        true,
        "cide: fast-forward pull",
    )
    .wrap()?;

    Ok(FetchOutcome {
        advanced: behind as u32,
        ..outcome
    })
}

/// The head of a repository, for callers that want the status bar's one line and nothing else.
///
/// A thin alias for [`status::branch_info`] so this module is the whole of the branch surface
/// rather than half of it.
pub fn head(root: &Path) -> Result<BranchInfo> {
    let repo = repo_mod::open(root)?;
    status::branch_info(&repo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_with_a_space_is_refused_before_libgit2_sees_it() {
        assert_eq!(
            validate_name("my branch"),
            Err(GitError::InvalidBranchName {
                name: "my branch".to_string()
            })
        );
        assert_eq!(
            validate_name(""),
            Err(GitError::InvalidBranchName {
                name: String::new()
            })
        );
        // `..` is what `git check-ref-format` rejects for revision-range ambiguity.
        assert!(validate_name("a..b").is_err());
        assert!(validate_name("feature/login").is_ok());
        assert!(validate_name("release-1.2").is_ok());
    }

    #[test]
    fn a_tip_is_abbreviated_to_eight_hex_digits() {
        let oid = Oid::from_str("0123456789abcdef0123456789abcdef01234567").expect("oid");
        assert_eq!(short_oid(oid), "01234567");
    }
}
