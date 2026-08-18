# ADR 0003 — xterm.js owns terminal rendering; Rust keeps a parallel vt100 mirror

**Status:** accepted (M0, confirmed by the M0 GO/NO-GO measurement)
**Date:** 2026-08-07

## Context

Something has to turn PTY bytes into a picture. Two shapes were available.

**A — the webview parses.** Raw bytes go to the frontend and `@xterm/xterm` does everything:
VT parsing, scrollback, selection, the alt screen, mouse protocols, OSC 8 links, OSC 52
clipboard, DEC 2026 synchronized output.

**B — Rust parses.** `vt100` maintains the screen in Rust and the frontend receives dirty-row
diffs to paint. Far less data on the wire, and the frontend becomes a dumb renderer.

The plan committed to A with an explicit escape hatch: measure the IPC transport first, and
if raw pull throughput came in under about 30 MB/s, switch to B **before any other code was
written**. That gate is `cargo xtask bench-ipc` and its numbers are in `BENCH.md`.

The measurement passed, and the deciding argument is not throughput anyway: a terminal is far
more than a grid of cells. Reimplementing selection, link detection, clipboard escapes,
bracketed paste, mouse reporting and the alt screen on top of dirty-row diffs is a
multi-year project, and `claude`'s TUI exercises all of it. xterm 6 specifically is required
— it is the first release with DEC mode 2026 synchronized output, which Claude Code probes at
startup; on 5.5 the TUI flickers.

## Decision

The frontend owns rendering. `crates/cide-pty` writes raw bytes to a `Channel<...>` and
`ui/src/terminal/xterm.ts` feeds them to `term.write(Uint8Array)`.

Rust *also* keeps a `vt100::Parser` per session, fed **always**, attached or not, with 5000
lines of scrollback. It is never used to paint the live terminal. It exists for three things
that A alone cannot do:

- **Attach.** `session.attach` sends `PtySession::reattach_state()` as the first frame —
  contents, bracketed paste, mouse protocol, cursor, **and which of the two buffers that
  picture is of** — so a window that has just opened shows the session as it is rather than
  as it will be from the next byte on.
- **Catch-up.** When a sink falls more than 500 ms behind, the raw stream is dropped and one
  `reattach_state()` frame is pushed instead.
- **Host eviction.** Live terminal hosts are capped; an evicted idle pane rehydrates from the
  mirror.

Three rules make the transport work, and each is a correctness requirement rather than a
tuning choice:

- **Coalesce to ≥8 KiB or 8 ms, whichever comes first, capped at 64 KiB.** Below Tauri's
  1024-byte `MAX_RAW_DIRECT_EXECUTE_THRESHOLD` a raw payload is serialised into a JSON array
  of decimal numbers and `eval`'d — roughly 5× inflation, on the GTK main loop. An unbatched
  interactive PTY yields 20–200 byte reads and would live permanently on that path.
- **Real OS backpressure.** Blocking reader thread → bounded channel (16) → coalescer. When
  the channel fills the reader blocks, the kernel PTY buffer fills, and the child blocks in
  `write()`. `cat bigfile` throttles itself.
- **Credit ACK, acked from `term.write`'s completion callback.** The channel handler proves
  bytes *arrived*; the completion callback is the point at which xterm has parsed them.
  Acking on arrival reports a rate the renderer cannot sustain and defeats the mechanism.

After attaching, if the alt screen is engaged, one `cols-1 → cols` nudge is sent so the
fullscreen TUI repaints from its own model — covering everything `vt100` does not track
(OSC 8, OSC 52, DEC 2026 framing).

## Consequences

- Terminal features come from a maintained terminal emulator rather than from us.
  `@xterm/addon-unicode-graphemes` is **mandatory** (`allowProposedApi: true`,
  `activeVersion: '11'`); without it every wide glyph is one cell narrow and the fullscreen
  frame shears.
- Two representations of one screen exist and can in principle disagree. They are reconciled
  in one direction only — the mirror seeds a new attachment, never corrects a live one — so a
  divergence costs one stale first frame, not a permanently wrong terminal. **That held only
  once the seed said which buffer it was a picture of, and for a long time this document
  claimed it did when it did not.** `vt100::Screen::state_formatted` dumps whichever grid is
  active and emits no `?1049` at all, so a sink that missed the child's own `?1049h`/`?1049l`
  — which a *choked* sink is designed to do, since its raw frames are skipped and this
  snapshot is substituted — sat on the other buffer from the child for the rest of the
  session, with nothing able to bring the two back. The reported symptoms were a pane with no
  scrollbar whose wheel xterm turned into cursor keys aimed at the agent's prompt, and an
  agent's question that was drawn into rows the viewport no longer had, so it appeared only
  when a resize forced a full repaint. `cide_pty::reattach_bytes` prepends the buffer, which
  is what makes the seed self-sufficient; `PtySession::screen_state` is kept, unprefixed, for
  the one consumer that must *not* follow the buffer — replaying a dead child's last screen
  into a fresh shell (`SpawnSpec::preload`).
- Credit flow control has a deadlock shape: a webview that stops acking (crash, GPU hang,
  occluded-window throttle, WebGL context loss) would pin a session above the high-water mark
  forever, looking frozen with no error anywhere. Mitigated by a 5 s watchdog force-reset, by
  a detached sink clearing its credit rather than stranding it, and structurally by
  `sinks` being a list.
- PTY bytes never enter React. Terminals live in `ui/src/layout/paneHosts.ts` at module
  scope, outside the component tree entirely.
