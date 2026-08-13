# ADR 0007 — Linux packaging: AppImage is the channel, .deb is a convenience, Flatpak is awkward

**Status:** accepted (M11)
**Date:** 2026-08-09

## Context

cide's central dependency is the `claude` CLI, which lives on the user's machine and updates
itself — 2.1.221 → 2.1.226 over the course of this project. The IDE integration protocol it
speaks is undocumented and carries no version field (ADR 0005). A cide build that cannot
update itself will, within weeks, be running reverse-engineered constants against a CLI
several releases ahead of them.

So the distribution question is not "which formats are nice to have". It is "which format can
push a fix to a user who installed months ago", and on Linux `tauri-plugin-updater` answers
that with exactly one: **AppImage**. That is the whole of its Linux support, not a preference.

## Decision

Three artefacts, with clearly different standing.

### AppImage — the channel

The build users are pointed at, and the only one that can self-update. Produced by
`cargo tauri build --bundles appimage`.

It needs **`cargo-tauri` 2.x**. The version already installed on the reference machine was
`1.5.12` — what `cargo install tauri-cli` gave people for years — and it reads this v2
`tauri.conf.json` against the v1 schema and dies a long way from anything that says "wrong
version". The preflight's old check only asked whether `cargo-tauri` existed, so it printed
`ok` for that build; it now runs `cargo-tauri --version` and fails on a major other than 2.

`plugins.updater` is **not yet configured** in `crates/cide-app/tauri.conf.json`: it needs a
signing key pair and a release endpoint, neither of which exists at M11. `cargo xtask package`
reports this as a warning rather than a failure — an AppImage without an update endpoint is
still a working AppImage — but the warning is the reminder that the reason this format was
chosen is not yet switched on.

### `cide-hook` rides along as a sidecar, because the bundler would otherwise leave it out

`cargo tauri build` bundles the `[[bin]]` targets of the crate that holds `tauri.conf.json`
and nothing else — `tauri-cli`'s `get_binaries` reads that one `Cargo.toml` plus its
`src/bin/`. `cide-hook` is a separate workspace crate and has to stay one: it may not link
tauri, and `crates/cide-app/Cargo.toml` says in as many words that `cide-app` is the only
crate that may.

Left alone, then, the bundler produces an AppImage containing only `cide`. That package
installs, launches, looks entirely correct, and every Claude session inside it runs with no
hooks: no token figures, no fast buffer reload, and a close confirm that cannot tell busy from
idle. `cmd::session::hook_settings` returns `None`, logs one `warn`, and the session proceeds.
Nothing fails.

So `bundle.externalBin` names `../../target/release/cide-hook`. The bundler resolves an
`externalBin` entry by appending the target triple, and `Settings::copy_binaries` strips the
triple back off when it copies the file into the package's `usr/bin/` — beside `cide`, which is
exactly where `hook_settings` looks (`current_exe().parent().join("cide-hook")`). Inside a
mounted AppImage `current_exe()` is `$APPDIR/usr/bin/cide`, so the two agree.

Producing the triple-suffixed copy is two steps in `cargo xtask package`'s plan, ahead of the
bundler:

```sh
cargo build --release --locked -p cide-hook
install -m755 target/release/cide-hook target/release/cide-hook-x86_64-unknown-linux-gnu
```

The rejected alternative was `bundle.linux.appimage.files`, which takes a plain dest→src map
and needs no triple dance. It is AppImage-only: the `.deb` would have gone on shipping without
the hook, and the failure is invisible, so a mechanism that covers only one of the two formats
is the wrong one.

#### …but the key may not live in `tauri.conf.json`

`bundle.externalBin` is not read only by the bundler. `tauri-build` — `cide-app`'s build script
— copies every named sidecar into the target directory on **every** cargo invocation, and
fails with `resource path ... doesn't exist` when one is missing. The first version of this
change put the key in `crates/cide-app/tauri.conf.json`, and the result was that `cargo build`,
`cargo test --workspace` and `cargo clippy --workspace` all failed on any tree that had not
already produced a *release* `target/release/cide-hook-<triple>`. That is every fresh clone,
and it is `.github/workflows/ci.yml`'s `cargo build --locked --workspace` step, which runs long
before anything packages anything. The symptom reads as a broken checkout rather than as a
packaging decision, and the machine it was developed on could not see it: the packaging run had
already left the sidecar in place.

The key therefore lives in `crates/cide-app/tauri.bundle.conf.json`, a merge patch nothing
reads unless it is asked to, and the plan asks:

```sh
cd crates/cide-app && cargo tauri build --config tauri.bundle.conf.json --bundles appimage,deb
```

`tauri-cli` merges that patch over `tauri.conf.json` for its own bundling **and** exports the
merged patch as `TAURI_CONFIG` (`src/helpers/config.rs`), which `tauri-build` reads — so the
sidecar reaches both halves of the build, and only during a packaging run. `--config` resolves
its path against the process working directory, which is why the flag's value is the bare file
name and the step's `cwd` is the app crate.

`tauri.linux.conf.json`, the platform-overlay file, was the other candidate and loses for the
same reason as the base config: `tauri_utils::config::parse::read_platform` is called by
`tauri-build` too, so it breaks the workspace build identically on the only platform this file
targets.

Six tests guard the pair: that the overlay still names the hook, that a config without it is a
preflight *failure* rather than a warning, that the plan builds and suffixes the binary before
the bundler runs, that the bundler step passes `--config`, that the base config carries no
`externalBin` at all, and that the path the bundler will open is byte-for-byte the path the plan
writes. That last one replaces an `ends_with("target/release/cide-hook")` assertion that a
config entry missing its `../../` also satisfied — green for precisely the drift it named.

### The bundle's environment stops at the bundle

An AppImage is a mount, and `AppRun` — AppImageKit's entry point, four processes above cide —
rewrites nine environment variables so the bundled binary finds the bundled libraries:
`PATH`, `LD_LIBRARY_PATH`, `PYTHONHOME`, `PYTHONPATH`, `XDG_DATA_DIRS`, `GSETTINGS_SCHEMA_DIR`,
`PERLLIB`, `QT_PLUGIN_PATH`, `GST_PLUGIN_SYSTEM_PATH*`, plus `APPDIR`, `APPIMAGE`, `ARGV0`,
`OWD` and the `linuxdeploy` GTK hook's loader caches. Every one of them is inherited by every
descendant, and **cide's descendants are the user's programs, not ours.**

The first report was an MCP server that would not start in a pane and started fine in a
terminal, with identical `~/.claude.json` entries. `uvx yandex-tracker-mcp` spawns Python 3.13,
which honours the inherited `PYTHONHOME=$APPDIR/usr/` — a prefix inside a bundle that contains
no Python at all — and dies with `Failed to import encodings module` before it can write one
byte of protocol. What the user sees is `CONNECTION_CLOSED` against a configuration that is
correct, two processes below anything cide logs. `LD_LIBRARY_PATH` is the same hazard for any
child linking one of the 160 libraries in the bundle, and surfaces as a symbol lookup error.

`cide_core::child_env` is the answer, and it is a rule about **values, not variable names**: an
entry that lives under `$APPDIR` is dropped from a child's environment, a variable left with
nothing is unset rather than left empty, and everything else is passed through untouched.
`AppRun` prepends, so filtering restores exactly what the user's session had. A name list would
have covered AppImageKit 13 and the GTK hook and silently stopped covering the next plugin;
this covers anything of the same shape. Dropping *empty* entries is part of it — the residue of
a prepend onto nothing is `PYTHONPATH=:`, and an empty entry means the current directory, so
passing that on would trade a dead interpreter for one importing out of the pane's cwd.

Four spawn sites take it, which is all of them: `cmd::session::base_env` (every PTY child,
shells and Claude panes alike, via `SpawnSpec`), `cide_claude::headless` (the one-shot lane),
`cmd::app::claude_version`, and `cide_git`'s binary route for `push`/`fetch`. Nothing is
scrubbed when `APPDIR` is unset, so `./run.sh`, the `.deb` and the Flatpak produce byte-identical
environments to the ones they produced before.

Two things are deliberately **not** rescued. A variable `AppRun` *overwrote* rather than
prepended to cannot be recovered — `PYTHONHOME` is the only one, and a user who had their own
loses it, which still beats passing on a path that ceases to exist when cide exits. And
`GDK_BACKEND=x11`, `GTK_THEME=Adwaita:dark` and `PYTHONDONTWRITEBYTECODE=1` are left alone: they
name nothing inside the bundle, so nothing distinguishes them from the same variables set by the
user's own profile, and what they cost a child is cosmetic where the others cost it its life.

### .deb — a convenience

For people who want their package manager to own the install. It cannot self-update and is
not expected to; a distribution package that rewrites itself behind `dpkg` would be worse than
one that does not.

### Flatpak — a manifest, and an honest warning

`packaging/flatpak/dev.cide.ide.yml` is generated by `cargo xtask package --write`, along with
the `.desktop` entry and the AppStream metainfo, because all three restate the app id, version
and binary names from `tauri.conf.json`, and a hand-maintained copy that drifts produces a
package which installs and then cannot find its own executable. `--check` is a gate.

**Flatpak fits this application badly, and the reason is structural.** cide's core operation is
spawning `claude` — a host binary that updates itself and holds the user's OAuth credentials
in the host keychain. A sandbox has neither. Making it work needs `--filesystem=host` *and*
`--talk-name=org.freedesktop.Flatpak` (to reach the host through `flatpak-spawn`), at which
point most of the sandbox's value has been spent on getting out of it. Both are in the
manifest and both are asserted by tests, because without them the package installs, launches,
and fails to spawn a single Claude pane.

Two further caveats are recorded in the manifest itself:

- **The build needs network access**, which Flathub's own builders forbid. A Flathub
  submission additionally needs offline source manifests generated by flatpak-builder-tools
  (`flatpak-cargo-generator.py`, `flatpak-node-generator`). The checked-in manifest is for
  local builds.
- **The frontend is installed with `npm install`, not the pnpm lockfile.** The GNOME node22
  SDK extension ships npm and not pnpm, and `npm ci` is not an option: this repository has
  `ui/pnpm-lock.yaml` and no `package-lock.json`, and npm exits `EUSAGE` rather than falling
  back (`The npm ci command can only install with an existing package-lock.json`). So Flatpak
  is the one channel whose dependency tree is not the lockfile's. Direct dependencies are
  pinned exactly in `ui/package.json` — no carets, bar `@tauri-apps/plugin-dialog` — so the
  drift is confined to transitive versions, but it is real and it is why an AppImage and a
  Flatpak of the same commit are not the same build. Resolving it properly means either
  committing a `package-lock.json` alongside the pnpm one or generating the offline node
  sources a Flathub submission needs anyway.
- **The runtime is pinned to `org.gnome.Platform//49`**, verified as offered by flathub on
  2026-08-09 (`flatpak remote-ls flathub --runtime` listed 49 and 50, nothing older). What is
  **not** verified is that this runtime carries the WebKitGTK **4.1** (GTK 3) ABI Tauri 2
  links, rather than only 6.0 (GTK 4). Confirming it needs the runtime installed, which is a
  multi-gigabyte download the packaging task will not perform on its own. This is the open
  risk of the Flatpak channel and it is the first thing to check before shipping one.

### libgit2 and OpenSSL are vendored, not taken from the runtime

`git2` is built with `vendored-libgit2` and `vendored-openssl` (workspace `Cargo.toml`), so
both are compiled into the binary in every format. This is what lets the same binary run on a
distribution whose system libgit2 is three years old, and it removes the class of breakage
where a runtime upgrade changes an ABI underneath a package. The cost is a longer build and a
perl dependency for OpenSSL's own build scripts, which every SDK here provides. The Flatpak
manifest deliberately contains **no** libgit2 or openssl module; a test asserts as much,
because adding one would silently take precedence and reintroduce exactly what vendoring
avoids.

## How to run it

```sh
cargo xtask package                 # preflight every target, print the plan, build nothing
cargo xtask package --appimage      # preflight one target
cargo xtask package --write         # regenerate packaging/flatpak/*
cargo xtask package --check         # fail if those files are stale (CI gate)
cargo xtask package --run           # actually build
```

The default builds nothing. Bundling needs a release build of the whole workspace plus a
produced `ui/dist` — a twenty-minute job on the reference machine — and running that as a side
effect of a command somebody typed to see what it would do is hostile.

`--run` for the Tauri targets executes:

```sh
cargo build --release --locked -p cide-hook
install -m755 target/release/cide-hook target/release/cide-hook-<host triple>
cd crates/cide-app && cargo tauri build --config tauri.bundle.conf.json --bundles appimage,deb
```

(plus a `curl` for the AppImage runtime, below, when `target/appimage-runtime/` is empty.)

The last from the app crate, because `cargo tauri build` finds `tauri.conf.json` by walking up
from the working directory and this workspace has no `src-tauri`. That config's
`beforeBuildCommand` already runs `pnpm build` in `ui/`, so the frontend is not a separate
step; adding one would build it twice. The host triple comes from `rustc -vV` rather than
`std::env::consts`, which knows the arch and the OS but not the vendor or the libc. Artefacts
land in `target/release/bundle/`, and the task prints their sizes — a bundle that came out at
12 MiB has not picked up WebKit's helper processes, and the number is the cheapest way to
notice.

`--run` for Flatpak executes:

```sh
flatpak-builder --force-clean --install --user target/flatpak/dev.cide.ide \
  packaging/flatpak/dev.cide.ide.yml
```

Installed into the user's own installation rather than published to a repository: publishing
needs signing keys the task must not go looking for.

### appimagetool fetches a runtime, and when that stalls the build hangs forever

The innermost layer of `cargo tauri build --bundles appimage` is `appimagetool`, four
processes down (`cargo tauri` → `linuxdeploy` → `linuxdeploy-plugin-appimage` →
`appimagetool`). Given no runtime file it downloads one from
`github.com/AppImage/type2-runtime`. On the reference machine that download stalled: the
socket sat in `CLOSE-WAIT`, the process in `futex_do_wait`, and it stayed there. There is no
timeout anywhere in that stack and `cargo tauri build` holds the pipes, so the run printed
`Bundling cide_0.1.0_amd64.AppImage` and then nothing, indefinitely — a last line that reads
like success.

So the plan fetches the runtime itself, with `curl --retry 3 --max-time 180`, into
`target/appimage-runtime/`, and hands it down as `LDAI_RUNTIME_FILE` — the environment variable
`linuxdeploy-plugin-appimage` turns into `appimagetool --runtime-file`. `curl` fails rather
than hangs, the fetch is skipped when the file is already there, and a second build needs no
network for this. With it set, the same build that had hung completed in about two minutes.

## What was actually built

`cargo xtask package --appimage --run` on the reference machine (openSUSE, WebKitGTK 2.52.3,
`cargo-tauri` 2.11.4) produces:

```
target/release/bundle/appimage/cide_0.1.0_amd64.AppImage    87,615,992 bytes (83.6 MiB)
```

Its squashfs payload carries `usr/bin/cide` and `usr/bin/cide-hook` side by side, which is what
`hook_settings` needs, plus 160 bundled shared libraries — GTK 3, WebKitGTK 4.1, JavaScriptCore,
libsoup 3, GStreamer, and the pixbuf and immodule loaders. Verified with
`unsquashfs -o 944632 -l`, not by running it.

A later build of the same version — `88,877,560` bytes, 2026-08-12, the copy installed at
`~/bin/cide_0.1.0_amd64.AppImage` — **has** since been run on the reference machine, launching,
opening projects and hosting Claude panes. That is what surfaced the environment bug the
section above is about, and it is worth recording how: not from a log or a test, but from a
Claude session inside a pane being unable to start one of its own MCP servers. The failure was
three processes below anything cide instruments, and the artefact itself looked entirely
healthy while it was happening. The clean-VM run in the milestone's acceptance criterion is
still outstanding, and the reference machine cannot stand in for it — it is the build host.

That run predates the move of `externalBin` out of the base config, so it was built without
`--config`. The artefact is unaffected: `tauri-cli` merges the patch into the same configuration
it would otherwise have read, and the merged value is identical. What has *not* been re-run
end-to-end since the move is `cargo xtask package --appimage --run` itself; the mechanism was
verified one layer down, by putting the same patch in `TAURI_CONFIG` — which is exactly what
`--config` sets — and watching `cargo check -p cide-app` demand the sidecar and then accept it.

`cide` links fifteen libraries. Eleven are inside the bundle: `libgtk-3.so.0`,
`libgdk-3.so.0`, `libgdk_pixbuf-2.0.so.0`, `libcairo.so.2`, `libglib-2.0.so.0`,
`libgobject-2.0.so.0`, `libgio-2.0.so.0`, `libdbus-1.so.3`, `libsoup-3.0.so.0`,
`libwebkit2gtk-4.1.so.0`, `libjavascriptcoregtk-4.1.so.0`. Four come from the host:
`libc.so.6`, `libm.so.6`, `libgcc_s.so.1`, `libz.so.1`. `cide-hook` needs only `libc` and
`libgcc_s`. libgit2 and OpenSSL appear in neither list because they are vendored.

## Consequences

- `cargo xtask package --check` belongs in CI next to `codegen --check` and `contract-check`.
- **The milestone's acceptance test — *a fresh AppImage on a clean VM with only WebKitGTK
  installed launches and runs Claude* — has still not been performed,** and there is now a
  specific reason to expect it to fail rather than merely an absence of evidence. The bundle
  contains `libwebkit2gtk-4.1.so.0` but **not** `WebKitWebProcess` or `WebKitNetworkProcess`,
  the two out-of-process helpers a web view cannot render without. `tauri-bundler` tries to
  bring them along, but it looks only under `<libdir>/webkit2gtk-4.1/` — its own source
  carries the comment `// TODO: Check if it's the same dir name on all systems` — and on
  openSUSE they are in `/usr/libexec/libwebkit2gtk-4_1-0/`. It found neither, copied nothing,
  and said nothing: the loop is `if source.exists()` with no else. Nothing sets
  `WEBKIT_EXEC_PATH` either (the generated `AppRun` sources one hook, the GTK plugin's), so the
  bundled library falls back to its compiled-in absolute path. That path exists on a host laid
  out like the build machine and does not on Debian or Ubuntu, where the helpers live in
  `/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/`. The expected symptom is a window that never
  paints. `cargo xtask package --appimage` now warns about exactly this and names where the
  helpers actually are. Fixing it needs either a build host whose layout matches the search
  list, or an `AppRun` that exports `WEBKIT_EXEC_PATH` — which `bundle.linux.appimage.files`
  cannot deliver, because only the `usr/` subtree of those files reaches the AppDir.
- **The environment scrub's rule is unit-tested; the bundled path it exists for is not.**
  `cide_core::child_env`'s tests pin the decisions against the literal strings a running
  AppImage produces, and the cure was confirmed at the shell level — the polluted environment
  kills `python3` with `Failed to import encodings module`, the environment the scrub yields
  does not. What no check covers is the whole chain: bundle → pane → `claude` → `uvx` → a
  server that answers. Only a bundled build can run that, so it belongs with the clean-VM
  acceptance test rather than in CI.
- Anyone enabling the updater needs to add `plugins.updater` with a public key and endpoints,
  and sign releases with `tauri signer`. Until then the warning in the preflight is accurate
  and should stay.
