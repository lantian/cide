# ADR 0004 — Changelists are the truth; the git index is rewritten at commit time

**Status:** accepted (M1, implemented in M10)
**Date:** 2026-08-07

## Context

The design mock's git panel is IDEA's Commit tool window, not a porcelain view of `git
status`: a tri-state checkbox tree grouped by **changelist** (`Changes`, `fixes`, `Ignore`,
`tomaster`, `Unversioned Files`), an `Amend` checkbox, a message box and `Commit` /
`Commit and Push…`. Changelists are the organising idea, and they have no representation in
git at all — IDEA keeps them in `.idea/workspace.xml` and its shelf in `.idea/shelf`.

Two models could back that UI.

**A — the index is the truth.** Checking a file stages it; the tree is a rendering of the
index. Simple, and it interoperates with everything.

**B — changelists are the truth.** The tree is our own model; committing rewrites
`.git/index` from the active changelist and commits that.

A cannot express the UI. The index is a single flat set, so it cannot hold four named groups
at once, and per-line staging within a changelist has nowhere to live. It also makes "commit
this changelist and leave the other alone" impossible without staging and unstaging around
every commit.

B expresses it exactly, and it is what IDEA does. Its cost is a real collision: bash panes
inside cide are precisely where a user will run `git add`, so an index rewritten from our
model can destroy staging they built by hand. That is not theoretical — it is the expected
case.

## Decision

Changelists are the truth. The model lives in a sidecar at
`$XDG_STATE_HOME/cide/repos/<blake3(root)>/changelists.json`, keyed per repository, with the
shelf as our own patch files beside it. Committing rewrites `.git/index` from the active
changelist and commits.

Per-hunk and per-line staging works by synthesising a unified diff of exactly the checked
hunks and feeding it to `git2`'s `Repository::apply(ApplyLocation::Index)`. This is why the
project uses `git2` and not `gix`: `gix` 0.86 has no push, no stash and no apply, and
`Repository::apply` *is* the hunk-staging mechanism.

Three mitigations ship with it, and none is optional:

1. **The external-staging guard.** Before every commit, the index is snapshotted and compared
   against what cide last wrote. If it differs, a non-blocking bar appears — *"staging changed
   outside cide — reload or overwrite?"* — instead of a silent clobber.
2. **IDEA's own escape hatch.** A first-class "use Git staging area instead" mode. In that
   mode cide never resets a hand-built index; the panel becomes a view of the index and the
   changelist layer stands down.
3. **Property tests from day one.** This is the only code in the project where a bug destroys
   uncommitted work, and it is silently wrong on CRLF, a missing trailing newline, mode
   changes, binary files, submodules, intent-to-add entries and renames-with-edits. So the
   tests generate random working-tree states and hunk selections and assert byte-identical
   index state against the **real `git apply --cached`** (git 2.54.0 is on the reference
   machine). No hand-written examples: hand-written examples cover the cases the author
   thought of, which are exactly the cases that already work.

## Consequences

- The UI in the mock is implementable as drawn, including committing one changelist while
  another is untouched.
- Staging state now exists in two places, and cide is responsible for reconciling them. The
  guard bar is the user-visible admission of that; a design that never showed it would be
  claiming a consistency it does not have.
- Shelving is our own patch format rather than `git stash`, because a shelf entry belongs to a
  changelist and a stash does not. Stash remains available and separate, mirroring IDEA.
- Multi-root projects key the sidecar per repository, so a monorepo with several repos and
  nested submodules gets independent changelists rather than one merged set.
