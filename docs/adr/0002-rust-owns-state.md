# ADR 0002 — The Rust core owns all durable state; sessions are owned by the process

**Status:** accepted (M1)
**Date:** 2026-08-07

## Context

The webview is the obvious place to keep the workspace: the tree is what it renders, React
state is cheap, and every mutation would be a local edit with no round trip.

Three facts make that wrong here, and none of them is a preference.

1. **A detached pane is a separate JavaScript realm.** ADR 0001 rules out multiwebview, so
   tearing a pane into its own window means a second webview with no shared memory. If the
   pane tree lived in the first window's store, the second would have no way to read it and
   the two would immediately disagree about which panes exist.
2. **Sessions must outlive every view.** A pane, a tab and a window are all things a user
   closes casually; a `claude` process mid-turn is not. If a session were owned by the pane
   showing it, closing the pane would kill the child — and "detach into a window" would be
   indistinguishable from "restart the conversation".
3. **PTY children are already Rust resources.** The master fd, the reader thread, the vt100
   parser and the writer channel live in `cide-pty`. A pane tree in JavaScript would be a
   second, unsynchronised description of the same processes, and the two would drift on every
   crash, reload or window close.

## Decision

`cide-core` owns `Workspace`: settings, projects, roots, tabs, pane trees, ratios, focus and
window roles. Every mutation goes through a `cide-core` operation, is validated, bumps `rev`
and is broadcast to every window as `cide://workspace-changed`. `ui/src/store/workspace.ts` is
a *mirror* — it never edits the tree, and it drops any snapshot whose `rev` is older than what
it already holds, because two windows mutating one tree means snapshots can arrive out of
order.

Sessions live **outside** the tree, in a process-global `SessionRegistry` keyed by
`SessionId`. A `Pane` holds a `SessionId`, never a `Session`. Attaching, detaching, closing a
pane, closing a window: none of them touch the child. `Session.sinks` is a *list*, not an
`Option`, so a new window's sink can be attached before the old one is dropped and not one
byte is lost across a detach.

The webview owns exactly two things: transient gesture state (which overlay is open, whether
a splitter is mid-drag) and the live DOM instances — the xterm `Terminal` and the CodeMirror
`EditorView` — which cannot be serialised and must not be recreated.

## Consequences

- **The features this pays for are nearly free.** "Detach pane into its own window" is:
  remove the leaf, collapse the parent split into its sibling, open a `pane:<uuid>` window,
  attach to the *same* `SessionId`. The PTY never notices and `claude` never restarts. The
  Settings radio *Stack projects in the header* vs *One window per project* is two different
  mappings from `ProjectId` to window label over identical state — about forty lines.
- **Pinning is enforced where it cannot be bypassed.** `tab.close` on index 0 returns
  `Err(TabPinned)` and `tab.reorder` refuses to move it. No frontend bug can lose the project
  console, because the frontend never had the authority.
- Every mutation costs an IPC round trip. Acceptable for structural changes, which are
  human-paced; unacceptable for splitter drags, which is why a drag writes
  `gridTemplateColumns` to the DOM and commits `pane.set_ratio` once, on `pointerup`.
- The wire contract has to be checked, because two languages now describe one tree. All DTOs
  live in `cide-ipc` with `ts-rs`; `cargo xtask codegen --check` and `cargo xtask
  contract-check` are gates, so a Rust field rename fails the build instead of surfacing as
  an `undefined` in the webview.
- `workspace.json` records what a pane *was*, never a liveness claim: kind, cwd, Claude uuid,
  title. On launch each `SessionSpec` is reconciled to Resumable (`claude --resume <uuid>` —
  the `SessionId` *is* the uuid, so this needs no extra bookkeeping) or Fresh.
- `cide-headless` exists as the standing proof of this boundary: it renders a live session
  from the vt100 mirror to stdout with zero Tauri linked. If it stops compiling, domain logic
  has leaked into the app crate.
