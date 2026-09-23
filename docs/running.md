# Running cide in development

The detail behind `./run.sh`: profiles, the sibling forks, external CLIs, the one-instance
guard, and the launch flags. `CLAUDE.md` has the short version; `CONTRIBUTING.md` has
prerequisites and the build.

```sh
pnpm --dir ui install     # once
./run.sh
```

## `./run.sh`, not the binary

**Use `./run.sh`, not the binary.** A debug build does not load `ui/dist` — `tauri.conf.json`
bakes `devUrl: http://localhost:1420` into the binary, so launching `./target/debug/cide`
with nothing on that port gives a white window and a connection error. `run.sh` builds
`cide-app`, starts Vite, waits for the port, stops any previous instance (SIGTERM, so the
workspace flushes), reaps orphaned `claude` children, and refuses to start when the saved
workspace would open more than eight windows.

## Keeping `run.sh` portable

It runs on **macOS as well as Linux**, which took three fixes and is easy to undo: no `mapfile`
(bash 4; macOS ships 3.2), no bare `"${array[@]}"` that could be empty (`set -u` aborts on it
before bash 4.4), and no `/proc` — a process's profile is read from `/proc/<pid>/environ` where
that exists and from the profile's own `instance.lock` where it does not. `docs/platforms.md`
has the whole account, and **`./scripts/check-bash32.sh` is what keeps the first two fixed** — a
bash 4 builtin is not a syntax error, so `bash -n` passes, every Linux job passes, and the break
is invisible until a Mac runs the script. It scans every `*.sh` with comments stripped first (the
`read_pids` comment says `mapfile` four times) and fails when a rule stops matching its own
fixture. `run.sh` refuses `sh run.sh` and `zsh run.sh` by name, and names the class when it dies
before launching under bash 3.x.

## Profiles

**`./run.sh` launches the `dev` profile, not your real instance.** cide is developed inside
cide, so the two run side by side; before profiles they shared one `workspace.json` and one
Tauri app directory, and whichever exited last stamped its layout over the other's. A profile
moves the whole footprint — `$XDG_STATE_HOME/cide-dev` and `$XDG_CONFIG_HOME/cide-dev`, and a
suffixed bundle identifier so the WebKit storage, the log and the remembered window geometry
move with it — and prefixes the OS window title with `[DEV] ` so a task switcher tells them
apart. `cide_core::profile` is the whole rule; `CIDE_PROFILE` is how it is set, and children
inherit it, so `cide-headless tree` in a profiled pane inspects that profile's workspace.

## The sibling forks

**The rust-analyzer fork** (ADR 0011) lives in `../forks/rust-analyzer`, branch `cide` —
never a workspace member — and since Phase 1 it drags its sibling **`../forks/salsa`**, a
patched 0.28.2 wired through `[patch.crates-io]` (the manifest's `../salsa` resolves relative
to the fork, so the pair must stay siblings of each other), which the disk
index's tracked-struct restore cannot exist without — and, since Phase 3, its LRU-evicted-memo
disk tier (`memos.redb` beside the snapshot, reloading only what shallow-verifies — ADR 0011,
Decision 5). A dev build has no bundled sidecar, so it
runs the PATH rust-analyzer with zero setup; to run the fork instead, build it once in the
sibling and launch with `CIDE_RA_PATH=$HOME/work/forks/rust-analyzer/target/release/rust-analyzer
./run.sh` — the override outranks both the bundled binary and PATH, and refuses (rather than
falls through) when it points at nothing executable. To carry the overrides on every launch
without retyping them, put plain `KEY=value` lines in **`.env`** at the repo root — run.sh
sources it (allexport) when present, a missing file is simply an empty one, and it is
gitignored because the paths in it are one machine's. Packaging any bundle requires all three
forks at the revisions `packaging/*.lock` pin — `./scripts/clone-forks.sh` checks them out, and
preflight names whichever is missing or off its pin. **`docs/forks.md` is the whole story**: what
each fork is, why the two-sibling rule is load-bearing, how a pin moves, and what CI checks about
one. **Each fork also carries its own `CLAUDE.md`** — the rust-analyzer one is the merge-survival
guide (what must outlive an upstream rebase, and the six invariants that break with no compile
error), the salsa one is a porting guide: it is a real fork of `salsa-rs/salsa` since 2026-08-28,
so a version bump is a `git rebase` onto the next release tag, but never a mechanical one — each
patch is re-argued against what upstream's persistence has grown, and retiring one of ours in
favour of theirs is the good outcome. Read them before touching either repo.

**The gopls checkout** (ADR 0011, Decision 6) is the third fork: `../forks/tools`
(golang/tools — gopls is its `gopls/` module), branch `cide` from the tag
`packaging/gopls.lock` pins, built as the `cide-gopls` sidecar with the host's `go`
(`GOTOOLCHAIN=auto` fetches the toolchain the module demands). **No cide patches yet** — it
builds stock gopls under the cide name; its `CLAUDE.md` records the planned patch surface
(persist the metadata graph, cap the in-memory layer) and the measure-first gate. Dev
override: `CIDE_GOPLS_PATH=$HOME/work/forks/tools/gopls/cide-gopls ./run.sh`. The shipped
gopls is configured through the **env lane** (`cide_lsp::config::extra_env` — `GOPLSCACHE`
into the per-profile cache dir), not `initializationOptions`.

## The `openspec` CLI

**The `openspec` CLI** (ADR 0012) is the one dependency cide neither ships nor bundles. M28's
OpenSpec support reads `openspec/` by running `openspec … --json`, so a machine without it gets a
panel that says so and names `npm install -g @fission-ai/openspec` — and nothing else about the
feature appears. It is installed by `npm -g`, which puts it in a Node directory a *shell* rc file
adds to `PATH` and a desktop launcher does not, while `toolchain::extra_dirs` is `~/.cargo/bin`
and `~/go/bin` and its header forbids widening that list. So `cide_spec::discover` carries its own
rungs (`CIDE_OPENSPEC_PATH` alone and first, then `which`, then `$NVM_BIN`/Volta/pnpm/npm-global
and `~/.nvm/versions/node/*` newest first), and `child_env::run_filter_with` hands the chosen
directory back to the child — because `openspec` is a `#!/usr/bin/env node` script, so without it
`execve` succeeds and the shebang dies with `env: node: No such file or directory`. To see which
answer a given launch gets, run `./target/debug/cide-headless spec <root>`.

## One cide per profile

**One cide per profile, and `cide --help` is not a launch.** Both are new, and both exist
because of the same morning: this binary's path is in `$EDITOR` inside every pane, cide parsed
no argument but `--wait`, and an agent probing the CLI started a whole second IDE on the live
profile — then took down every `claude` in the first instance trying to clean it up.
`cide_app::cli` answers `--help`/`--version` and refuses an unknown flag (a bare path is still
ignored, not refused); `cide_app::instance` writes a pid into
`$XDG_STATE_HOME/cide-<profile>/instance.lock` and refuses a start while that pid is a live
cide. Every doubt — no file, a corrupt one, a dead or unidentifiable pid — starts the
application, because an IDE that will not launch is the worse failure.
`CIDE_ALLOW_SECOND_INSTANCE=1` is the deliberate override; `run.sh` needs it for nothing, since
it stops the profile's running instance first.

## A fresh profile is empty

A profile starts **factory-fresh** — settings live inside `workspace.json`, so a new one has no
installed extensions, no global agent roles and the default keymap. That is the point, and it
is the first thing to remember before filing "my extensions are gone".

## Launch flags

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
