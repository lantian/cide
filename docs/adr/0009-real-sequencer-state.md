# ADR 0009 — A conflict cide can resolve is a conflict git can see

**Status:** accepted (M20)
**Date:** 2026-08-21

## Context

`crates/cide-git/src/replay.rs` rejects libgit2's stateful `git_revert` and `git_cherrypick`, and
by extension `git_merge` and `git_rebase`. Its header gives two reasons:

> * Under ADR 0004 `.git/index` is a *derived* artifact that cide rebuilds from the active
>   changelist at commit time. A sequencer that plants state there is planting it in the one
>   place the next commit is going to overwrite.
> * The moment `RepositoryState` is non-clean, `crate::repo::operation_in_progress` starts
>   refusing **every other action in this crate** — commit, checkout, reset, replay. A user who
>   cherry-picked one commit would find the whole git surface locked, from a panel with nothing
>   on it that can finish or abort the operation.

Both were right, and the consequence rippled outward. `branch::pull` was fast-forward-only for
exactly the second reason and said so. `worktree::integrate` composed the agent merge in memory
and returned `Integration::Conflicts`. The sentence *"cide has no conflict-resolution surface"*
appeared in six places in the workspace and was load-bearing in each.

The cost was that every one of those operations answered a conflict by naming the files and
sending the user to a terminal to do the whole thing again — including `git.pull`, on Ctrl+T,
which is the IDE's headline key.

## Decision

**Operations that can conflict write real git state, and cide builds the surface that finishes
it.**

`pull` uses `Repository::merge` and `Repository::rebase`; `replay` falls back to
`Repository::cherrypick` / `Repository::revert` on the conflicting path. Each leaves what git
leaves: `MERGE_HEAD`, `.git/rebase-merge`, `CHERRY_PICK_HEAD` or `REVERT_HEAD`; an index carrying
stages 1, 2 and 3; and git's own conflict markers in the working tree.

`crates/cide-git/src/conflict.rs` reads and resolves it, and the commit panel's *Merge Conflicts*
group and `MergeBar` are where a person drives it.

### The first objection is answered, not ignored

`cide_git::commit` takes the **staging-area arm** whenever it is concluding an operation,
whatever the sidecar says, and waives `require_index_unchanged`.

On a merge the index *is* the truth: it was built by the merge and edited by the resolutions, and
it holds the only record of which side won each conflicted file. `rebuild_index` would
`read_tree(HEAD)` over it and apply the active changelist's hunks on top — silently discarding
*theirs* and committing HEAD-plus-selected-hunks under a message saying the branches were merged.
ADR 0004's rule has one exception and this is it.

The guard is waived for the same reason rather than for convenience: it exists to catch an index
cide did not write, and a merge index is by construction one cide did not write hunk by hunk.

### The second objection is removed rather than answered

`operation_in_progress` had exactly one consumer in the frontend — a tooltip fragment — and its
real effect was to refuse everything. It is now the source of `MergeState`, which the bar is drawn
from and the resolver reads. The refusals that remain are the ones that are still unsafe: starting
a *second* merge, a checkout, a reset, a replay. `commit` admits `merge`, `cherry-pick` and
`revert`, because concluding those is an ordinary commit plus `cleanup_state`; it still refuses
`rebase` and `am`, whose `--continue` belongs to the sequencer and is driven by
`conflict::cont`.

## Consequences

* **A conflict is resumable across a restart**, legible to `git status` in a terminal pane, and
  abandonable with `git merge --abort` by somebody who would rather not use the resolver. None of
  that is true of a cide-private conflict model, and a rebase's *conflict on commit 3 of 7* would
  have forced us to reimplement the sequencer anyway.
* **`Rebase<'repo>` borrows the `Repository`,** so it cannot be held across an IPC call. Every
  continue and abort reopens the repository and calls `Repository::open_rebase`. That round trip
  is load-bearing: if it ever stops working, the resolver's *Continue* is dead and the user is
  stuck mid-rebase.
* **The clean paths are unchanged.** `replay` still composes a non-conflicting cherry-pick by
  hand and creates no sequencer state, which is almost every cherry-pick anyone makes;
  `nothing_leaves_a_sequencer_state_behind` still asserts it, and now also asserts that a
  conflicting one leaves nothing that outlives an abort.
* **`GitError::ReplayWouldConflict` is unreachable and kept.** Removing a variant from this wire
  would turn an old payload into `[object Object]` in a toast.
* **This ADR exists because reverting the decision by reflex from `replay.rs`'s header is the
  likeliest way it gets undone.** That header now points here; anyone tempted to restore the
  in-memory refusal should be satisfied that the panel which justifies this is gone before doing
  so.
