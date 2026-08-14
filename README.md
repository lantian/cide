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

## Language support (M12), and what is not done

Rust and Go get a tree-sitter symbol layer (`cide-lang`) and a language-server client
(`cide-lsp`). The honest state, because half of this is data with no surface on top of it yet:

**Works, and is checked.** `Ctrl+F12` (File Structure popup), `Ctrl+Alt+Shift+N` (Go to Symbol in
project), `Alt+Up`/`Alt+Down` (previous/next member), the `getDiagnostics` MCP tool answering from
a real store, and the whole Rust pipeline underneath: extraction for both languages, a parallel
project walk, the merged diagnostic store, the LSP codec/session/supervisor.

**Verified against real servers.** `cargo test -p cide-lsp -- --ignored` drives the real binaries;
**CI does not run it**, so run it by hand after touching that crate. All six pass — `gopls`
reporting on a module, rust-analyzer indexing this workspace and reporting an introduced type
error, the shutdown ladder actually stopping a server, the on-disk-edit test below, and a real
`textDocument/definition` resolving a reference to its declaration.

Verified in the app, too, once: with a type error planted in `cide-core` *after* the build, a
launched binary logged `published … child_env.rs n=2 "mismatched types: expected u32, found &str"`
alongside an empty publish for the open editor tab — the full chain from spawn to store, and the
empty one is the `[]`-means-looked case the panel is built on.

A rustup caveat worth knowing, because it is not a cide bug and reads exactly like one:
`~/.cargo/bin/rust-analyzer` is a symlink to `rustup`, so it passes any "on PATH and executable"
probe and then fails at exec if the component is missing for the toolchain that project resolves
to. A repo pinned by `rust-toolchain.toml` gets the pinned one; a repo with no pin gets the default,
which may not have the component. `start_failure_reason` recognises the shim's own wording and the
panel says `rustup component add rust-analyzer` rather than reporting a crash.

Running them is what found the bug they now guard against. `Ready` was computed as "handshake done
and no progress token in flight", which is right for a server that indexes in one pass and wrong
for rust-analyzer, whose indexing is a *sequence* of tokens — so in the gaps between them the
source reported **Ready with an empty list**, i.e. a clean bill of health, eight times in the first
0.9 seconds against a workspace it had not read yet. The first version of the test passed anyway,
because it only asserted that `Scanning` appeared before `Ready` and `Session::new` makes that
trivially true. `READY_SETTLE` holds a `Ready` for a second before believing it, and the test now
asserts that no `Ready` is ever followed by a `Scanning`.

**Two bugs that a fully green gate did not see**, both found by running the thing and reading the
log rather than by any check, and both with the same shape — a feature that was built, registered,
reachable and never called:

- **No language server started on a normal launch.** `DiagnosticsRegistry::ensure` was called only
  from `project_open`, which a *restoring* launch never reaches, because the projects arrive from
  `workspace.json`. `lib.rs` already carried a paragraph about this exact omission costing the IDE
  servers their headline feature "on every launch but the first"; the language servers repeated it
  a hundred lines below. The symptom was an `unavailable` panel, `✗ — ⚠ —`, and `getDiagnostics`
  answering `[]` — each one indistinguishable from a clean workspace.
- **`getDiagnostics` was never wired to a store.** `IdeServers::ensure` set the source only if the
  diagnostics registry already held the project, guarded by a comment calling the order a race that
  "degrades to honesty". It was not a race: `project_open` calls the IDE `ensure` first, every
  time, so the guard was always false. The link is now made from both ends (`ide::link_diagnostics`)
  so the order genuinely does not matter.

**Document sync**, and why it is not optional. `didOpen`/`didChange`/`didSave`/`didClose` existed as
commands, were registered, and were exported in `client.ts` — with no caller. `cide-lsp`'s
`an_on_disk_edit_alone_never_refreshes_diagnostics` measures what that cost: flycheck runs once when
the workspace finishes loading, so a project **opens** with a correct list and looks fine, but
repairing a file on disk and telling the server nothing changes nothing, for as long as you care to
wait. The panel froze at the state the project opened in while the user edited underneath it, which
is worse than showing nothing. `ui/src/editor/docSync.ts` now sends all four, refcounted by path so
a split is one open document, and flushes the pending change *before* `didSave` so the server never
checks text from 300 ms ago.

**On screen and fed.** The Problems panel renders a live snapshot, the status bar's counts and the
⚑ rail badge are derived from that *same* snapshot (so the three cannot disagree about whether the
workspace is clean), and Settings ▸ Inspections drives all of it. The status-bar trail now carries
the caret's `mod › impl › fn` chain after the path.

**In the editor.** Squiggles and a gutter column, via `@codemirror/lint` — `text-decoration` in
theme tokens rather than the library's stock inline-SVG data URIs, which bake `#d11` and `orange`
and cannot read a custom property. The gutter glyphs are the panel's own `✗ ⚠ ℹ ·`, so margin and
panel share one alphabet. The per-editor highlighting level is three checked items in the code
pane's context menu; it is session-scoped over a persisted default, deliberately — a `none` set
three weeks ago and silently restored is a user concluding their language server is broken.

Go has a real grammar now (`func`, `chan`, `defer`, `select`, raw strings, rune literals,
non-nesting block comments) instead of borrowing Java's keyword table through `clike`. The outline
re-parses 300 ms after you stop typing, so the breadcrumb, the popup and the member walk follow the
buffer rather than the last save.

**Still not built.** "Fix with Claude" on a problem row, and Claude-authored inspections — the
`claude` source toggle exists and nothing produces findings for it. A row action needs the
host/pure split `GitPanelHost` uses, so the panel's SSR smoke test keeps working. Breadcrumbs are
drawn but **not clickable**: `StatusBar` receives a flat list and does not know where the path ends
and the symbols begin.

**Go to definition works, and it resolves rather than guesses.** `Ctrl+B`, `Ctrl+Click`, or the code
pane's context menu. It asks the language server `textDocument/definition` and does nothing else —
in particular it does **not** fall back to the symbol index, because jumping to whichever of the
eleven `fn new` in a workspace shares the identifier's spelling is right about one time in eleven,
and a confident wrong answer is worse than the disabled item it replaces. With no server running it
says which server is missing.

That meant building a request/response path in `cide-lsp`, which until now was strictly one-way:
notifications out, `LspEvent`s back. `Session::on_message` returns an empty effect vec for any
response whose id is not `initialize_id`, so a reply was parsed and dropped with no log line
anywhere. Replies are now intercepted in the supervisor pump *before* `Session` sees them and handed
to a per-request channel — not a new `LspEvent`, because `drain()` has one consumer and a command
reaching in to find its own reply would swallow the `Published` events the diagnostics pump needed.
Request ids start at 100: `initialize` is always 1 and `shutdown` always 2, **per life of the
server**, so a naive counter would eventually have a caller's request resolved by a handshake reply.
Ten unit tests cover the correlation, and
`cargo test -p cide-lsp -- --ignored` proves it against a real rust-analyzer.

**The click modifiers changed to match IDEA.** Ctrl+Click was CodeMirror's default multi-cursor
modifier on Linux (`clickAddsSelectionRange` is `browser.mac ? metaKey : ctrlKey`); it is now Go to
Definition, and adding a caret moved to **Alt+Click**. One facet override does both, because
`rectangularSelection` already claims Alt and its style consults that same facet to decide whether
to add or replace. The honest divergence: Alt+**drag** now adds its rectangle to the selection
rather than replacing it, where IDEA replaces.

An adversarial review of this work confirmed thirteen defects, nine in the new code, and every one
of them was invisible to the gate that had just gone green. Three were the same shape as the bugs
above — cancellation that read as complete and was not (`supervise` cancelled waiters *around* the
`run_once` call, so the stop-check that returns before it left a caller to sit out its five-second
deadline and then be told "still indexing" about a server that had been deliberately stopped); a
restart backoff that popped a queued request off the outbox and discarded it, cancelling nothing,
while also letting a single `didChange` skip the rest of the backoff and turn a crash loop into a
respawn storm; and a `.catch(() => {})` on `file.open` whose comment claimed `Failures` would show
the error anyway, when catching it is precisely what stops `unhandledrejection` from firing.

Two more were in the highlighting-level axis and predate this feature. `App.tsx` was applying the
per-editor level to the Problems panel, the status bar and the rail badge, so a default of `None`
produced `✗ 0 ⚠ 0` over a workspace full of errors — the confident zero this whole surface exists to
prevent, arriving through the one axis the design says must never reach it. And `EditorPane` read
`levelFor(path, 'all')` with the fallback hardcoded, which made the setting inert for every editor,
the one surface it is defined for. Both are fixed; the level is now applied exactly once, to the
buffer it belongs to.

**Still not wired**, and named here rather than discovered: `clearLevel`, `overrideFor` and
`reducedCount` in `highlightLevel.ts` are documented as feeding the context menu and the panel's
detail line, and are called only by the check script. There is no "reset this file to the default"
affordance and the panel does not say when an editor is showing less than it holds.

**Known limits of what does work.** No 100k-file repository has been indexed — the caps in
`cide_lang::Limits` are reasoned, not measured. Go to definition has three of its own, all
deliberate: a definition in a file no pane has open resolves against **on-disk** text, because
`didOpen` is only sent for buffers an editor mounted; a reveal is delivered to *every* editor
showing that path, so a split showing one file twice moves both carets; and a reveal requested from
a detached pane does not cross into the shell window, so a cross-window jump into a closed file
opens it at line 1. The lookup gives up after five seconds and says the server is still indexing —
which, for the first minute of a session on a large workspace, is the truthful answer.


## Verifying the Claude Code CLI

The IDE integration is reverse-engineered from a surface that is undocumented, unversioned and
self-updating. There is no protocol-generation field anywhere in the CLI, so the only question
that can be answered is not "which protocol is this" but **"is this a version anybody checked"**.

```sh
cargo xtask verify-cli              # rebuild, then check the installed CLI
cargo xtask verify-cli --no-build   # reuse what is already compiled
```

It preflights before spending anything. `claude` must be on `PATH`, and so must `script(1)`:
the CLI opens an IDE connection only from its interactive UI — a `-p` run opens no socket at
all — so the check needs a pty, and `script` is how it gets one. The binary it chose and what
`claude --version` says are printed *before* the run, so a hang has a version attached to it in
the scrollback.

Then one test: a real `claude` under a pty, which must find our lockfile, choose the WebSocket
transport, send the `x-claude-code-ide-authorization` header and complete the MCP handshake.
**It types no prompt.** No model is called and nothing is billed, which is what lets this be a
hard failure rather than a skip — the expensive test beside it,
`the_real_cli_round_trips_a_diff_three_ways`, spends a model turn and skips on anything it does
not recognise, and a skip-shaped version check is worth nothing.

**The handshake is checked before the version, and that order is load-bearing.** A green version
check on a build whose protocol had already broken would be a confident lie.

|                      | version in the record                  | version past it                                              |
| -------------------- | -------------------------------------- | ------------------------------------------------------------ |
| **handshake OK**     | pass                                   | fail — append the version, with this run as its evidence      |
| **handshake broken** | fail loudest — the record is a lie      | fail — real drift: read the transcript, fix `protocol.rs`, *then* append |

To record a new version, append it to `SUPPORTED_CLI` in `crates/cide-ide-mcp/src/protocol.rs`
and put the run's output in the module comment above it. That comment is the evidence trail, and
it is worth reading before assuming a release is harmless: 2.1.227 changed nothing in the IDE
protocol but *did* begin rejecting `--resume <id> --session-id <new>`, which broke resume until
`cide_claude::session` was corrected.

The record is **never enforced**. Refusing to run outside the range would break the app roughly
every fortnight to protect a feature that mostly keeps working across releases, so the strongest
thing here is a warning: a protocol change should degrade the diff view, never break the terminal.

### What a user sees

A green test says nothing about *someone else's* machine, and a test that wrote into the app's
settings would be recording a claim about a developer's laptop into a file a user then reads. So
the two halves are separate. At runtime the app writes whichever `claude` last completed a real
IDE handshake **on this machine** to `claude-handshake.json` in the state directory, and Settings
shows it with a badge saying whether that version is one this build was verified against.

Neither half reads `~/.claude/.credentials.json` and neither sets `ANTHROPIC_API_KEY`. The child
authenticates by inheriting the environment; that is the only supported path, and injecting a key
would silently bill a Console org for a user on a subscription.

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

## Opening a file a pane printed, including one outside the project

Ctrl+click a path in any terminal pane and it opens as a tab, at the line and column the
producer named. The bytes a pane prints are attacker-influenced by definition — a build log, a
tool result, an agent's transcript — so `terminal_open_path` is the one command in the app whose
path argument is untrusted, and it is the only route from a pane to the tab list.

Four guards sit on that path and they answer four different questions. Only one of them is about
the project boundary:

| guard | concern | can the user overrule it? |
| --- | --- | --- |
| absolute, no `..` | integrity — a path that resolves differently depending on who resolves it | no |
| inside a root (textually, then again after `canonicalize`) | confidentiality — any readable file on the machine | **yes, per click, by name** |
| a regular file | liveness — a FIFO parks a blocking-pool worker in `read_to_end` for ever; `/dev/zero` reports length 0 and is read until the process is OOM-killed | no |
| under the editor's size limit | junk — a 2 GB log becomes a tab and then an error | no |

The second one is a **question**, not a verdict: the refusal comes back carrying the canonical
path, a confirmation names it in full — and names the symlink target too, when the link resolves
somewhere else — and the answer that goes back to Rust is *that path*, not a boolean, so an
approval is an approval of a file rather than of a string. There is no "don't ask again": one
approval becoming a standing capability for every later line naming a sibling file is the thing
the gate exists to prevent, and the second line is written by whoever wrote the first.

A path is only offered as a link if it is really there. In-project candidates are answered from
the file index at no syscall cost; out-of-project ones get a real `stat` through `fs_stat_paths`,
because a linker error names `/usr/bin/ld`, `/dev/null` and half a dozen `.so`s, and underlining
all of them is how an underline stops meaning "cide can open this". **A relative path never
becomes an out-of-project candidate** — resolving `../../.ssh/id_rsa` against the pane's cwd
would produce a real private key from six characters on screen — so the property the
confirmation rests on holds: the full path was printed, and the user could read it before
clicking.

What such a tab then does: the status bar shows the whole absolute path (`pathTrail` falls back
to it), Ctrl+F12 and the member walk work, save works, and *Reveal in Files* says there is no row
rather than doing nothing. Diagnostics are the ragged edge — an out-of-project `.rs` is still
routed to the project's rust-analyzer by extension, which answers "file not included in module
tree" or nothing at all.

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
  cide-lang/       tree-sitter: what a Rust or Go file declares.                 (M12)
  cide-lsp/        An LSP *client*: rust-analyzer and gopls.                     (M12)
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
