//! Pull: fast-forward, merge and rebase, and the ladder that decides which. (M20)
//!
//! # What changed, and why the decision in `replay.rs` does not apply here
//!
//! [`crate::replay`]'s header rejects libgit2's stateful `revert`/`cherrypick` — and by
//! extension `merge` and `rebase` — for two reasons. The first, that `.git/index` is a derived
//! artifact under ADR 0004, is real and is answered in [`crate::commit`]: a commit made while
//! `MERGE_HEAD` exists takes the staging-area arm regardless of the sidecar, because on a merge
//! the index *is* the truth and there is nothing for a changelist to contribute.
//!
//! The second reason was the load-bearing one:
//!
//! > The moment `RepositoryState` is non-clean, [`crate::repo::operation_in_progress`] starts
//! > refusing **every other action in this crate** — from a panel with nothing on it that can
//! > finish or abort the operation.
//!
//! That panel now exists. [`crate::conflict`] is the surface, `MergeState` is what it draws
//! from, and the sequencer's own `--continue` and `--abort` are two buttons. So the decision
//! reverses **with** its reason, and it reverses only for the operations that need it. See
//! `docs/adr/0009-real-sequencer-state.md`, which exists because reverting this by reflex from
//! `replay.rs`'s header is the likeliest way it gets undone.
//!
//! Real git state is what makes a conflict *resumable across an app restart*, legible to
//! `git status` in a terminal pane, and abandonable with `git merge --abort` by someone who
//! would rather not use the resolver at all. A cide-private conflict model would be none of
//! those, and a rebase's "conflict on commit 3 of 7" would force us to reimplement the
//! sequencer anyway.
//!
//! # Where cide is deliberately narrower than git
//!
//! * **A merge commit among the commits a rebase would replay is refused**, not flattened.
//!   `git rebase` drops it silently; the resolutions recorded inside it go with it. See
//!   [`GitError::RebaseWouldDropMerges`].
//! * **No `--rebase=merges` and no interactive rebase.** Both are sub-modes of the thing above.
//! * **No autostash.** A pull that would overwrite local changes refuses by naming them, the
//!   same refusal a checkout makes, computed by the same [`crate::branch::blockers_against_tree`].
//!
//! # The order of the ladder
//!
//! `branch.<name>.rebase`, then `pull.rebase`, then cide's own [`PullDefault`]. The
//! repository's own configuration wins over the application's, because `pull.rebase` is a fact
//! about a project's workflow — often set by whoever set the repository up — and a global
//! preference that silently overrode it would make cide disagree with the `git pull` typed into
//! a pane two lines below.

use std::path::Path;

use cide_core::proxy::ProxyEnv;
use cide_ipc::git::{
    Divergence, FetchOutcome, GitError, PullDefault, PullRequest, PullStrategy, PulledCommit,
};
use git2::{BranchType, ConfigLevel, ErrorCode, Oid, Repository, Tree};

use crate::{Result, Wrap, branch, changelist, push, repo as repo_mod, status};

/// How many of a pull's commits are described one by one.
///
/// Ten, because the surface is a toast: it is the number that fits without the notice becoming
/// a scrolling panel, and everything past it is still reported as a count. The cap is here
/// rather than in the frontend so that a fortnight away — four hundred commits — does not put
/// four hundred rows on the IPC wire for a component to `slice` down to ten.
pub const PULL_COMMIT_CAP: usize = 10;

/// How many local commits cide will replay in one rebase.
///
/// The cost is one three-way merge and one tree write per commit, which is the same work
/// `git rebase` does — so this is not a claim about being slow. It is a claim about a gesture
/// with no progress meter and no cancel: the whole replay runs on one blocking-pool call. A
/// hundred commits is a long-lived branch and about a second; past that the honest answer is
/// "merge, or use a terminal that has a Ctrl-C".
///
/// Refused up front and **never truncated**: a truncated rebase is a rewrite that lost commits.
pub const REBASE_COMMIT_CAP: usize = 100;

// --- entry points -------------------------------------------------------------------------

/// Fetch, then integrate, deciding everything from configuration.
///
/// The three-argument shape `branch::pull` had, kept so a caller with nothing to say stays one
/// line — and so the tests that predate strategies keep asserting what they assert.
pub fn pull(root: &Path, remote: Option<&str>, proxy: &ProxyEnv) -> Result<FetchOutcome> {
    pull_with(
        root,
        &PullRequest {
            remote: remote.map(str::to_string),
            ..PullRequest::default()
        },
        PullDefault::default(),
        proxy,
    )
}

/// Fetch, then integrate per `request`, falling back to `fallback` when git config is silent.
///
/// `fallback` is a value and not a lookup, for exactly the reason [`crate::push::push`] takes a
/// [`ProxyEnv`]: this crate reads no cide configuration, and reaching for the app's settings
/// here would give `cide-git` a dependency on the crate that owns them.
///
/// The order below is the design, and it is written as one straight line rather than as guard
/// functions because the *sequence* is what makes the guarantee — every refusal happens while
/// the repository is exactly as the user left it, and two of them happen before the network is
/// touched at all.
pub fn pull_with(
    root: &Path,
    request: &PullRequest,
    fallback: PullDefault,
    proxy: &ProxyEnv,
) -> Result<FetchOutcome> {
    let repo = repo_mod::open(root)?;

    /*
     * 1-2. A half-finished operation, and an index with unresolved entries.
     *
     * `branch::pull` checked neither, and every other mutating entry point in this crate does
     * — `replay` steps 1 and 2, `worktree::integrate`, `branch::checkout`, `reset`. The
     * symptom was that a pull run during a rebase spent a network round trip and then failed
     * inside `checkout_tree` with a raw `GitError::Git`.
     *
     * **Before the fetch**, which is the half that is not merely tidiness: a doomed pull must
     * not cost the user a network round trip, and the refusal is just as true beforehand.
     */
    if let Some(operation) = repo_mod::operation_in_progress(&repo) {
        return Err(GitError::OperationInProgress { operation });
    }
    let unresolved = repo_mod::conflicted_paths(&repo)?;
    if !unresolved.is_empty() {
        return Err(GitError::Conflicted { paths: unresolved });
    }

    // 3. The remote, resolved once. `NoRemote` is raised here rather than inside the fetch so
    //    that `skip_fetch` cannot bypass it — an answer to a dialog must fail the same way a
    //    first click would.
    let head = status::branch_info(&repo)?;
    let remote_name = match &request.remote {
        Some(name) => name.clone(),
        None => push::default_remote(&repo, &head.upstream),
    };
    if repo.find_remote(&remote_name).is_err() {
        return Err(GitError::NoRemote { name: remote_name });
    }

    // 4. The fetch, unless the caller is answering a `PullNeedsStrategy` and the refs it
    //    described are still the ones on disk. See `PullRequest::skip_fetch`.
    let outcome = if request.skip_fetch {
        // Honestly empty rather than reconstructed: no transport ran, so there is no transport
        // text, and inventing one would put a sentence in a toast that nothing said.
        FetchOutcome::fetched(remote_name.clone(), false, String::new())
    } else {
        branch::fetch_with(&repo, root, &remote_name, proxy)?
    };

    // 5. Reopened after the fetch on purpose: it wrote refs, and this crate's rule is that no
    //    `Repository` handle outlives the operation that opened it.
    let repo = repo_mod::open(root)?;
    let head = status::branch_info(&repo)?;

    /*
     * 6. Three states, three sentences. These used to collapse into `NoUpstream`, which meant a
     * detached HEAD was told `a1b2c3d4 has no upstream branch to pull from` — a sentence that
     * calls a commit a branch and points at the wrong fix, since no `--set-upstream` will help
     * someone who is not on a branch at all. An unborn branch had the same problem in reverse:
     * there is no commit to fast-forward, which `Unborn` already says in one word.
     */
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
    let upstream = local_branch.upstream().map_err(|_| GitError::NoUpstream {
        branch: head.head.clone(),
    })?;
    let upstream_name = upstream
        .name()
        .ok()
        .flatten()
        .unwrap_or(&remote_name)
        .to_string();
    let (Some(local_oid), Some(remote_oid)) =
        (local_branch.get().target(), upstream.get().target())
    else {
        return Err(GitError::NoUpstream { branch: head.head });
    };

    if local_oid == remote_oid {
        // Nothing came down — but the branch is still worth naming. *"main is already up to
        // date with origin"* is a different sentence from a bare fetch's *"already up to
        // date"*, and in a project with four repositories it is the only thing that says
        // which of them just answered.
        return Ok(FetchOutcome {
            branch: head.head,
            old_oid: short_oid(local_oid),
            new_oid: short_oid(local_oid),
            ..outcome
        });
    }

    let (ahead, behind) = repo.graph_ahead_behind(local_oid, remote_oid).wrap()?;

    /*
     * 7. `behind == 0` is *already up to date*, whatever `ahead` says.
     *
     * The upstream is an ancestor of the branch: nothing came down, and `git pull` answers
     * "Already up to date." The old test was `ahead > 0` alone, so a branch merely *ahead* of
     * its upstream — every branch with an unpushed commit and a quiet remote — was told it had
     * diverged and handed two counts to explain why it could not fast-forward.
     */
    if behind == 0 {
        return Ok(FetchOutcome {
            branch: head.head,
            old_oid: short_oid(local_oid),
            new_oid: short_oid(local_oid),
            ..outcome
        });
    }

    if ahead == 0 {
        return fast_forward(
            &repo,
            root,
            &head.head,
            local_oid,
            remote_oid,
            behind as u32,
        )
        .map(|report| FetchOutcome {
            strategy: Some(PullStrategy::FastForward),
            ..report.into_outcome(outcome)
        });
    }

    /*
     * 8. Unrelated histories, once, before either strategy.
     *
     * git's own *refusing to merge unrelated histories*. Asked here rather than left to
     * libgit2 because both strategies fail on it deep inside, and both surface it as
     * `GitError::Git` — a class name printed at somebody who pressed Pull.
     */
    if repo.merge_base(local_oid, remote_oid).is_err() {
        return Err(GitError::UnrelatedHistories {
            branch: head.head,
            upstream: upstream_name,
        });
    }

    // 9. Which strategy, and the dialog if nothing says.
    let strategy = request
        .strategy
        .or_else(|| configured_strategy(&repo, &head.head))
        .or_else(|| fallback.strategy());

    let Some(strategy) = strategy else {
        let (incoming, more_incoming) =
            taken_commits(&repo, remote_oid, local_oid, behind as u32, PULL_COMMIT_CAP)?;
        let (local, more_local) =
            taken_commits(&repo, local_oid, remote_oid, ahead as u32, PULL_COMMIT_CAP)?;
        return Err(GitError::PullNeedsStrategy(Box::new(Divergence {
            branch: head.head,
            remote: remote_name,
            upstream: upstream_name,
            ahead: ahead as u32,
            behind: behind as u32,
            incoming,
            more_incoming,
            local,
            more_local,
        })));
    };

    if strategy == PullStrategy::FastForward {
        return Err(GitError::NotFastForward {
            branch: head.head,
            ahead: ahead as u32,
            behind: behind as u32,
        });
    }

    // Dropped before the integration: `local_branch` and `upstream` borrow `repo`, and the two
    // strategies need it mutably in places.
    drop(upstream);
    drop(local_branch);

    let result = match strategy {
        PullStrategy::Merge => merge_pull(
            &repo,
            root,
            &head.head,
            local_oid,
            remote_oid,
            &remote_name,
            &upstream_name,
            behind as u32,
        ),
        PullStrategy::Rebase => rebase_pull(
            &repo,
            root,
            &head.head,
            local_oid,
            remote_oid,
            ahead as u32,
            behind as u32,
        ),
        PullStrategy::FastForward => unreachable!("returned above"),
    };

    let report = result?;

    /*
     * 10. Remember, once the strategy has actually been applied.
     *
     * Which includes a pull that landed conflicts: the user answered the question either way,
     * and a merge that needs resolving is the strategy working rather than failing. It is not
     * reached when the pull was *refused* before integrating — remembering an answer that never
     * ran would pin the user to a strategy with no dialog left to change it.
     */
    if request.remember {
        remember_strategy(&repo, strategy)?;
    }

    Ok(FetchOutcome {
        strategy: Some(strategy),
        ..report.into_outcome(outcome)
    })
}

// --- the config ladder --------------------------------------------------------------------

/// What git config says about this branch, or `None` when it says nothing.
///
/// `branch.<name>.rebase` first, then `pull.rebase`, each through the repository's normal
/// multi-level lookup — [`git2::Config::get_entry`] reports the highest-priority entry, and
/// that *is* the precedence rule, so a repo-local value beats a global one without this
/// function knowing what a level is.
///
/// Deliberately not `multivar`, which [`crate::push`] uses to find a credential helper: its
/// question is *"is there any at all"* and this one is *"which one wins"*. They are different
/// questions and reading one with the other's tool is how a `/etc/gitconfig` default silently
/// outranks the repository.
fn configured_strategy(repo: &Repository, branch: &str) -> Option<PullStrategy> {
    let config = repo.config().ok()?;
    for key in [format!("branch.{branch}.rebase"), "pull.rebase".to_string()] {
        if let Ok(entry) = config.get_entry(&key)
            && let Some(strategy) = parse_rebase(&entry)
        {
            return Some(strategy);
        }
    }
    None
}

/// `pull.rebase`'s value, by git's own rules.
///
/// Reproduces `builtin/pull.c::rebase_parse_value`, and every clause of it is load-bearing:
///
/// * the boolean set is git's, not Rust's — `true`/`yes`/`on` and **any non-zero integer** are
///   true, `false`/`no`/`off`/`0` and the *empty string* are false;
/// * a **valueless** key — `[pull]` followed by a bare `rebase` line — is `true`, and
///   [`git2::Config::get_string`] cannot see the difference between that and a missing key.
///   [`git2::ConfigEntry::has_value`] is the only way to ask;
/// * `merges`/`m` and `interactive`/`i` are rebases. They are sub-modes cide does not
///   implement, and the one place that matters — a merge commit among the replayed set —
///   refuses by name with [`GitError::RebaseWouldDropMerges`]. Quietly doing a plain rebase for
///   somebody who wrote `merges` is exactly what that refusal exists to prevent;
/// * anything else — `preserve`, removed from git in 2.34, or a typo — is **unconfigured**,
///   with a warning naming the value. git dies on it; cide cannot die, and quietly choosing
///   merge would be a lie about the user's own file. Unconfigured means the dialog opens,
///   which is the one place a person can see that something is wrong.
fn parse_rebase(entry: &git2::ConfigEntry<'_>) -> Option<PullStrategy> {
    if !entry.has_value() {
        return Some(PullStrategy::Rebase);
    }
    let value = entry.value().ok()?.trim().to_ascii_lowercase();
    match value.as_str() {
        "true" | "yes" | "on" => Some(PullStrategy::Rebase),
        "false" | "no" | "off" | "" => Some(PullStrategy::Merge),
        "merges" | "m" | "interactive" | "i" => Some(PullStrategy::Rebase),
        other => {
            if let Ok(n) = other.parse::<i64>() {
                return Some(if n == 0 {
                    PullStrategy::Merge
                } else {
                    PullStrategy::Rebase
                });
            }
            tracing::warn!(
                key = entry.name().unwrap_or("pull.rebase"),
                value = other,
                "unrecognised pull.rebase value; treating this branch as unconfigured"
            );
            None
        }
    }
}

/// Record the answer as `pull.rebase` in **this repository's own** config.
///
/// [`git2::Config::open_level`] rather than a bare `set_bool` on `repo.config()`. The two agree
/// for an ordinary checkout: `repo.config()` is the live multi-level config, not a snapshot,
/// and `git_config_set_*` resolves to the highest-priority writable backend, which is
/// `.git/config` — which is why [`crate::push`]'s `set_branch_upstream` has always landed
/// there. They disagree for a repository with `extensions.worktreeConfig`, where backend 0 is
/// `config.worktree` and a remembered answer would apply to one worktree and not the project.
/// Naming the level is a comment the compiler checks.
///
/// `pull.rebase` and not `branch.<name>.rebase`: the checkbox says *remember this choice*,
/// which a user means about the project, and a per-branch key would have to be answered again
/// on every branch they create.
fn remember_strategy(repo: &Repository, strategy: PullStrategy) -> Result<()> {
    let mut local = repo
        .config()
        .wrap()?
        .open_level(ConfigLevel::Local)
        .wrap()?;
    local
        .set_bool("pull.rebase", strategy == PullStrategy::Rebase)
        .wrap()
}

// --- the report ---------------------------------------------------------------------------

/// What an integration did, before it is folded into the fetch's half of the answer.
struct Integrated {
    old_oid: Oid,
    new_oid: Oid,
    branch: String,
    advanced: u32,
    rewritten: u32,
    skipped: u32,
    files_changed: u32,
    insertions: u32,
    deletions: u32,
    commits: Vec<PulledCommit>,
    more_commits: u32,
    conflicts: Vec<String>,
}

impl Integrated {
    fn into_outcome(self, base: FetchOutcome) -> FetchOutcome {
        FetchOutcome {
            branch: self.branch,
            old_oid: short_oid(self.old_oid),
            new_oid: short_oid(self.new_oid),
            advanced: self.advanced,
            rewritten: self.rewritten,
            skipped: self.skipped,
            files_changed: self.files_changed,
            insertions: self.insertions,
            deletions: self.deletions,
            commits: self.commits,
            more_commits: self.more_commits,
            conflicts: self.conflicts,
            ..base
        }
    }
}

/// `git diff --shortstat` between two trees.
///
/// The totals across the whole integration, **not** summed per commit: the question a pull
/// answers is *"what is different about my tree now"*, and summing per-commit stats would count
/// a file three commits touched three times.
///
/// Always old-local-tree → final-tree, and never merge-base → upstream. For a merge those are
/// different sets, and the first one is the answer to the question that was asked.
pub(crate) fn diff_totals(
    repo: &Repository,
    from: &Tree<'_>,
    to: &Tree<'_>,
) -> Result<(u32, u32, u32)> {
    let stats = repo
        .diff_tree_to_tree(Some(from), Some(to), None)
        .wrap()?
        .stats()
        .wrap()?;
    Ok((
        stats.files_changed() as u32,
        stats.insertions() as u32,
        stats.deletions() as u32,
    ))
}

/// The commits reachable from `tip` but not from `hidden`, newest first, capped.
///
/// `total` is passed in rather than recomputed: [`git2::Repository::graph_ahead_behind`] has
/// already walked exactly this set, so the overflow count is arithmetic rather than a second
/// walk.
///
/// Used for both halves of a divergence — what came down, and what a rebase would replay — so
/// the dialog and the outcome describe their sets with the same cap and cannot disagree about
/// how many there were.
pub(crate) fn taken_commits(
    repo: &Repository,
    tip: Oid,
    hidden: Oid,
    total: u32,
    cap: usize,
) -> Result<(Vec<PulledCommit>, u32)> {
    let mut walk = repo.revwalk().wrap()?;
    walk.push(tip).wrap()?;
    walk.hide(hidden).wrap()?;

    let mut commits = Vec::new();
    for oid in walk.take(cap) {
        let oid = oid.wrap()?;
        let commit = repo.find_commit(oid).wrap()?;
        commits.push(summarise(&commit));
    }
    let more = total.saturating_sub(commits.len() as u32);
    Ok((commits, more))
}

/// One commit as the three lines a chooser shows.
///
/// `pub(crate)` because [`crate::push::preview`] lists a set this walk cannot produce — the
/// commits no branch of a remote has, which is a revwalk with many `hide`s and no second
/// endpoint to measure a total against — and the two lists appear side by side in the same
/// dialogs. Two copies of "how do you render a commit into three fields" is how one of them
/// quietly starts showing an email where the other shows a name.
///
/// Both accessors refuse non-UTF-8 bytes rather than mangling them, and an empty string is the
/// honest rendering of that: the alternative is mojibake in a toast, and the short oid beside it
/// is still enough to run `git show`.
pub(crate) fn summarise(commit: &git2::Commit<'_>) -> PulledCommit {
    let author = commit.author();
    PulledCommit {
        short_oid: short_oid(commit.id()),
        summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
        author: author.name().unwrap_or("").to_string(),
    }
}

/// Eight hex digits, the width every other short oid in this crate uses.
pub(crate) fn short_oid(oid: Oid) -> String {
    oid.to_string().chars().take(8).collect()
}

// --- the three integrations ---------------------------------------------------------------

/// Move the branch onto its upstream. The old behaviour, moved here whole.
fn fast_forward(
    repo: &Repository,
    root: &Path,
    branch: &str,
    local_oid: Oid,
    remote_oid: Oid,
    behind: u32,
) -> Result<Integrated> {
    let target = repo.find_commit(remote_oid).wrap()?;
    let in_the_way = branch::blockers(repo, &target)?;
    if !in_the_way.is_empty() {
        // The same refusal as a checkout, because it is the same operation: moving the
        // working tree onto a different commit.
        return Err(GitError::CheckoutWouldOverwrite {
            branch: branch.to_string(),
            paths: in_the_way,
        });
    }

    /*
     * The report is built **before** the working tree moves, and that ordering is the design.
     *
     * Everything it reads is a local object the fetch already wrote, so it can only fail if the
     * object database is broken — and in that case failing here leaves the tree exactly where
     * it was, whereas failing after `checkout_tree` would report an error for a pull that had
     * actually happened. That is the worse of the two: a user who is told the pull failed will
     * run it again.
     */
    let report = integrated(
        repo,
        branch,
        local_oid,
        remote_oid,
        behind,
        0,
        0,
        Vec::new(),
    )?;

    write_orig_head(repo, local_oid);
    let mut builder = git2::build::CheckoutBuilder::new();
    builder.safe();
    repo.checkout_tree(target.as_object(), Some(&mut builder))
        .wrap()?;
    repo.reference(
        &format!("refs/heads/{branch}"),
        remote_oid,
        true,
        "cide: fast-forward pull",
    )
    .wrap()?;
    settle(root, repo);
    Ok(report)
}

/// `git pull --no-rebase`: a real merge commit, or a conflicted tree to resolve.
#[allow(clippy::too_many_arguments)]
fn merge_pull(
    repo: &Repository,
    root: &Path,
    branch: &str,
    local_oid: Oid,
    remote_oid: Oid,
    remote: &str,
    upstream: &str,
    behind: u32,
) -> Result<Integrated> {
    let target = repo.find_commit(remote_oid).wrap()?;
    // The dirty-tree refusal, before anything is written. Not a conflict — a conflict is
    // between two commits and is resolvable; this is uncommitted work the merge would
    // clobber, and there is nothing to resolve it against.
    let in_the_way = branch::blockers(repo, &target)?;
    if !in_the_way.is_empty() {
        return Err(GitError::CheckoutWouldOverwrite {
            branch: branch.to_string(),
            paths: in_the_way,
        });
    }

    write_orig_head(repo, local_oid);

    let annotated = repo.find_annotated_commit(remote_oid).wrap()?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    // `SAFE`, never forced. `blockers` above has already established that nothing this writes
    // is locally modified, so `SAFE` means that if that reasoning is ever wrong libgit2 refuses
    // instead of overwriting — the same argument `replay::write` makes.
    checkout.safe();
    // Everything else is `MergeOptions`' defaults, and that is correct rather than lazy:
    // `GIT_MERGE_OPTIONS_INIT` is `{VERSION, GIT_MERGE_FIND_RENAMES}`, so rename detection is
    // already on, which is what git's own `ort` strategy does.
    repo.merge(&[&annotated], None, Some(&mut checkout))
        .wrap()?;

    /*
     * Overwrite libgit2's `MERGE_MSG` with git's own sentence, before anything can read it.
     *
     * `git_merge` writes `Merge commit '<full oid>'` plus a `#Conflicts:` block — true, and
     * not what `git pull` writes. It matters because the *conflicted* path concludes through
     * `cide_git::conflict::cont`, which commits whatever `MERGE_MSG` holds, so leaving
     * libgit2's version would give a conflicted merge a different message from a clean one for
     * no reason a user could see. Written once, here, and both paths use it.
     */
    let message = merge_message(repo, remote, upstream, branch);
    let _ = std::fs::write(repo.path().join("MERGE_MSG"), &message);

    let index = repo.index().wrap()?;
    let conflicts = repo_mod::conflicts_of(&index)?;
    if !conflicts.is_empty() {
        /*
         * Left exactly as it is, and reported as success.
         *
         * `MERGE_HEAD` is on disk, the index carries stages 1/2/3 and the working tree carries
         * git's markers — which is what makes this resumable after a restart, visible to
         * `git status` in a pane, and abandonable with `git merge --abort` by somebody who
         * would rather not use the resolver.
         *
         * `Ok` and not `Err` because the gesture did what it was asked to do. An error routes
         * to the red toast `chrome/Failures.tsx` draws for refusals, and this is not a refusal:
         * it is work waiting for the user.
         */
        settle(root, repo);
        return integrated(repo, branch, local_oid, local_oid, behind, 0, 0, conflicts);
    }

    // Clean. Conclude it here rather than leaving `MERGE_HEAD` for the panel: a merge with
    // nothing to decide is not a state to make somebody click through.
    let signature = repo.signature().wrap()?;
    let head = repo.find_commit(local_oid).wrap()?;
    let tree_oid = repo.index().wrap()?.write_tree().wrap()?;
    let tree = repo.find_tree(tree_oid).wrap()?;
    let created = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &tree,
            // **`[ours, theirs]`, in that order.** The reverse is a perfectly valid commit
            // that is wrong in a way only `git log --first-parent` reveals, months later.
            &[&head, &target],
        )
        .wrap()?;
    repo.cleanup_state().wrap()?;
    settle(root, repo);
    integrated(repo, branch, local_oid, created, behind, 0, 0, Vec::new())
}

/// `git pull --rebase`: replay the local commits onto the upstream.
#[allow(clippy::too_many_arguments)]
fn rebase_pull(
    repo: &Repository,
    root: &Path,
    branch: &str,
    local_oid: Oid,
    remote_oid: Oid,
    ahead: u32,
    behind: u32,
) -> Result<Integrated> {
    // The two refusals that must happen before `git2::Repository::rebase` writes anything,
    // because once `.git/rebase-merge` exists the only ways out are Continue and Abort.
    let replayed = local_only(repo, local_oid, remote_oid)?;
    if replayed.len() > REBASE_COMMIT_CAP {
        return Err(GitError::RebaseTooLong {
            branch: branch.to_string(),
            ahead,
            limit: REBASE_COMMIT_CAP as u32,
        });
    }
    let merges = merges_among(repo, &replayed)?;
    if !merges.is_empty() {
        return Err(GitError::RebaseWouldDropMerges {
            branch: branch.to_string(),
            commits: merges,
        });
    }

    /*
     * The dirty-tree refusal, before `Repository::rebase` writes a byte.
     *
     * The other two paths make it through `branch::blockers`, and the rebase needs it for a
     * sharper reason than symmetry: without it libgit2 raises its own
     * `Rebase/GenericError: unstaged changes exist in workdir`, which reaches the user as a
     * `GitError::Git` — a libgit2 class name attached to a button they pressed, naming no file
     * they could act on. `blockers` names them, and it asks the *narrower* question a checkout
     * asks, so an edit to a file the replay does not touch still comes along.
     *
     * Asked against the upstream's tree, which is what the working tree moves to first.
     */
    let onto_commit = repo.find_commit(remote_oid).wrap()?;
    let in_the_way = branch::blockers(repo, &onto_commit)?;
    if !in_the_way.is_empty() {
        return Err(GitError::CheckoutWouldOverwrite {
            branch: branch.to_string(),
            paths: in_the_way,
        });
    }

    let head = repo.find_branch(branch, BranchType::Local).wrap()?;
    let onto = repo.find_annotated_commit(remote_oid).wrap()?;
    let branch_ref = repo.reference_to_annotated_commit(head.get()).wrap()?;
    drop(head);

    let signature = repo.signature().wrap()?;
    let mut options = git2::RebaseOptions::new();
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.safe();
    options.checkout_options(checkout);
    // `git_rebase_init` writes `ORIG_HEAD` itself (`rebase.c:485`), so unlike the other two
    // paths this one does not.
    let mut rebase = repo
        .rebase(Some(&branch_ref), Some(&onto), None, Some(&mut options))
        .wrap()?;

    let mut rewritten = 0u32;
    let mut skipped = 0u32;
    while let Some(operation) = rebase.next() {
        operation.wrap()?;
        let index = repo.index().wrap()?;
        let conflicts = repo_mod::conflicts_of(&index)?;
        if !conflicts.is_empty() {
            // Stop here with the sequencer's state on disk. `crate::conflict::cont` picks it
            // up again through `Repository::open_rebase`, which is why nothing tries to hold
            // this `Rebase` across the IPC boundary — it borrows `repo`.
            drop(rebase);
            settle(root, repo);
            let tip = repo.head().wrap()?.target().unwrap_or(local_oid);
            return Ok(Integrated {
                rewritten,
                skipped,
                ..integrated(repo, branch, local_oid, tip, behind, 0, 0, conflicts)?
            });
        }
        match rebase.commit(None, &signature, None) {
            Ok(_) => rewritten += 1,
            /*
             * `GIT_EAPPLIED`: replaying this commit would produce nothing.
             *
             * git's `--empty=drop`, which in practice means *this patch is already upstream*.
             * Counted rather than swallowed — `git rebase` drops them silently, and a user
             * whose three commits became two has to be told which arithmetic happened or they
             * go looking for the lost one.
             *
             * **Two differences from `git rebase`, both reported rather than hidden.**
             *
             * git's `--cherry-pick` pre-pass compares patch-ids before replaying, so it also
             * drops a commit whose patch is upstream but whose replay would conflict. libgit2
             * has no such pass, so that case reaches us as a conflict and stops there. A stop
             * is not damage — it names the paths and offers the resolver — but it is a
             * difference worth knowing about.
             *
             * And libgit2 answers `EAPPLIED` for a commit that was **empty to begin with**
             * (`git commit --allow-empty`), which `git rebase` keeps: `--keep-empty` is its
             * default and only commits that *become* empty are dropped. libgit2's rebase
             * offers no way to force one through mid-sequence, so cide follows libgit2 and the
             * commit is counted here. That is why `skipped` is on the wire at all — a marker
             * commit that vanished has to be visible in the notice rather than discovered by
             * someone reading their own log a week later.
             */
            Err(e) if e.code() == ErrorCode::Applied => skipped += 1,
            Err(e) => {
                let _ = rebase.abort();
                return Err::<Integrated, git2::Error>(e).wrap();
            }
        }
    }
    rebase.finish(Some(&signature)).wrap()?;
    drop(rebase);

    let tip = repo
        .find_branch(branch, BranchType::Local)
        .wrap()?
        .get()
        .target()
        .unwrap_or(remote_oid);
    settle(root, repo);
    Ok(Integrated {
        rewritten,
        skipped,
        ..integrated(repo, branch, local_oid, tip, behind, 0, 0, Vec::new())?
    })
}

// --- shared machinery ---------------------------------------------------------------------

/// Build the report for a branch that moved from `old` to `new`.
#[allow(clippy::too_many_arguments)]
fn integrated(
    repo: &Repository,
    branch: &str,
    old: Oid,
    new: Oid,
    behind: u32,
    rewritten: u32,
    skipped: u32,
    conflicts: Vec<String>,
) -> Result<Integrated> {
    let old_tree = repo.find_commit(old).wrap()?.tree().wrap()?;
    // A conflicted pull has not moved `HEAD`, so `new == old` and the totals come out zero.
    // That is the honest answer: nothing is different about the tree *yet*.
    let new_tree = repo.find_commit(new).wrap()?.tree().wrap()?;
    let (files_changed, insertions, deletions) = diff_totals(repo, &old_tree, &new_tree)?;
    // Always what came down from upstream, whichever strategy ran, hidden behind the
    // **pre-pull** local tip. That is what makes one walk right for all three.
    let upstream_tip = repo
        .find_branch(branch, BranchType::Local)
        .ok()
        .and_then(|b| b.upstream().ok())
        .and_then(|u| u.get().target())
        .unwrap_or(new);
    let (commits, more_commits) = taken_commits(repo, upstream_tip, old, behind, PULL_COMMIT_CAP)?;
    Ok(Integrated {
        old_oid: old,
        new_oid: new,
        branch: branch.to_string(),
        advanced: behind,
        rewritten,
        skipped,
        files_changed,
        insertions,
        deletions,
        commits,
        more_commits,
        conflicts,
    })
}

/// The commits on the branch that the upstream does not have, oldest first.
///
/// `TOPOLOGICAL | REVERSE` is git's replay order: parents before children, oldest applied
/// first.
fn local_only(repo: &Repository, local: Oid, upstream: Oid) -> Result<Vec<Oid>> {
    let mut walk = repo.revwalk().wrap()?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE)
        .wrap()?;
    walk.push(local).wrap()?;
    walk.hide(upstream).wrap()?;
    walk.collect::<std::result::Result<Vec<_>, _>>().wrap()
}

/// Any merge commits among `oids`, as the refusal's list.
fn merges_among(repo: &Repository, oids: &[Oid]) -> Result<Vec<PulledCommit>> {
    let mut out = Vec::new();
    for oid in oids {
        let commit = repo.find_commit(*oid).wrap()?;
        if commit.parent_count() <= 1 {
            continue;
        }
        if out.len() < PULL_COMMIT_CAP {
            out.push(PulledCommit {
                short_oid: short_oid(*oid),
                summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
                author: commit.author().name().unwrap_or("").to_string(),
            });
        }
    }
    Ok(out)
}

/// Point `ORIG_HEAD` at where the branch was.
///
/// libgit2 writes it from its own `merge` and `rebase` (`merge.c:2856`, `rebase.c:485`), so
/// only the fast-forward path needs this — but it is a one-level pseudo-ref that
/// `git_reference_normalize_name` accepts and writes no reflog for, which is what git does too.
///
/// Written **before** the checkout. If the checkout then fails, `ORIG_HEAD` names where you
/// still are, which is harmless; the other order leaves the one undo affordance a rewritten
/// branch has pointing at nothing. `git reset --hard ORIG_HEAD` is what a person reaches for
/// after a pull they did not want, and cide offers no other way back.
///
/// Best-effort: a repository where this fails is one where the pull is about to fail anyway,
/// and refusing to pull because a convenience ref could not be written would be the wrong
/// trade.
pub(crate) fn write_orig_head(repo: &Repository, was: Oid) {
    let _ = repo.reference("ORIG_HEAD", was, true, "cide: pull");
}

/// Put the changelist sidecar back in step with the index this pull just rewrote.
///
/// # Why this exists at all
///
/// `checkout_tree` makes libgit2 rewrite `.git/index` for every path it writes, and a merge
/// rewrites it wholesale. Under ADR 0004 the sidecar holds a fingerprint of the index cide last
/// wrote, and a stale one makes the **next** commit refuse with `IndexChangedExternally` —
/// pointing the "staging changed outside cide" bar at cide's own act.
///
/// `commit`, `replay`, `reset` and `stage` all do this and `branch` never did, which is why a
/// pull followed by a commit could ask the user to reconcile an index nobody but cide had
/// touched. Same `let _` as `replay::replay`: a sidecar that cannot be written is a warning,
/// not a reason to report a pull that happened as failed.
pub(crate) fn settle(root: &Path, repo: &Repository) {
    let _ = changelist::record_index(root, repo);
    // The pulled paths either arrived as commits, and so have no pending change, or landed as
    // conflicts, which live outside changelists entirely. Either way no changelist needs
    // *editing* — only explicit assignments are stored — but the assignments that are now dead
    // have to be swept. Same trailer, same reason, as `commit::commit` and `replay::replay`.
    let live = status::live_paths(root).unwrap_or_default();
    let _ = changelist::update(root, |data| {
        let _ = data.reconcile(&live);
        Ok(())
    });
}

// --- the merge message --------------------------------------------------------------------

/// The title `git pull` writes for a merge.
///
/// # Two things about this that are not guessable
///
/// **A pull's merge message does not come from `git merge`.** `git merge origin/main` writes
/// *"Merge remote-tracking branch 'origin/main'"*; a pull writes something else, because the
/// message comes from `fmt-merge-msg` reading `FETCH_HEAD`, which records the *remote-side*
/// branch name and the URL it came from. Reproducing `git merge`'s message here would be
/// wrong on every pull.
///
/// **The ` into <branch>` clause is omitted for `master` and for `main`, and for nothing else.**
/// Those two are a hardcoded pair in `fmt-merge-msg.c` — *not* a lookup of
/// `init.defaultBranch`, which was the obvious guess and is wrong: a repository whose
/// `init.defaultBranch` is `trunk` still gets ` into trunk` on `trunk` and still omits it on
/// `main`. Established by running the real binary over all four cases (git 2.54) rather than
/// recalled, and pinned by `the_merge_message_names_the_branch_except_on_master_and_main` in
/// `tests/pull.rs`, which is what will notice if git ever changes the pair.
///
/// Reproduced character for character the way [`crate::replay`]'s `message_for` reproduces
/// `sequencer.c`'s revert body, and for the same reason: a message that is *nearly* git's is
/// the kind of difference that only shows up in somebody's release notes.
///
/// No body. `merge.log` defaults to false, so the shortlog block a `--log` pull adds is not
/// written.
fn merge_message(repo: &Repository, remote: &str, upstream: &str, branch: &str) -> String {
    // The branch as the *remote* knows it: `branch.<name>.merge` with `refs/heads/` stripped,
    // falling back to the upstream shorthand minus its remote prefix. The two differ whenever
    // somebody tracks a differently-named branch, which is exactly when a wrong answer would
    // be least noticeable.
    let remote_branch = repo
        .config()
        .ok()
        .and_then(|c| c.get_string(&format!("branch.{branch}.merge")).ok())
        .and_then(|v| v.strip_prefix("refs/heads/").map(str::to_string))
        .or_else(|| {
            upstream
                .strip_prefix(&format!("{remote}/"))
                .map(str::to_string)
        })
        .unwrap_or_else(|| upstream.to_string());

    let url = repo
        .find_remote(remote)
        .ok()
        .map(|r| String::from_utf8_lossy(r.url_bytes()).into_owned())
        .unwrap_or_default();

    let mut out = format!("Merge branch '{remote_branch}'");
    // git drops the ` of …` clause for the `.` remote, which is a repository merging from
    // itself and has no URL worth printing.
    if !url.is_empty() && url != "." {
        out.push_str(&format!(" of {}", anonymize_url(&url)));
    }
    if branch != "master" && branch != "main" {
        out.push_str(&format!(" into {branch}"));
    }
    out.push('\n');
    out
}

/// git's `transport_anonymize_url`.
///
/// Strips `user:password@` from a URL that has a scheme, leaves a local path alone, and — git's
/// own oddity, reproduced because the message has to match — turns `git@github.com:o/r` into
/// `github.com:o/r`. That last clause is not a security measure; it is what git does, and a
/// merge message that differs from git's by a `git@` is still a difference.
fn anonymize_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let host = rest.split_once('@').map_or(rest, |(_, after)| after);
        return format!("{scheme}://{host}");
    }
    // scp-like `[user@]host:path`. Only when the part before the first `:` contains an `@`,
    // so an absolute Windows-style path or a plain `host:path` is untouched.
    if let Some((head, tail)) = url.split_once(':')
        && let Some((_, host)) = head.split_once('@')
    {
        return format!("{host}:{tail}");
    }
    url.to_string()
}
