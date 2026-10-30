# ADR 0016 — Local History keeps its own store, outside the project

**Status:** accepted (M194)
**Date:** 2026-10-30

## Context

cide can recover what git has seen. It could not recover what git has not: an agent's bad edit
between commits, a `cargo fmt` over uncommitted work, a file deleted before it was ever added. In
an IDE whose main author is a process in a terminal pane, those are the ordinary ways to lose work.
Local History records every project file as it changes, independently of git, so that any version
or deleted file can be restored, in a project with no repository at all.

Five decisions are the ones a later refactor would undo, each because the tidier-looking option
is the one that fails on the day the store is needed.

## Decision 1 — the store lives in the state directory, not in the project

`<state>/local-history/<key>/`, keyed by `scratch::key(primary_root)`.

`.cide/history/` was the alternative and it moves with the project. It loses because of when this
store is needed: `git clean -fdx`, a deleted folder, an agent running something it should not
have. A copy inside the thing that was destroyed is not a copy. It would also need an ignore entry
in a directory that is otherwise committed, sit inside the watched tree, and be grepped by agents.

The cost is accepted: moving a project starts a new history, because the key follows the path. The
old store stays, with an origin record naming its project.

## Decision 2 — a store is append-only packs and a JSONL log, not SQLite and not loose blobs

**Not one file per blob**, which was the first design. A durable new file is a create, a write, a
fsync and a directory fsync, at least two journal commits on ext4, so a 100,000-file baseline would
be 200,000 commits on the filesystem the user's saves wait on, and 100,000 inodes that a cap in
stored bytes cannot see. A pack turns that into one append and one fsync per batch. The price is
that dropping a blob frees nothing until the pack is rewritten, which is what compaction does.

**Not SQLite**, which would answer everything here and is a C build, a line in the root
`Cargo.toml` and a second storage idiom in a workspace with none. The store needs to append a
batch, replay at open and rewrite during retention, and that is a file. Measured on ext4 over NVMe
with this repository's source cycled to 100,000 files: 28 s to baseline 1.35 GiB into 444 MiB of
packs and 20 MiB of log, 0.08 s to reopen, about 10 ms per single-file commit.

The order is the crash story, in both directions: **blobs, their fsync, rows, their fsync**. A row
is the only thing that makes a blob reachable, so a crash leaves bytes nothing names and never a row
naming bytes that did not arrive. Compaction mirrors it: copy live blobs and fsync, replace the
log, then unlink.

## Decision 3 — one worker per store, and three capture sources

Everything that writes runs on one thread: bursts, saves, passes and retention, as chunks with the
queue drained between them. So there is no reconcile racing a capture, and no compaction racing an
append: they cannot overlap. Only the walk runs elsewhere, because it holds a shared permit.

No single source is enough. The watcher sees agents and terminals, but reports only paths, after
the old bytes are gone. A save knows its bytes and can look at the file first, which is the last
moment a stale Ctrl+S's victim exists. A reconcile pass is the only source that can produce a
baseline or see what happened while cide was closed. Content hashes reconcile the three, compared
with the path's head and only its head, which handles cide's own double report with no suppression
timer, and keeps A → B → A as three real versions.

`Store::commit` runs under a write lock, so saves ask with `try_read` and on a miss read the old
bytes, which is always safe. A save never waits on a compaction.

## Decision 4 — a walk sighting is never a creation

A file's first row says when cide first saw it, not when it appeared. So there are two kinds:
`Baseline`, which claims nothing, and `Created`, which claims the file did not exist before this
row, and which only a watcher event after a durable `BaselineDone` marker may write.

A folder restore deletes on `Created` and only on `Created`. Everything it cannot prove is left
alone and reported. This is the difference between a restore and trashing a file that had been
there for years, and it is why a file over the size limit, or past the baseline's share of the cap,
still gets a row with no bytes.

**The proof also needs the store to remember the moment.** A `Created` only proves absence back to
the file's previous life, and retention forgets those: a file that existed, was deleted and came
back has a `Created` for a first row once the older rows age out, and if all of them have expired
there is no row left to say otherwise. So the log's header carries `trimmed_through`, the newest
row retention has ever dropped, and a moment at or before it gets no proof at all. Demoting the row
at compaction was the first fix and cannot work: the path that lost every row has no row to demote.

## Decision 5 — off is a switch, and it never deletes

JetBrains spells "off" as a retention of 0. Here it is a switch, global with a per-project
override, and retention clamps to at least one day: a number that means "stop" at one value is a
hidden switch, and one that deletes everything at that value is worse.

Off stops recording and keeps every version. A project with a store keeps a paused worker, so
browsing and restoring have one road whether the switch is on or off, and no second opener can be
holding the lock when a real worker starts. **A restore while off still saves the current file
first**: an explicit overwrite is not the place to honour "stop recording". Deleting is its own
action, behind a confirmation.

## Consequences

* Moving a project starts a new history.
* Gitignored files are not kept even when saved in the editor: it keeps one eligibility rule, and
  does not copy `.env` into a second place. A file git cannot recover is therefore a file this
  cannot either, and that is a real gap worth revisiting on request.
* Edits within one watcher window collapse into one version, and edits while cide is closed into
  one `Detected` version. Intermediate states of a file nobody saved in cide are not recoverable.
* Agent worktrees are invisible until integrated, because `.cide/` is refused as a whole.
* External renames read as a deletion and a creation. Only cide's own carry the history across.
