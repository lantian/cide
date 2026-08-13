# cide


An IDE whose centre of gravity is a live Claude Code session rather than a text buffer.

The distinguishing feature is a **pinned, non-closable Claude tab per project**, hosting a
tiling grid of panes — the project's primary Claude session, additional sessions, plain
shells, and read-only diffs — alongside the ordinary IDE furniture that serves it: a file
tree, a tabbed editor, an IDEA-style git commit tool window, `Ctrl+P`, and `Shift+Ctrl+P`.

Rust + Tauri 2. Linux-first (developed on KDE/Wayland), with the code kept portable.

**Status: M0, M1, M3 and M4 complete; M5 code complete, its audit unrun.** The workspace
builds and runs, real `claude` processes render in xterm panes, the domain core owns the
workspace tree, the shell chrome matches the design mock to within a pixel in both themes,
and the pinned Claude tab tiles into a recursive split tree whose panes hold live sessions.
Panes detach into their own windows, quitting saves the workspace, and relaunching resumes
each project's primary conversation. Four automated gates guard the seams: the IPC transport
benchmark (`BENCH.md`), the chrome layout audit, the pane lifecycle audit, and the window
audit.

## Run it

```sh
pnpm --dir ui install
cargo build -p cide-app
./run.sh
```

**Use `./run.sh` rather than launching the binary directly.** Beyond the process hygiene
below, it starts the piece a debug build cannot do without.

A **debug** build does not load `ui/dist`. `tauri.conf.json` sets
`devUrl: http://localhost:1420`, and that URL is baked into the binary — so launching
`./target/debug/cide` with nothing on port 1420 gives a white window and
`Could not connect to localhost: Connection refused`. The app is running correctly; it just
has no document. `run.sh` starts Vite, waits for the port, and stops it again on exit.

If you would rather not have a dev server at all, `./run.sh --release` runs a release build,
which embeds the frontend:

```sh
pnpm --dir ui build
cargo build --release -p cide-app
./run.sh --release
```

`run.sh` also stops any instance already running before starting a new one, reaps `claude`
children orphaned by a previous crash, and refuses to start when the saved workspace would
open more than eight windows.

That last guard is not hypothetical. A workspace here accumulated 242 copies of one
directory, was left in per-project window mode, and the restore path faithfully opened a
window for every one of them — enough to make the machine unusable. Three things now stand in
the way: opening an already-open path activates it instead of duplicating it, the restore
caps how many windows a file may produce, and `run.sh` checks before anything is on screen.

```sh
./run.sh --release          # release build; embeds the UI, needs no dev server
./run.sh --fresh            # start from an empty workspace
./run.sh --bench            # IPC transport gate (M0)
./run.sh --audit-chrome     # chrome vs the design mock (M3)
./run.sh --audit-panes      # pane host registry under churn (M4)
./run.sh --audit-windows    # detach/re-dock and window modes (M5)
./run.sh --on-top           # keep the window above others, for screenshots
```

### Environment variables

| variable | effect |
| --- | --- |
| `CIDE_BENCH=1` | Run the IPC benchmark on first paint, print the report to stdout, exit. This is the M0 GO/NO-GO gate — see `BENCH.md`. |
| `CIDE_ON_TOP=1` | Build the window `always_on_top`. Needed for screenshot-based verification: KDE's focus-stealing prevention keeps a shell-launched window behind everything else. |
| `CIDE_AUDIT=1` | Measure the chrome against the design mock's stated dimensions in both themes at 1440x900, print the table to stdout. This is M3's acceptance check — see below. |
| `CIDE_AUDIT_WINDOWS=1` | Detach and re-dock a pane, flip the window mode, and assert no session was lost to any of it. This is M5's acceptance check. **Not yet run** — see the milestone table. |
| `CIDE_AUDIT_PANES=1` | Run 100 split/close/maximize/tab-switch cycles against the real domain, asserting `term.open()` happened exactly once per pane and no host was destroyed while mounted. This is M4's acceptance check. |
| `CIDE_NO_GRAPHICS_WORKAROUNDS=1` | Skip the Linux graphics ladder, for bisecting a rendering bug against stock behaviour. **The app does not start on a stock KDE Wayland desktop without it** — see ADR 0006. |

## Check it

```sh
cargo build --workspace
cargo test --workspace           # domain invariants + PTY behaviour under load
cargo clippy --workspace --all-targets
cd ui && pnpm typecheck
CIDE_BENCH=1 ./target/debug/cide # re-measure the IPC transport
CIDE_AUDIT=1 ./target/debug/cide # re-check the chrome against the design mock
CIDE_AUDIT_PANES=1 ./target/debug/cide # re-check the pane host registry under churn
CIDE_AUDIT_WINDOWS=1 ./target/debug/cide # re-check detach/re-dock and window modes
```

## Quitting and coming back

cide runs **no background daemon**: quitting quits its Claude sessions. What survives is the
workspace — windows, tabs, splits, detached panes — and the conversations, which resume
because a `SessionId` *is* the value passed to `claude --session-id`.

Shutdown is a ladder rather than a kill: SIGHUP, then SIGTERM, then SIGKILL. A `claude` asked
to stop politely finishes writing its transcript; killed outright it may leave a conversation
unresumable, which is exactly what the restore half depends on. Signals are caught through a
self-pipe, because taking the workspace lock inside a signal handler deadlocks whenever the
interrupted thread already held it.

On relaunch, `app_restore_plan` marks each pane `Resumable` or `Fresh`, and exactly one pane
per project `eager` — its primary session. Every other Claude pane shows a resume splash and
spawns when asked, so reopening a six-pane project does not silently start six agents.

## The pane audit

The host registry is the one piece of this application that cannot be verified by reading
it: its whole job is that a terminal's DOM survives operations that would ordinarily
destroy it, and nothing about that shows up until a pane has been moved a few dozen times.

```
CIDE_AUDIT_PANES=1 ./target/debug/cide
```

It opens this repo plus a second Claude tab, then runs 100 cycles of split, close, focus,
maximize and tab-switch against the **real** Rust-owned tree — not a fake — asserting after
every cycle that `term.open()` has run at most once per pane, that no host was destroyed
while mounted, and that no terminal lost its element. A failure names the cycle and the pane.

Current result: `PASS — 103 panes, no terminal re-opened beyond its eviction allowance, no
host destroyed while mounted`.

## The chrome audit

M3's stated acceptance criterion is a screenshot diff against the design mock at 1440x900 in
both themes. Neither side of that diff is obtainable here — the mock is a template needing a
runtime this repo does not have, and KDE Wayland will not raise a shell-launched window for a
capture tool. What survives is the geometry, so the check became data:

```
CIDE_AUDIT=1 ./target/debug/cide
```

It resizes to a viewport of exactly 1440x900 (correcting for the compositor's invisible
window border, which is 52px on this desktop), renders a fixture covering every chrome state
a live app would not show on its own, and measures 48 dimensions against the mock's stated
values in each theme. An element that is missing reports as a **failure**, not a skip — a
check that silently disappears is indistinguishable from one that passes.

Current result: `PASS — 48 dimensions within ±2px of the mock` in both dark and light, every
delta exactly zero, with both self-hosted fonts confirmed loaded.

The half that is not automated is colour and glyph fidelity, which still wants a human eye.

## Layout

```
crates/
  cide-app/        THE ONLY crate that may depend on tauri. Glue by policy.
  cide-ipc/        Wire DTOs. serde + ts-rs. Zero logic, no tauri.
  cide-core/       The domain: workspace tree, settings, keymap, persistence.
  cide-pty/        PTY sessions: spawn, coalescing, backpressure, vt100 mirror.
  cide-claude/     Spawning and supervising `claude`: env, hooks, resume/fork.   (M7)
  cide-ide-mcp/    The Claude Code IDE-integration MCP server.                   (M6)
  cide-git/        Multi-root git, hunk/line staging, changelists, shelf.        (M10)
  cide-fs/         Gitignore-aware indexing and watching.                        (M8)
  cide-search/     Fuzzy pickers behind a Matcher trait.                         (M8)
  cide-hook/       Second binary: bridges a Claude hook to the running IDE.      (M7)
  cide-headless/   Third binary: proves the core links without tauri.
ui/                React 19 + Vite 8 frontend. One document per window.
docs/adr/          Decisions that would otherwise be refactored away.
```

`cide-headless` is load-bearing architecture, not a demo: it links `cide-core`, `cide-ipc`
and `cide-pty` and must never be able to link `tauri`. If domain logic leaks into the app
crate, it stops building — cheaper than a code-review convention.

## Three decisions that shape everything

**One webview per OS window; panes are DOM.** Tauri's `unstable` multiwebview is the
obvious way to build a pane grid and is functionally broken on Linux: `tauri-runtime-wry`
packs child webviews into a `GtkBox`, and wry only honours `set_bounds` for `GtkFixed`
parents or X11 child windows. Splitter drags would be silent no-ops that still return
`Ok(())`. See ADR 0001.

**Sessions are owned by the Rust core, not by any window, tab or pane.** Panes hold a
`SessionId` and merely *attach*; closing one detaches a sink, it never kills a child. That
single decision is what makes "detach a pane into its own window", "sessions survive a
window closing", and the stack-projects-in-header-vs-one-window-per-project setting all
fall out as the same mechanism. Sinks are a *list*, so detach is gapless — the new window's
sink is live before the old one drops, and not a byte falls between them.

**cide registers as a real Claude Code IDE.** It serves the IDE-integration MCP server
(`openDiff`, `getDiagnostics`, `openFile`, `close_tab`) and sends `selection_changed` and
`at_mentioned`, so Claude's edits arrive as diffs in cide's own editor rather than as ASCII
in a terminal. That surface is undocumented and unversioned, so it lives behind one adapter
with a pinned known-good CLI range and degrades to plain-PTY-only if the handshake fails.

cide never reads `~/.claude/.credentials.json` and never injects `ANTHROPIC_API_KEY` — that
variable outranks subscription OAuth and would silently bill a Console org. Children inherit
their auth by inheriting the environment.
