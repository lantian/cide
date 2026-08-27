# Contributing to cide

Everything needed to build, run and check cide. For what cide *is*, see
[`README.md`](README.md); for the decisions a refactor would otherwise undo, see
[`docs/adr/`](docs/adr/); for how each piece came to be the way it is, see
[`docs/journal.md`](docs/journal.md).

## Prerequisites

- **Rust 1.92.0** — pinned in `rust-toolchain.toml`, so `rustup` picks it up automatically.
- **Node and pnpm** for the frontend.
- **WebKitGTK 4.1 and GTK 3.** On Debian/Ubuntu:

  ```sh
  sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
                          libjavascriptcoregtk-4.1-dev pkg-config
  ```

- The **`claude` CLI**, installed and authenticated — cide hosts it, it does not ship it.
- Optional: the **`openspec` CLI** (`npm install -g @fission-ai/openspec`) for the OpenSpec panel.
  Without it that panel says so and names the install command, and nothing else about the feature
  appears.

## Running it

```sh
pnpm --dir ui install     # once
./run.sh
```

**Use `./run.sh`, not the binary.** A debug build does not load `ui/dist` — `tauri.conf.json`
bakes `devUrl: http://localhost:1420` into the binary, so launching `./target/debug/cide` with
nothing on that port gives a white window and a connection error. `run.sh` builds `cide-app`,
starts Vite, waits for the port, stops any previous instance (SIGTERM, so the workspace flushes),
reaps orphaned `claude` children, and refuses to start when the saved workspace would open more
than eight windows.

That last guard is not hypothetical: a workspace here once accumulated 242 copies of one
directory in per-project window mode, and the restore path faithfully opened a window for each.

```sh
./run.sh --release        # embeds ui/dist, needs no dev server (run `pnpm --dir ui build` first)
./run.sh --profile <name> # run under another profile; `default` shares the real instance's state
./run.sh --fresh          # start from an empty workspace (moves the old one to .bak)
./run.sh --bench          # CIDE_BENCH=1         IPC transport gate (M0), prints and exits
./run.sh --audit-chrome   # CIDE_AUDIT=1         48 chrome dimensions vs the design mock, both themes
./run.sh --audit-panes    # CIDE_AUDIT_PANES=1   100 split/close/maximize cycles over the real domain
./run.sh --audit-windows  # CIDE_AUDIT_WINDOWS=1 detach/re-dock and window modes
./run.sh --inspect        # console into the Rust log + WebKit inspector on 127.0.0.1:9222
./run.sh --on-top         # CIDE_ON_TOP=1, for screenshots (KDE won't raise a shell-launched window)
```

The audits need a display and are not in CI; they are the only checks for the pieces that cannot
be verified by reading them. `CIDE_NO_GRAPHICS_WORKAROUNDS=1` skips the Linux graphics ladder —
the app does not start on stock KDE Wayland without that ladder (ADR 0006).

The binary has a **second mode**, which is not a way to start the app: `cide --wait <file>` opens
that file in the cide already running and blocks until the tab is closed. It is what a pane's
`$EDITOR` points at, so Claude Code's Ctrl+G edits a plan in cide. It needs `$CIDE_EDIT_SOCK`,
which only a child cide spawned has, and says so if run anywhere else.

### `./run.sh` launches the `dev` profile, not your real instance

cide is developed inside cide, so the two run side by side. Before profiles they shared one
`workspace.json` and one Tauri app directory, and whichever exited last stamped its layout over
the other's. A profile moves the whole footprint:

| | default | `dev` |
| --- | --- | --- |
| state | `$XDG_STATE_HOME/cide` | `$XDG_STATE_HOME/cide-dev` |
| config | `$XDG_CONFIG_HOME/cide` | `$XDG_CONFIG_HOME/cide-dev` |
| Tauri identifier | `dev.cide.ide` | `dev.cide.ide.dev` |
| window title | `cide - claude` | `[DEV] cide - claude` |

`cide_core::profile` is the whole rule; `CIDE_PROFILE` is how it is set, and children inherit it,
so `cide-headless tree` in a profiled pane inspects that profile's workspace. It has to be an
environment variable rather than anything derived from the Tauri config, because
`apply_graphics_overrides` reads `workspace.json` off disk *before* the application exists
(ADR 0006).

**A profile starts factory-fresh** — settings live inside `workspace.json`, so a new one has no
installed extensions, no global agent roles and the default keymap. That is the point, and it is
the first thing to remember before filing "my extensions are gone". What profiles do *not*
separate: `.cide/tasks.json`, agent worktrees and `cide/<agent>` branches are per-project, inside
the repository.

### Environment variables

| variable | effect |
| --- | --- |
| `CIDE_PROFILE` | which profile's state and config directories to use |
| `CIDE_BENCH=1` | run the IPC benchmark on first paint, print the report, exit — see [`BENCH.md`](BENCH.md) |
| `CIDE_ON_TOP=1` | build the window `always_on_top`; needed for screenshots, since KDE's focus-stealing prevention keeps a shell-launched window behind everything else |
| `CIDE_AUDIT=1` | measure the chrome against the design mock in both themes at 1440x900 |
| `CIDE_AUDIT_PANES=1` | 100 split/close/maximize/tab-switch cycles, asserting `term.open()` happened exactly once per pane |
| `CIDE_AUDIT_WINDOWS=1` | detach and re-dock a pane, flip the window mode, assert no session was lost |
| `CIDE_NO_GRAPHICS_WORKAROUNDS=1` | skip the Linux graphics ladder, for bisecting a rendering bug against stock behaviour. **The app does not start on stock KDE Wayland without the ladder** |
| `CIDE_RA_PATH`, `CIDE_GOPLS_PATH`, `CIDE_OPENSPEC_PATH` | override the language servers and the OpenSpec CLI with a specific binary |

Put plain `KEY=value` lines in **`.env`** at the repo root to carry overrides on every launch —
`run.sh` sources it, and it is gitignored because the paths in it are one machine's.

### The sibling forks

cide bundles a rust-analyzer fork (ADR 0011) that lives in `../forks/rust-analyzer`, branch
`cide` — never a workspace member — and it drags its sibling `../forks/salsa`, a patched 0.28.2
wired through `[patch.crates-io]`. The gopls checkout is `../forks/tools`, branch `cide`, built as
the `cide-gopls` sidecar. **Each fork carries its own `CLAUDE.md`; read it before touching either
repo.**

A dev build has no bundled sidecar, so it runs the PATH `rust-analyzer` with zero setup. To run
the fork instead, build it once in the sibling and launch with
`CIDE_RA_PATH=$HOME/work/forks/rust-analyzer/target/release/rust-analyzer ./run.sh`.

Packaging is what needs them, all three, at the revisions `packaging/*.lock` pin:

```sh
./scripts/clone-forks.sh     # clones or updates ../forks/{rust-analyzer,salsa,tools} to their pins
```

The script has no URLs of its own — it sources the same locks the release workflow does. Where
those pins come from, how to move one, and how the three repositories relate to their upstreams
is `docs/forks.md`.

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

- **One Rust test:** `cargo test -p cide-git branches`. `--locked` matters — a re-resolved
  lockfile is the drift class CI exists to catch. `cargo xtask` is a `.cargo/config.toml` alias,
  so global flags lead: `cargo --locked xtask codegen`.
- **`#[ignore]`d tests are ignored on purpose.** Most spawn the real `claude` or the real
  `openspec`: they need the binary on PATH, an authenticated account, network, and they spend the
  user's own quota. The rest is `cide-pty`'s 1 GiB soak. Run them deliberately:
  `cargo test --workspace -- --ignored`.
- **The frontend has no test runner.** `ui/scripts/check-*.mjs` *are* the suite: each compiles a
  deliberately import-free module with the TypeScript in `node_modules`, or SSR-bundles an entry
  through Vite, and asserts on the output. Adding a `check:foo` script to `ui/package.json` is
  enough — CI enumerates them rather than listing them. Several modules are import-free *so that*
  their check can compile them standalone; keep them that way.
- **Which check covers what you touched** is the table in [`CLAUDE.md`](CLAUDE.md) — it is
  maintained per surface and names the silent-failure class each check exists to catch. Read the
  row for the area you are editing before you edit it.
- Icon drift: `scripts/gen-icons.sh --check`. Casing collisions: `pnpm --dir ui run check:casing`
  — a name differing from a sibling's only in case is one path on macOS, and it cost a Mac build
  once.

## The wire contract has three gates

1. DTOs are `#[derive(TS)]` types in `crates/cide-ipc`. `cargo test -p cide-ipc` writes
   `crates/cide-ipc/bindings/*.ts`; `cargo xtask codegen` concatenates them into
   **`ui/src/ipc/generated.ts`, which is generated — never hand-edit it.** `codegen --check` fails
   the build when a Rust field rename never reached TypeScript. It writes a second generated file,
   `ui/src/editor/builtinLanguages.ts`, from `cide_ipc::lang::builtins()`.
2. Adding or removing a `#[tauri::command]` (registered in `crates/cide-app/src/lib.rs`) or a
   `cide://` event (all of which go out through `crates/cide-app/src/emit.rs` and nowhere else)
   drifts `contract/{commands,events}.json`. Accept it with
   `cargo xtask contract-check --write`, and move `ui/src/ipc/client.ts` with it.
3. `ui/src/ipc/client.ts` is the frontend's only seam to `invoke`/`listen`.
   `ui/src/chrome/WindowFrame.tsx` is the one documented exception, for window controls.

## Packaging

```sh
./build.sh                # one artefact for this host (AppImage on Linux), installed into ~/bin
./build.sh --all          # the host's whole default set
./build.sh --plan         # build nothing, print what would happen
cargo xtask package [--appimage|--deb|--flatpak|--tarball|--app|--dmg|--src] [--check|--write|--run]
```

Without `--run`, `cargo xtask package` only prints a plan. Naming no target means everything
*this host* is responsible for; naming a bundle the host cannot build (`--dmg` on Linux) is a
preflight failure, because nothing here cross-compiles (ADR 0007). `--src` refuses a dirty tree —
a tarball cut from one is a false claim about a commit.

Releases are `.github/workflows/release.yml`, dispatched by hand with a version: it cuts
`release/v<version>` from master, writes that version into the four files that carry it, tags,
builds every artefact and publishes one release with a `SHA256SUMS`.

## Conventions

- **Comments carry the why, at length, including the option that lost and the bug the code
  prevents.** Match that density; do not "tidy away" a comment that names a failure — most of them
  exist because the failure happened.
- **Only `cide-app` may depend on tauri.** `cide-headless` is the standing proof, and a CI job
  enumerates every workspace member with `cargo tree` and fails on `tauri|wry|tao`. Reaching for
  an `AppHandle` inside domain logic is the signal that the logic is in the wrong crate.
- Third-party versions live only in the root `[workspace.dependencies]`; crates say
  `foo = { workspace = true }`. `wry` and `tao` must never become direct dependencies (0.x, so a
  second version forks the webview stack); `gtk` is pinned to exactly what tauri pins.
- `ui/package.json` pins every version exactly, no carets.
- **Never read `~/.claude/.credentials.json`, never inject `ANTHROPIC_API_KEY`** — it outranks
  subscription OAuth and would silently bill a Console org. Children inherit auth from the
  environment.
- Every child process goes through `cide-core::child_env`: `prepare_command` (ADR 0007) and `arm`
  (ADR 0008). A new `Command::new`/`SpawnSpec` anywhere in the workspace needs both.

## Platforms

**Linux is the only platform cide has ever run on.** [`docs/platforms.md`](docs/platforms.md) is
the record: what a `cfg` arm does on macOS instead, which guarantees have no equivalent there
(`PR_SET_PDEATHSIG` above all), and what still needs a Mac. The `macos` CI job is advisory until
it has passed once. Windows is not a target.
