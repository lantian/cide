# Architecture

The structural rules a refactor would otherwise undo. Read the ADRs in `docs/adr/` it names
before changing anything structural.

## The rule that shapes the crate graph

**Only `cide-app` may depend on tauri.** `cide-headless` is the standing proof: it links
`cide-core`, `cide-ipc`, `cide-pty`, `cide-tasks` and `cide-agents` and renders a live session,
a task board and an agent roster to stdout with zero Tauri;
if it stops compiling, domain logic leaked into the app crate. A CI job enumerates every
workspace member with `cargo tree` and fails on `tauri|wry|tao`. Reaching for an `AppHandle`
inside domain logic is the signal that the logic is in the wrong crate.

```
crates/
  cide-app/       the Tauri shell: windows, the command surface, process lifecycle. Glue only.
  cide-ipc/       wire DTOs (serde + ts-rs). Simultaneously in-memory domain, disk format, wire format.
  cide-core/      behaviour over those DTOs, as free functions: workspace tree, keymap, commands,
                  persist, and the VS Code colour-theme importer (`scheme`).
  cide-pty/       PTY sessions: spawn, coalescing, backpressure, vt100 mirror.
  cide-claude/    spawning and supervising `claude`: env, hooks, resume/fork, headless one-shots.
  cide-ide-mcp/   the Claude Code IDE-integration MCP server (openDiff, getDiagnostics, openFile).
  cide-git/       multi-root git, hunk/line staging, changelists, shelf; and the read-only
                  half — log, graph lanes, file history, blame — plus the commit actions,
                  pull's merge/rebase, and the conflict surface (ADR 0009).
  cide-fs/        gitignore-aware indexing and watching.   cide-search/  fuzzy + content search.
  cide-tasks/     The tracker: `.cide/tasks.json` as an index and `.cide/tasks/<id>/task.json`
                  per task (schema 2, M68). One owning store, a repairing loader, a stale-file
                  merge per half, and a migration that converts a schema 1 file on open.
  cide-spec/      OpenSpec, by running its CLI — never by parsing its markdown (ADR 0012).
  cide-docker/    A Docker daemon over the Engine API — never by running `docker` (ADR 0013).
  cide-agents/    `.cide/` roles and config, and the `cide_task_*` MCP vocabulary. Spawns nothing yet.
  cide-lang/      tree-sitter over Rust and Go: what a file declares. No tauri, no cide-fs.
  cide-lsp/       an LSP *client*. Threads, not tokio (see its lib.rs). Since M22 the server set
                  is a registry, not an enum: builtins plus whatever an extension declares.
  cide-ext/       extensions and the git repositories they come from: manifests, marketplaces,
                  installs, and the merge that decides which contribution wins. Runs no
                  extension code — that is a Worker in the webview (ADR 0010).
  cide-hook/      second binary: bridges a Claude hook to the running IDE over a unix socket.
  cide-headless/  third binary: proves the core links without tauri
                  (`cide-headless tree|commands|keymap|tasks|agents|properties`).
```

`docs/adr/` records the decisions that a refactor would otherwise undo — read 0001 (no
multiwebview), 0002 (Rust owns state), 0003 (xterm owns VT) before changing anything
structural, 0009 (real sequencer state) before touching how a conflict is landed (it reverses an
argument still written out at length in `cide-git/src/replay.rs`), and 0010 (extensions out of
realm) before touching anything under `ui/src/ext/`: it is why a contributed panel is a *view
model* rather than a React component, and why a capability check that moved into the worker would
be a check the extension could delete.

## The state loop

Rust owns everything durable. A gesture goes:

```
ui/src/ipc/client.ts → #[tauri::command] in crates/cide-app/src/cmd/*
                     → validated mutation in cide-core::workspace (bumps `rev`)
                     → emit::workspace_changed → EVERY window
                     → ui/src/store/workspace.ts (a mirror; drops any snapshot with a stale rev)
```

`ui/src/store/workspace.ts` never edits the tree — two windows editing one tree would give two
answers, and a detached pane is a separate JavaScript realm with no shared memory. The webview
owns exactly two things: transient gesture state (which overlay is open, whether a splitter is
mid-drag) and live DOM instances (xterm `Terminal`, CodeMirror `EditorView`) that cannot be
serialised. Splitter drags write `gridTemplateColumns` directly and commit `pane_set_ratio`
once, on `pointerup`, because every mutation costs a round trip.

**Sessions live outside the tree**, in a process-global registry keyed by `SessionId`. A pane
holds an id and merely *attaches*; closing a pane, tab or window never touches the child.
`Session.sinks` is a list, not an `Option`, so a detach is gapless — the new window's sink is
live before the old one drops. That one decision is what makes detach-into-a-window,
survive-a-window-close, and the stacked-vs-per-project window setting the same mechanism.

## The wire contract has three gates

1. DTOs are `#[derive(TS)]` types in `crates/cide-ipc`. `cargo test -p cide-ipc` writes
   `crates/cide-ipc/bindings/*.ts`; `cargo xtask codegen` concatenates them into
   **`ui/src/ipc/generated.ts`, which is generated — never hand-edit it.** `codegen --check`
   fails the build when a Rust field rename never reached TypeScript.
2. Adding or removing a `#[tauri::command]` (registered in `crates/cide-app/src/lib.rs`'s
   `generate_handler!`) or a `cide://` event (all of which go out through
   `crates/cide-app/src/emit.rs` and nowhere else) drifts `contract/{commands,events}.json`.
   Accept it with `cargo xtask contract-check --write`, and move `ui/src/ipc/client.ts` with it.
3. `ui/src/ipc/client.ts` is the frontend's only seam to `invoke`/`listen`; that is what keeps
   the surface greppable. `ui/src/chrome/WindowFrame.tsx` is the one documented exception, for
   window controls, which are not commands.

`cargo xtask codegen` writes a **second** generated file since M22:
**`ui/src/editor/builtinLanguages.ts`**, from `cide_ipc::lang::builtins()`. A language's *routing*
— which extensions it claims, what the status bar calls it, how it folds, whether the scratch
picker offers it — has one home, in Rust, because the same table is merged with what an extension
contributes and two copies would drift. What is *not* there is the tokenizer: a builtin's grammar
contains a function (Rust's lifetimes, Markdown's headings) and stays in
`ui/src/editor/languages/<id>.ts`. A contributed one carries `rules` instead — see
`cide_ipc::lang::GrammarRule`.

## Commands and keys are one registry

`cide-core::commands` is the single table behind both the palette and the keymap. Two tables
would let a command be bindable but unlistable, and both drift silently.

- **Ids are API.** A user's `keymap.json` names them. Add freely, never rename.
- Every id is either handled by a `case` in `ui/src/keys/dispatch.ts` or carries an
  `unavailable` reason. Listed-and-silently-inert is the state `check:commands` makes
  unrepresentable — it once described 24 of 37 commands.
- A `when` clause may only name a flag in `CONTEXT_FLAGS`, and every flag must actually be
  supplied by the webview; a flag nobody sets is false for ever and the command silently
  vanishes from the palette.
- Key resolution is layered in Rust (defaults → platform → user `keymap.json`) and normalised;
  `ui/src/keys/keymap.ts` only indexes what it was handed.

## Pane hosts: the load-bearing thirty lines

`ui/src/layout/paneHosts.ts` keeps terminal DOM in a module-level map outside React; `PaneSlot`
merely `appendChild`s it. A destroyed node loses scrollback, selection, renderer and in-flight
turn, and xterm has no restore that survives it. The rules, each a bug if broken: hidden tabs
use `visibility: hidden`, never `display: none` (measurements read zero and `fit()` corrupts
the child's terminal size); never conditionally render a pane or `replaceChildren` a slot;
`term.open()` runs exactly once per pane; `releaseHost` is for a pane leaving this window with
its session alive, `destroyHost` for one that is finished. Only `CIDE_AUDIT_PANES=1` can catch
a regression here.

## PTY and Claude hosting

`cide-pty` owns explicit threads, not tokio (portable-pty's handles are blocking): reader
thread → bounded channel (cap 16) → coalescer → vt100 mirror + sinks. Backpressure is
structural — a full channel blocks the reader, then the kernel buffer, then the child.
Coalescing is a correctness requirement, not tuning: Tauri routes `Channel` payloads under
1024 bytes through `webview.eval` as a JSON array of decimal numbers on the GTK main loop.

Claude is hosted three ways at once (ADR 0005): a real PTY, the IDE-integration MCP server
(one per project, advertised via `~/.claude/ide/<port>.lock`), and hooks through the
`cide-hook` binary. Three invariants:

- **`openDiff` blocks the agent's turn.** Every early return in `cide-app/src/ide.rs::pump`
  must cancel the request first, or the `claude` that asked waits for ever with nothing on
  screen.
- **Never read `~/.claude/.credentials.json`, never inject `ANTHROPIC_API_KEY`** — it outranks
  subscription OAuth and would silently bill a Console org. Children inherit auth from the
  environment.
- **No child inherits the bundle's environment, and every child is armed.** Both rules live in
  `cide-core::child_env`, which is the module every spawn site passes through.
  `prepare_command` (ADR 0007, and since M17 one pass more): an AppImage's `AppRun` leaves
  `PYTHONHOME` and `LD_LIBRARY_PATH` pointing inside a mount, and a `claude` that inherits them
  cannot start a single stdio MCP server — it reports `CONNECTION_CLOSED` from three processes
  below anything cide logs. It also *appends* `cide-core::toolchain::extra_dirs` to the child's
  `PATH`, because a child that inherits a GUI launch's `PATH` cannot find its own toolchain —
  which is what made `gopls` answer `no views` on a Mac where cide had found and started it.
  `arm` (ADR 0008, moved here from `cide-claude` in M12) hands the kernel a pid to kill when cide
  dies, and its contract is that **the forking thread must outlive the child** — which is what
  `on_spawn_thread` is for, and why a language server is never spawned from a Tauri command
  worker. A new `Command::new`/`SpawnSpec` anywhere in the workspace needs both lines.

**The console may be codex (M93).** Settings → Harness picks the CLI a *fresh* console runs.
Each console pane records the CLI it actually runs (`Pane::harness`, stamped by
`bind_session` from `SessionRegistry::note_harness`), and a resume or fork goes to that CLI
whatever the setting says now (`cmd::session::ConsoleSpawn::decide`).

Codex is hosted two of the three ways:
- a real PTY;
- hooks, handed over as `-c hooks.*` overrides with `--dangerously-bypass-hook-trust`, which
  send the same frames through the same `cide-hook` into the same state machine.

It has no IDE MCP client, so `openDiff`, diagnostics and the selection stream are claude-only.
Its thread id is never cide's `SessionId`: it arrives by hook into `Pane::conversation`.
`cide_core::codex_cli` holds the launch rules; codex agent runs use the same TUI hosting
(`harness/codex.rs`).

A `SessionId` *is* the value passed to `claude --session-id`, which is what makes resume free.
Shutdown is a ladder — SIGHUP, SIGTERM, SIGKILL — so a `claude` finishes writing the transcript
that resume depends on; signals arrive through a self-pipe because taking the workspace lock in
a handler deadlocks whenever the interrupted thread already held it.

## On disk

Under a profile every path below has `cide-<profile>` where it says `cide` — one leaf, decided
in `cide_core::persist::xdg_dir` and nowhere else, so an instance's whole footprint moves or
none of it does. Inspect a profile's with `CIDE_PROFILE=<name> ./target/debug/cide-headless tree`.

- `$XDG_STATE_HOME/cide/workspace.json` — the tree. Written atomically, 500 ms debounce; reads
  never fail, because a broken layout must not become a launch loop. Inspect without a GUI:
  `./target/debug/cide-headless tree`.
- `$XDG_CONFIG_HOME/cide/keymap.json` — user binding overrides only (diffs; defaults are
  compiled in).
- `$XDG_CONFIG_HOME/cide/schemes/<id>.json` — imported editor colour schemes, one file each.
  The *converted* scheme, not the source VS Code theme: re-converting at launch would let a
  change to `cide_core::scheme::SCOPES` repaint a buffer somebody was happy with. `cide` is
  not a file — it is what `tokens.css` declares, so selecting it clears the properties.
