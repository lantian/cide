# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

cide is an IDE whose centre of gravity is a live Claude Code session rather than a text
buffer: a pinned, non-closable Claude tab per project hosting a tiling grid of panes. Rust +
Tauri 2 backend, React 19 + Vite 8 frontend, Linux-first (developed on KDE/Wayland).

`README.md` holds the milestone table, and it is the honest record — it names, per milestone,
which acceptance criteria are unmet. Read it before assuming a feature works end to end, and
keep it honest when finishing something.

## Running it

```sh
pnpm --dir ui install     # once
./run.sh
```

**Use `./run.sh`, not the binary.** A debug build does not load `ui/dist` — `tauri.conf.json`
bakes `devUrl: http://localhost:1420` into the binary, so launching `./target/debug/cide`
with nothing on that port gives a white window and a connection error. `run.sh` builds
`cide-app`, starts Vite, waits for the port, stops any previous instance (SIGTERM, so the
workspace flushes), reaps orphaned `claude` children, and refuses to start when the saved
workspace would open more than eight windows.

**`./run.sh` launches the `dev` profile, not your real instance.** cide is developed inside
cide, so the two run side by side; before profiles they shared one `workspace.json` and one
Tauri app directory, and whichever exited last stamped its layout over the other's. A profile
moves the whole footprint — `$XDG_STATE_HOME/cide-dev` and `$XDG_CONFIG_HOME/cide-dev`, and a
suffixed bundle identifier so the WebKit storage, the log and the remembered window geometry
move with it — and prefixes the OS window title with `[DEV] ` so a task switcher tells them
apart. `cide_core::profile` is the whole rule; `CIDE_PROFILE` is how it is set, and children
inherit it, so `cide-headless tree` in a profiled pane inspects that profile's workspace.

A profile starts **factory-fresh** — settings live inside `workspace.json`, so a new one has no
installed extensions, no global agent roles and the default keymap. That is the point, and it
is the first thing to remember before filing "my extensions are gone".

```sh
./run.sh --release        # embeds ui/dist, needs no dev server (run `pnpm --dir ui build` first)
./run.sh --profile <name> # run under another profile; `default` shares the real instance's state
./run.sh --fresh          # start from an empty workspace (moves the old one to .bak)
./run.sh --bench          # CIDE_BENCH=1        IPC transport gate (M0), prints and exits
./run.sh --audit-chrome   # CIDE_AUDIT=1        48 chrome dimensions vs the design mock, both themes
./run.sh --audit-panes    # CIDE_AUDIT_PANES=1  100 split/close/maximize cycles over the real domain
./run.sh --audit-windows  # CIDE_AUDIT_WINDOWS=1 detach/re-dock and window modes
./run.sh --inspect        # console into the Rust log + WebKit inspector on 127.0.0.1:9222
./run.sh --on-top         # CIDE_ON_TOP=1, for screenshots (KDE won't raise a shell-launched window)
```

The audits need a display and are not in CI; they are the only checks for the pieces that
cannot be verified by reading them. `CIDE_NO_GRAPHICS_WORKAROUNDS=1` skips the Linux graphics
ladder — the app does not start on stock KDE Wayland without that ladder (ADR 0006).

## Checking it

Everything CI runs, in CI's order:

```sh
cargo fmt --all --check
cargo --locked xtask contract-check
cargo build --locked --workspace
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo --locked xtask codegen --check
pnpm --dir ui exec tsc --noEmit
pnpm --dir ui run check:<name>          # every check:* script in ui/package.json
pnpm --dir ui build
```

- **One Rust test:** `cargo test -p cide-git branches`. `--locked` matters (a re-resolved
  lockfile is the drift class CI exists to catch); `cargo xtask` is a `.cargo/config.toml`
  alias, so global flags lead: `cargo --locked xtask codegen`.
- **`#[ignore]`d tests are ignored on purpose.** Most spawn the real `claude` — they need the
  binary on PATH, an authenticated account, network, and they spend the user's own quota. The
  rest is `cide-pty`'s 1 GiB soak. Run them deliberately: `cargo test --workspace -- --ignored`.
- **The frontend has no test runner.** `ui/scripts/check-*.mjs` *are* the suite: each compiles
  a deliberately import-free module with the TypeScript in `node_modules` (or SSR-bundles an
  entry through Vite) and asserts on the output. Adding a `check:foo` script to
  `ui/package.json` is enough — CI enumerates them rather than listing them. Several modules
  are import-free *so that* their check can compile them standalone; keep them that way.
- Icon drift: `scripts/gen-icons.sh --check`. Packaging: `cargo xtask package [--appimage|--deb|--flatpak|--tarball|--app|--dmg|--src] [--check|--write|--run]` — without `--run` it only prints a plan. Naming no target means everything *this host* is responsible for; naming a bundle the host cannot build (`--dmg` on Linux) is a preflight failure, because nothing here cross-compiles. `--src` is the exception and the cheap one: `git archive` of HEAD into `target/release/bundle/src/cide-<version>-src.tar.gz`, byte-reproducible, buildable on any host but in the **Linux** default set only, so a release matrix uploads one source tarball rather than two that differ. It refuses a dirty tree — a tarball cut from one is a false claim about a commit. `--tarball` is the *binary* one and makes the opposite promise: `target/release/bundle/tarball/cide-<version>-linux-<arch>.tar.gz`, the two binaries plus the desktop entry, icons, README and LICENSE under a `cide-<version>/` prefix — a tenth the size of the AppImage because it carries no runtime, so it needs the host's WebKitGTK 4.1, and it is compiled output so nothing about its bytes is reproducible. Linux only. `./build.sh` is the wrapper for daily use: one artefact for the host (AppImage on Linux, `.dmg` on macOS), then installs it into `$CIDE_INSTALL_DIR` (default `~/bin`, Linux only — a `.dmg` is opened, not put on PATH). `--all` for the host's whole default set, `--plan` to build nothing, `--no-install` to stop after building. **Releases are `.github/workflows/release.yml`**, dispatched by hand with a version: it cuts `release/v<version>` from master, writes that version into the four files that carry it (Cargo.toml, tauri.conf.json, ui/package.json, and the regenerated flatpak metainfo — they drifted before it existed), tags, builds every artefact through `cargo xtask package`, and publishes one GitHub release with a SHA256SUMS.
- **Linux is the only platform cide has ever run on.** `README.md`'s Platforms section is the record: what a `cfg` arm does on macOS instead, which guarantees have no equivalent there (`PR_SET_PDEATHSIG` above all), and what still needs a Mac. The `macos` CI job is advisory until it has passed once.
- **The macOS arms type-check from here for nine of the twelve crates, and it is worth doing before touching a `cfg`-gated one.** `README.md`'s *Type-checking for macOS from Linux* has the command; it uses `scripts/darwin-cc.sh` and `DOCS_RS=1`. **`cide-app`, `cide-git` and `xtask` are excluded** — `git2` is `vendored-openssl`, so they build OpenSSL for Darwin and that needs a real cross toolchain. So it covers `cide-core`'s `child_env` arms and every other domain crate, and it does *not* cover the crate that links tauri: the two errors a Mac reported in M16 were both in `cide-app` and this would have caught neither. It proves nothing about linking or runtime, and it is not a substitute for the `macos` CI job.

Which check covers what you touched:

| you changed | run |
| --- | --- |
| `keys/`, a default binding, `cide-core::commands` | `check:keys`, `check:commands`, `check:switcher` |
| `sidebar/GitPanel/`, `cide-git` | `check:git`, `check:render`, `check:diff`, `check:diff-render`, `check:branches` |
| **pull, merge, rebase — `cide-git`'s `pull`/`conflict`, the divergence dialog** | `check:pull-strategy`, `check:branches`, plus `cargo test -p cide-git --test pull --test conflicts` (differential against the real `git`, including the merge message byte for byte) |
| the conflict resolver — `panes/MergePane*`, `panes/mergeModel.ts`, `sidebar/GitPanel/MergeBar.tsx` | `check:merge`, `check:render`, `check:theme`, `check:ui-scale` |
| `toolwindow/`, the activity rail, the bottom panel | `check:toolwindow`, `check:toolwindow-render`, `check:sidebar`, `check:menus` |
| `gitlog/`, `cide-git`'s `log`/`lanes`/`show`/`revision` | `check:log`, `check:log-render`, `check:diff-render` |
| blame — `editor/blame*`, `cide-git::blame` | `check:blame`, `check:editor` |
| the task tracker, `.cide/`, agent definitions — `cide-tasks`, `cide-agents`, `sidebar/TasksPanel/`, `sidebar/AgentsPanel/` | `check:agents`, `check:agents-render`, `check:sidebar`, `check:commands` |
| the commit actions — `cide-git`'s `replay`/`reset`/`tag`, `chrome/logActions.ts` | `check:log-actions`, plus `cargo test -p cide-git --test commit_actions` (differential against the real `git`) |
| the file tree, fs ops | `check:tree-status`, `check:tree-flicker`, `check:fs-clipboard`, `check:new-entry` |
| a synthetic tree row — a group, a note, a pin | `check:groups`, `check:notes`, `check:scratch`, `check:tree-drag` |
| `layout/` — splits, dividers, pane grid | `check:rows` |
| **a splitter's drag path, `layout/resizeGesture.ts`, or any `ResizeObserver`** | `check:resize` — a drag is smooth only because the expensive reactions to a size change (xterm's `fit()`, a `session_resize` that reflows the scrollback on the IPC thread, a minimap repaint) are deferred to the end of the gesture. Undo that and nothing throws, nothing changes on screen, and the app simply locks up while somebody drags a divider |
| `editor/` | `check:editor` |
| `editor/markdown/`, the markdown preview | `check:markdown`, `check:editor`, `check:ui-scale` |
| menus, header, chrome, settings | `check:menus`, `check:menu-model`, `check:tab-overflow`, `check:sidebar`, `check:theme`, `check:fonts`, `check:proxy` |
| **any `font-size`, or a box drawn around text** | `check:ui-scale` — chrome type is a closed ladder of `--fs-ui-*` rungs over one `--ui-scale`, and a bare `font-size: <n>px` is a label that silently stops following the UI font size. Looks right at the default, which is where you are working |
| a new sidebar panel, or anything `App.tsx` renders as one | `check:boundary` — an unwrapped panel takes the **whole window** down when it throws, and the rail's choice is restored on launch, so it stays down |
| **any `useWorkspace`/`useStore` selector** | `check:selectors` — a selector that *returns* a fresh array or object re-renders for ever and ends at *Maximum update depth exceeded*, which unmounts the whole root. The render checks SSR the pure views, and one server pass runs no updates, so nothing else in the suite can see it |
| terminal input, session state | `check:input`, `check:exit`, `check:awaiting`, `check:format` |
| **a pane host's lifecycle, or anything that decides when a terminal paints** | `check:render-stall` — xterm pauses its own renderer when `.xterm-screen` reports non-intersecting and resumes only on the same observer, so a pause with no DOM change on the way out leaves a frozen picture over a live buffer. `layout/paneHosts.ts`'s watchdog is what notices; the guards in `terminal/renderStall.ts` are what keep it from firing on the four states where painting nothing is correct |
| overlays, pickers, search | `check:picker`, `check:search`, `check:problems` |
| terminal file links, `cmd/file.rs`'s refusals | `check:paths`, `check:outside-open` |
| `windows/`, detach and re-dock | `check:detached`, `check:window-controls` |
| `cide-lang`, `cide-lsp`, symbol navigation, diagnostics | `check:outline`, `check:problems`, `check:commands`, `check:keys` |
| `ext/`, `sidebar/ExtensionsPanel/`, `cide-ext`, a manifest field | `check:ext`, `check:ext-render`, `check:boundary`, `check:sidebar`, plus `cargo test -p cide-ext` — the manifest refusal table, the path jail, the git route, and, against `../cide-marketplace`, the whole install road |
| **a `TabKind` variant** | `check:tab-overflow`, `check:tab-drag`, plus `cargo test -p cide-app` — a variant is four arms, and the two that get forgotten are `chrome/TabStrip.tsx`'s (a `never` there is a compile error, which is the point) and `closed_tabs::remembered`'s (a miss there is a tab Ctrl+Shift+T silently will not reopen) |
| **a language table, a fold spec, a scratch row, `cide_ipc::lang::builtins`** | `cargo --locked xtask codegen`, then `check:editor` — `ui/src/editor/builtinLanguages.ts` is **generated** from Rust and `codegen --check` is the gate. One registry feeds builtins and extensions alike, so a table edited in the wrong place is a language an extension can no longer supersede |
| **added, renamed or moved any file** | `check:casing` — a name differing from a sibling's only in case is one path on macOS, and it cost a Mac build once (see README's Platforms) |

## Architecture

### The rule that shapes the crate graph

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
  cide-core/      behaviour over those DTOs, as free functions: workspace tree, keymap, commands, persist.
  cide-pty/       PTY sessions: spawn, coalescing, backpressure, vt100 mirror.
  cide-claude/    spawning and supervising `claude`: env, hooks, resume/fork, headless one-shots.
  cide-ide-mcp/   the Claude Code IDE-integration MCP server (openDiff, getDiagnostics, openFile).
  cide-git/       multi-root git, hunk/line staging, changelists, shelf; and the read-only
                  half — log, graph lanes, file history, blame — plus the commit actions,
                  pull's merge/rebase, and the conflict surface (ADR 0009).
  cide-fs/        gitignore-aware indexing and watching.   cide-search/  fuzzy + content search.
  cide-tasks/     `.cide/tasks.json`: one owning store, a repairing loader, a stale-file merge.
  cide-agents/    `.cide/` roles and config, and the `cide_task_*` MCP vocabulary. Spawns nothing yet.
  cide-lang/      tree-sitter over Rust and Go: what a file declares. No tauri, no cide-fs.
  cide-lsp/       an LSP *client*. Threads, not tokio (see its lib.rs). Since M22 the server set
                  is a registry, not an enum: builtins plus whatever an extension declares.
  cide-ext/       extensions and the git repositories they come from: manifests, marketplaces,
                  installs, and the merge that decides which contribution wins. Runs no
                  extension code — that is a Worker in the webview (ADR 0010).
  cide-hook/      second binary: bridges a Claude hook to the running IDE over a unix socket.
  cide-headless/  third binary: proves the core links without tauri
                  (`cide-headless tree|commands|keymap|tasks|agents`).
```

`docs/adr/` records the decisions that a refactor would otherwise undo — read 0001 (no
multiwebview), 0002 (Rust owns state), 0003 (xterm owns VT) before changing anything
structural, 0009 (real sequencer state) before touching how a conflict is landed (it reverses an
argument still written out at length in `cide-git/src/replay.rs`), and 0010 (extensions out of
realm) before touching anything under `ui/src/ext/`: it is why a contributed panel is a *view
model* rather than a React component, and why a capability check that moved into the worker would
be a check the extension could delete.

### The state loop

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

### The wire contract has three gates

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

### Commands and keys are one registry

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

### Pane hosts: the load-bearing thirty lines

`ui/src/layout/paneHosts.ts` keeps terminal DOM in a module-level map outside React; `PaneSlot`
merely `appendChild`s it. A destroyed node loses scrollback, selection, renderer and in-flight
turn, and xterm has no restore that survives it. The rules, each a bug if broken: hidden tabs
use `visibility: hidden`, never `display: none` (measurements read zero and `fit()` corrupts
the child's terminal size); never conditionally render a pane or `replaceChildren` a slot;
`term.open()` runs exactly once per pane; `releaseHost` is for a pane leaving this window with
its session alive, `destroyHost` for one that is finished. Only `CIDE_AUDIT_PANES=1` can catch
a regression here.

### PTY and Claude hosting

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

A `SessionId` *is* the value passed to `claude --session-id`, which is what makes resume free.
Shutdown is a ladder — SIGHUP, SIGTERM, SIGKILL — so a `claude` finishes writing the transcript
that resume depends on; signals arrive through a self-pipe because taking the workspace lock in
a handler deadlocks whenever the interrupted thread already held it.

### On disk

Under a profile every path below has `cide-<profile>` where it says `cide` — one leaf, decided
in `cide_core::persist::xdg_dir` and nowhere else, so an instance's whole footprint moves or
none of it does. Inspect a profile's with `CIDE_PROFILE=<name> ./target/debug/cide-headless tree`.

- `$XDG_STATE_HOME/cide/workspace.json` — the tree. Written atomically, 500 ms debounce; reads
  never fail, because a broken layout must not become a launch loop. Inspect without a GUI:
  `./target/debug/cide-headless tree`.
- `$XDG_CONFIG_HOME/cide/keymap.json` — user binding overrides only (diffs; defaults are
  compiled in).

## Conventions

- **Comments here carry the why, at length, including the option that lost and the bug the
  code prevents.** Match that density; do not "tidy away" a comment that names a failure — most
  of them exist because the failure happened.
- Third-party versions live only in the root `[workspace.dependencies]`; crates say
  `foo = { workspace = true }`. `wry` and `tao` must never become direct dependencies (0.x, so
  a second version forks the webview stack); `gtk` is pinned to exactly what tauri pins.
- `ui/package.json` pins every version exactly, no carets — Vite 8's Rolldown/Oxc pipeline
  makes an unannounced plugin break expensive.
- Agent worktrees live in `.claude/worktrees/` and are gitignored; a blind `git add -A` during a
  parallel run would otherwise commit several complete copies of the tree.
