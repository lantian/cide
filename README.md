# cide


An IDE whose centre of gravity is a live Claude Code session rather than a text buffer.

The distinguishing feature is a **pinned, non-closable Claude tab per project**, hosting a
tiling grid of panes — the project's primary Claude session, additional sessions, plain
shells, and read-only diffs — alongside the ordinary IDE furniture that serves it: a file
tree, a tabbed editor, an IDEA-style git commit tool window, `Ctrl+P`, and `Shift+Ctrl+P`.

Rust + Tauri 2. Linux-first (developed on KDE/Wayland), with the code kept portable — *kept*
portable, not *shown* to be: **Linux is the only platform this has ever run on.** See
[Platforms](#platforms-m16), which says what macOS would and would not do and what has actually
been checked.

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

## Platforms (M16)

**Linux is the only platform cide has ever run on.** Everything below the first paragraph is
written from source — the workspace's own and its dependencies' — and *none of it has been
observed*. That distinction is the whole point of this section, so it is made once, plainly:

> **macOS: not done.** The workspace has never been *run* or bundled on macOS, and it cannot be
> from a Linux machine. Linking to Darwin needs Apple's linker and the macOS SDK, whose licence
> restricts it to Apple hardware; `rustup target add aarch64-apple-darwin` installs a `std` and
> nothing that can link Cocoa, AppKit or WebKit. Tauri documents cross-compiling *Windows* from
> Linux and nothing else. So the next step is a Mac or a `macos-15` runner, and there is no
> version of this work that does not need one.
>
> **It has now been compiled once, on somebody's Mac, and it failed.** M16 said reading every
> `cfg` arm suggested `cargo build` would succeed there "with few or no changes". Two errors say
> otherwise, and both are now fixed — see *What the first Mac build actually reported* below.
> That paragraph is left in the history rather than quietly deleted, because the lesson is the
> section's whole subject: reading arms is not compiling them, and the estimate was wrong in the
> direction estimates are always wrong.
>
> What exists now: the bundle configuration, an **advisory** CI job that compiles, tests and
> lints the workspace on `macos-15`, `cfg` arms that keep the Linux-only pieces out of the graph
> and say what the platform does instead, and — new — a **type-check for `aarch64-apple-darwin`
> that runs on Linux**, which is what turned the round trip below into a local command. What is
> known to be broken or absent there is the table below.

Windows is not a target and nothing here has been written with it in mind; where a `cfg` arm
says "not Linux" rather than "macOS", that is honesty about the BSDs and Windows too, not
coverage.

### What compiles, and what nobody knows yet

`cide-app` is the only crate that links tauri and the only one that would fail to *compile*;
the crates with a platform problem that compiles anyway are `cide-core` (`child_env`) and
`cide-fs` (`trash`), and both are in the table below. Every Linux-only API in the workspace is
already behind a `cfg` with a counterpart that compiles: `gtk` is target-gated to Linux and four
BSDs in `crates/cide-app/Cargo.toml`, the GTK mouse-button handler has a non-GTK arm, the folder
picker falls back to `tauri_plugin_dialog` (which parents the dialog itself off Linux, which is
the only reason the GTK arm exists), the signal handlers are `cfg(unix)` and macOS is a unix,
and `git2`/tree-sitter build with `cc` everywhere. Reading every arm suggested `cargo build`
would succeed on a Mac with few or no changes.

**That estimate was wrong, and this is the correction.** The `macos (advisory)` job in
`.github/workflows/ci.yml` exists to convert reading into knowing; it is `continue-on-error:
true` until it has been green once, because a job that has never passed cannot tell a regression
from a first attempt. The comment on that job says what to delete when it does.

### What the first Mac build actually reported

Two diagnostics, on the same file, both in `cide-app` as predicted — and neither one predictable
by reading a `cfg` arm in *this* workspace, because both are about a **dependency's** `cfg`:

| what | why | fix |
| --- | --- | --- |
| `warning: unused import: crate::emit` — `windows.rs:17` | `emit`'s only use is `emit::mouse_nav` inside `install_mouse_nav`, which is gated to Linux and the four BSDs. Off those targets the import is unused. A *warning* — but the macOS job's fourth step is `clippy --all-targets -- -D warnings`, where it is an error | gated with the identical five-target `cfg(any(…))`, matching its consumer exactly rather than approximately |
| `error[E0599]: no method named transparent` — `windows.rs:218` | tauri declares it `#[cfg(any(not(target_os = "macos"), feature = "macos-private-api"))]` (`tauri-2.11.5` `src/webview/webview_window.rs:1075`). The method does not exist on a Mac | the builder chain is split and the call omitted there. **Not** by enabling `macos-private-api`: that turns on private Apple SPI, is grounds for App Store rejection, additionally requires `macOSPrivateApi` in `tauri.conf.json`, and we pass `false` — so it would buy nothing at all |

`transparent(false)` only asserts what `WindowConfig::default()` already sets
(`tauri-utils-2.9.2` `src/config.rs:2320`), so deleting it outright would have been
behaviour-identical. It is kept on the platforms that have it because the comment above it
records *why* the window is opaque, and a comment attached to a live line survives a change of
tauri's default loudly instead of silently.

### And what the first Mac *bundle* reported: the filesystem, not the code

Two failures, in order, neither of them visible from Linux and neither in Rust.

**One: no `pnpm`.** `cargo tauri build` runs `beforeBuildCommand` before it compiles anything, so
the run ended in `sh: pnpm: command not found` — after the release build of `cide-hook`. That is
now a preflight verdict; see *Packaging* below and *What the Mac itself needs* above.

**Two: three pairs of file names that differ only in case.** `chrome/CloseConfirm.tsx` (the
dialog) sat beside `chrome/closeConfirm.ts` (its rules), and the same for `PasteConfirm` and
`GoToLine`. That is this codebase's documented split — the component in PascalCase, the testable
half in camelCase — and on Linux it is three ordinary pairs. APFS is **case-insensitive** by
default, and TypeScript resolves a bare specifier `.ts` before `.tsx`, so on a Mac
`import { CloseConfirm } from '@/chrome/CloseConfirm'` asked for `CloseConfirm.ts`, got
`closeConfirm.ts`, and reported the component missing from its own module. Seven errors in three
files, every one blaming an import that is correct.

The rules modules are now `closeConfirmModel.ts`, `pasteConfirmModel.ts` and `gotoLineModel.ts`,
following `branchModel.ts` and `menuModel.ts`, which have the same shape and never collided
because they were never a component's name. **`pnpm --dir ui run check:casing` is the gate**, and
it is the interesting part: the collision cannot be observed on the platform this is developed
on, so the check does not look for it — it walks the tree and refuses any directory holding two
names that differ only in case (whole names, anywhere in the repository, because such a pair
cannot be checked out on a Mac at all; and module *stems* under the TypeScript extensions,
because that pair resolves to the wrong file). It carries a positive control over the exact names
that broke, since every other assertion in it is a "no such pair exists" that would go green for
ever if the detector stopped detecting.

### Type-checking for macOS from Linux — partly, and not where it would have helped

`cargo check` never links, so Apple's linker and SDK are irrelevant to it. What stops a
`--target aarch64-apple-darwin` check on Linux is a handful of *dependency build scripts*
compiling C with flags a GNU `cc` rejects. `scripts/darwin-cc.sh` strips those flags and compiles
for the host instead — the objects are wrong for Darwin and nothing a check produces is linked, so
that does not matter — and `DOCS_RS=1` is `objc2-exception-helper`'s own early return:

```sh
rustup target add aarch64-apple-darwin
DOCS_RS=1 CC_aarch64_apple_darwin=$PWD/scripts/darwin-cc.sh \
         CXX_aarch64_apple_darwin=$PWD/scripts/darwin-cc.sh \
  cargo check --locked --workspace --all-targets --target aarch64-apple-darwin \
    --exclude cide-app --exclude cide-git --exclude xtask
```

**Note the three exclusions, because they are the whole caveat.** `git2` is configured
`vendored-openssl`, so anything depending on it builds OpenSSL from source *for Darwin*, and that
needs a real cross toolchain rather than a flag-stripping shim — it fails in OpenSSL's own `make`.
That rules out `cide-git`, `xtask` and **`cide-app`**.

So this covers nine of the twelve crates, including `cide-core`, where `child_env`'s
`PR_SET_PDEATHSIG` arms live — the macOS `cfg` code that matters most. It does **not** cover the
one crate that links tauri, and both errors in the table above were in `cide-app`. This would not
have caught either of them. It is worth running before touching a `cfg` arm in a domain crate, and
it is not a substitute for the `macos` CI job.


### What the Mac itself needs, and what it will spend

* **Xcode Command Line Tools**, and the failure without them does not look like a missing
  toolchain. Eight build scripts in the macOS graph invoke a C or Objective-C compiler —
  `blake3`, `libgit2-sys`, `libz-sys` (probe only), `openssl-sys`/`openssl-src`,
  `objc2-exception-helper`, `tree-sitter` and both grammars — and the first one dies with
  `xcrun: error: invalid active developer path`, which reads like a Rust problem. `xcode-select
  --install` is the fix. Full Xcode is not required; CLT supplies clang, the SDK, `make` and
  `libiconv`. `perl` (`/usr/bin/perl`) is present already and `openssl-src` needs it.
* **No pkg-config, GTK, dbus or X11.** `tao`, `wry`, `muda`, `tray-icon` and `rfd` declare all of
  those under Linux/BSD-only sections, so tauri's default `x11` and `dbus` features — which this
  workspace does inherit — are inert there. The Linux-only half of the graph is about a hundred
  crates that simply do not build.
* **`--locked` is safe.** `Cargo.lock` already carries the Darwin crates (`objc2`,
  `objc2-app-kit`, `embed_plist`, `window-vibrancy`, `plist`, `fsevent-sys`, `core-graphics`), so
  nothing re-resolves. `swift-rs` appears in the lockfile and in tauri's dependency list but is
  gated `cfg(all(target_vendor = "apple", not(target_os = "macos")))` — iOS only. No Swift
  toolchain is needed.
* **Node and `pnpm`, and this one is not a Rust prerequisite at all — it is the first thing a
  bundle needs.** `cargo tauri build` runs `build.beforeBuildCommand` (`pnpm build` in `ui/`)
  through `sh -c` before it compiles a line of Rust, so a Mac with no pnpm fails packaging with
  `sh: pnpm: command not found` and `beforeBuildCommand failed with exit code 127`. **Observed on
  the first Mac to run `./build.sh`** — and it arrived *after* the release build of `cide-hook`,
  which is step one of the plan. `corepack enable pnpm` (corepack ships with Node), `brew install
  pnpm` or `npm install -g pnpm`, then `pnpm --dir ui install` once. Vite 8.2.1 requires Node
  `^20.19.0 || >=22.12.0`; CI uses 22. Both the tool and `ui/node_modules` are now preflight
  checks in `cargo xtask package`, so a machine missing either is told in the first second and in
  one pass rather than over two long failures.
* **Budget 15–25 minutes for the first `cargo build`**, and note that
  `[profile.dev.package."*"] opt-level = 2` means even a debug build optimises all ~357
  dependencies. Three C builds dominate: libgit2, `tree-sitter-rust`'s `parser.c` as a single
  6.3 MB translation unit — and **OpenSSL, which is 1210 C files that nothing ever links.**
  `git2` is configured `vendored-openssl` without `https`; `libgit2-sys` declares
  `https = ["openssl-sys"]` and gates every use of it behind that feature (`build.rs:256,280,289`),
  so `libssl.a` is built and discarded. It is wasted on Linux too — this is a macOS-*visible*
  cost, not a macOS defect — and dropping the feature is a dependency change that wants its own
  look, not a line in a platform sweep.

### And then `cargo test`, which was the next thing to go red

The macOS job runs fmt → build → test → clippy, and `cargo test` carries no `--no-fail-fast`, so
it stops at the first failing binary. Compiling is therefore not the end of the round trip; it is
the start of it. The `cfg`-driven *values* can be simulated on Linux even though the target
cannot — `platform_defaults()` forced to the macOS layer, `cwd_of_pid` compiled through its
non-Linux arm, `LADDER_APPLIES` and `PARENT_DEATH_IS_ENFORCED` forced `false` — and running the
suite that way found **17 failures**, none of them visible to the compiler. All 17 are fixed; the
last two constants cost nothing, which is itself worth knowing.

**Sixteen were in `cide-core::keymap`, and every one was a test bug rather than a product bug.**
They drove `resolve`/`apply_edit` and asserted a literal `ctrl+…` spelling, so on macOS — where
the platform layer moves every `ctrl` default onto `meta` — they failed against an editor that
was behaving exactly as designed. The repair is not to gate them off macOS, which would delete
coverage on the platform that has least and leave the surviving assertions passing *vacuously*.
It is to **name the layer under test**: `apply_edit` grew an `apply_edit_layered` seam, the exact
counterpart of the `resolve_layers` seam that already existed for this reason, and those sixteen
now pin the PC layer explicitly and assert one platform's rule identically on every host.

One of the sixteen was not a spelling and deserved a decision.
`every_default_binding_can_be_given_back_to_whatever_is_underneath` asserts *exactly one line per
unbind, so a user's file stays readable* — which is a property of `defaults()`, where no command
carries two chords, and **not** of the editor.
The macOS layer *adds* ⌘[ / ⌘] beside `mouseback`/`mouseforward` rather
than rewriting them, so `navigate.back` stands on two chords there and an unbind correctly writes
two removals. That is now stated by its own test instead of being an accident of the host.

**The seventeenth** was `cmd::session`'s `/proc` test. Its four assertions were two that need
`/proc` and two containment refusals — and off Linux the refusals passed *for the wrong reason*,
because `cwd_of_pid` answers `None` there and every input was refused before the rule was ever
consulted. `contained_cwd` is now split: `contain` takes the cwd as an argument, so the
security-relevant half is driven on every host, which is what `cwd_of_pid`'s comment already
claimed and the code did not deliver. The `/proc` half is Linux-gated, and the non-Linux arm gets
a **tripwire** — it asserts `cwd_of_pid` is still `None`, so the day somebody implements it with
`proc_pidinfo` the test fails and sends them to turn the real assertions back on, instead of the
new code shipping untested exactly where it is new.

### Where a Linux guarantee has no macOS equivalent

| what | on macOS | where it is written down |
| --- | --- | --- |
| **`PR_SET_PDEATHSIG`** — cide's children die with it (ADR 0008) | **No equivalent.** A `SIGKILL`, OOM kill or crash leaves every `claude` and language server running. A clean quit is unaffected. | `cide_core::child_env::set_parent_death_signal`, non-Linux arm |
| the fork race inside that guarantee | **Still closed.** `getppid()`/`raise()` are POSIX, one shared body, one test on both platforms | same |
| **`/proc/<pid>/cwd`** — a shell pane's real working directory | **Always `None`.** Relative paths in terminal output stop resolving; absolute ones still work, and nothing errors | `cmd::session::cwd_of_pid`, non-Linux arm |
| **the eight resize grips** | **Not painted.** `tao`'s `drag_resize_window` is `NotSupported` on macOS and the grip's `preventDefault()` would swallow the gesture AppKit handles itself | `ui/src/chrome/windowControls.ts::drawsOwnResizeGrips` |
| **the graphics ladder** (ADR 0006) | **Applies nothing**, correctly — every rung is a WebKitGTK variable and macOS runs WKWebView | `cide_app::graphics::LADDER_APPLIES` |
| **⌘Q** | Reaches `lifecycle::shutdown` through `RunEvent::Exit`, which is now handled | `cide_app::run`, and `both_ways_out_of_the_run_loop_reach_shutdown` |
| **⌘W, ⌥⌘H** | **Dead.** macOS's default menu bar answers the accelerator before WKWebView is asked | `cide_core::keymap::MACOS_MENU_CHORDS` |
| the freedesktop trash spec | Writes to `~/.local/share/Trash`, which Finder cannot see or restore from | `cide_fs::trash` — **unchanged, and wrong there** |
| XDG state and config paths | Work, but are not the platform convention | `cide_core::persist` — a deliberate choice, see below |

**`PDEATHSIG` is the one that matters**, and dropping it silently on a platform is exactly how
an orphaned `claude` survives a crash — so it is not dropped silently. The non-Linux arm is
written out, shares one body with the Linux arm for the half that *is* portable, logs a
`warn` once per process, and `PARENT_DEATH_IS_ENFORCED` is a public `false` there so no caller
can read the arm as equivalent. The two real replacements are named in that arm's docs and
**neither is implemented**: a supervising helper process using `kqueue`'s `NOTE_EXIT` (which
`cide-hook`, already a second binary in the bundle, is the natural home for), and — cheaper,
weaker, and the one worth doing first — a startup sweep that kills children whose recorded cide
pid is dead. The sweep needs a pid registry that does not exist yet; `cide_claude::orphans`
sweeps *files*, not processes.

**Found on the way, and it is a Linux defect, not a macOS one.** `set_parent_death_signal`'s
doc comment said its caller "that matters" was `cide-pty`. `cide-pty` does not depend on
`cide-core` at all and has never called it: **no PTY pane — no `claude`, no shell — is armed on
any platform.** Two things stand in the way, and only the first was the one the comment named:
`portable-pty`'s `CommandBuilder` exposes no `pre_exec` hook, and `PtySession::spawn` is called
from a Tauri command worker, which fact 3 of that module makes the *wrong* thread to fork from —
arming there would kill panes seconds after they opened. What covers a PTY child today is the
shutdown ladder, the signal thread and `run.sh`'s reap of a previous run's orphans. The comment
now says all of that instead of naming a call that does not exist.

### What needs a Mac at the keyboard

Everything here is a decision, not a port, and each one has a consequence somebody has to look
at before choosing:

* **The title bar.** The window is built `decorations(false)` (ADR 0006). The macOS idiom is
  `TitleBarStyle::Overlay` + `hidden_title(true)`, which restores native edge resize, rounded
  corners, the window shadow and real traffic lights — at which point `windowControls.ts`'s
  `MAC` layout becomes dead code and the header has to reserve ~78px of leading inset instead.
  That is a design decision. Until it is made, the header draws its own buttons on the left and
  the OS resizes the edges.
* **The menu bar.** cide calls neither `.menu()` nor `.enable_macos_default_menu(false)`, so
  tauri installs `Menu::default`, whose accelerators AppKit resolves ahead of the web view.
  `keymap::MACOS_MENU_CHORDS` lists all twelve and `macos_menu_conflicts()` computes which
  bindings they kill; a test pins the answer in both directions, so a new dead chord fails the
  build rather than shipping, and `./target/debug/cide-headless keymap` prints the list under
  every dump **on every host**, because a chord that cannot fire on a Mac is invisible from the
  platform this is developed on. Today it is two: **⌘W** closes the window instead of the tab, and
  **⌥⌘H** hides other applications instead of moving a pane left. Turning the default menu off
  fixes both and takes the Edit submenu with it — and on macOS those items are a large part of
  how ⌘C/⌘V reach a text view at all, which is precisely the thing that cannot be checked from
  here. cide also has no `app.quit` command, so ⌘Q *is* that menu item: the quit path on a Mac
  is a gesture cide does not own, which is why the run loop now handles `RunEvent::Exit`.
* **Option in a terminal pane.** `ui/src/terminal/xterm.ts` constructs `new Terminal({…})`
  without `macOptionIsMeta`, which defaults false, so Option composes a dead-key character
  instead of sending `ESC`-prefixed bytes. That takes out `⌥F7`, `⌥↑`/`⌥↓`, the `⌥⌘h/j/k/l`
  family and every readline Meta binding inside a pane. A one-line fix that should be made by
  somebody who can press the key.
* **The cwd probe.** `proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, …)` is about twenty lines of
  `libc`. Whether it answers for a same-user child under the hardened runtime is the part that
  has to be observed rather than read.
* **Ctrl+click is the system context-menu gesture there, and cide claims it four times.** AppKit
  turns Control-click into a secondary click, and WebKit dispatches *both* a `mousedown` with
  `ctrlKey: true` and a `contextmenu`. `ui/src/menus/native.ts` installs a capture listener that
  `preventDefault()`s the webview's own menu but does not stop propagation, so cide's bubble-phase
  `onContextMenu` still opens cide's. Meanwhile every mouse gesture is spelled
  `ctrlKey || metaKey`: `terminal/clickGate.ts`'s `pressVerdict` (open a path link),
  `sidebar/FileTree.tsx` and `sidebar/GitPanel/ChangesTree.tsx` (toggle-select a row), and
  `editor/codeIntelGate.ts` (jump to definition). Each fires its action **and** opens a menu over
  the result. The correct macOS behaviour is ⌘-click only. The fix is a platform predicate of the
  shape `windowControls.ts::isMacUserAgent` already has — but it is roughly seven mouse-gesture
  call sites with different call paths, and getting it wrong breaks Ctrl+click file opening on the
  platform that works, so it wants a check script and a Mac rather than a sweep. Note
  `clickGate.ts` reasons explicitly that it accepts `metaKey` "because macOS spells the same
  gesture with Cmd" — it added the Mac spelling without removing the one macOS had already
  spoken for.
* **Three default bindings sit on F-keys a stock Mac keyboard does not send.** `f4`
  (`sidebar.toggle`), `ctrl+f12` → `meta+f12` (`structure.file`) and `alt+f7`
  (`navigate.usages`). On every Mac laptop and Magic Keyboard the F-row defaults to
  media/system keys — F4 is Spotlight, F7 previous-track, F12 volume-up — so all three need Fn
  held unless the user has flipped *Use F1, F2, etc. keys as standard function keys*.
  `MACOS_MENU_CHORDS` cannot see this: it is scoped to AppKit's menu accelerators, and this is a
  hardware-layer interception. It is deliberately **not** encoded as a computed list beside
  `macos_menu_conflicts()`, because unlike that one the rule is not crisp — it depends on the
  keyboard and on a system setting — and a computed list would overstate what is known.
* **The "come back here" signal is one Dock bounce, and clearing it does nothing.**
  `windows.rs::demand_attention` is built on the hint being a *standing* state that returning
  lowers and leaving re-raises. macOS has no such state: `tao-0.35.3`'s
  `request_user_attention` maps `Informational` to `NSApp requestUserAttention`, which bounces
  the icon once and stops, and the unset path is `if let Some(ty)` — with `None` it calls
  **nothing at all**, never `cancelUserAttentionRequest`. So a user who walks away from a turn
  awaiting permission gets one bounce, spent, never re-raised. `Critical` — which bounces until
  the app is activated — is the arm that would say the right thing there, and macOS is the one
  platform where that choice is the whole difference between a signal and none.
* **The shell pane opens `/bin/bash -l`, hardcoded.** `ui/src/panes/TerminalPane.tsx`'s
  `DEFAULT_SHELL` ignores `$SHELL` and `getpwuid`. On macOS `/bin/bash` is 3.2.57 (2007, the last
  GPLv2 release Apple shipped) and the user's login shell since Catalina is `/bin/zsh`, so
  `bash -l` reads `/etc/profile` and `~/.bash_profile` and the user's entire `~/.zshrc` never
  runs — including the `eval "$(/opt/homebrew/bin/brew shellenv)"` that is how Homebrew tells
  people to get on `PATH`, which compounds the `claude`-not-found entry above. The comment there
  says Settings → Terminal would take it over in M11; at M16 it has not, and
  `cide-headless/src/main.rs` reads `$SHELL` correctly, so the headless binary is more right than
  the app.
* **Move to trash.** `NSFileManager trashItemAtURL:` or the `trash` crate. `cide_fs::trash`
  rejected that crate *for Linux*, on reasoning that does not carry to macOS. It is wrong in
  **two** places, not one: the home case writes `~/.local/share/Trash` (already in the table
  above), and a file deleted from an external volume goes to `$topdir/.Trash-<uid>` per the
  freedesktop spec where macOS's own convention is `$topdir/.Trashes/<uid>`. Finder's *Put Back*
  can see neither.
* **Settings → Appearance** offers three graphics switches that persist and do nothing there.
  `graphics::LADDER_APPLIES` is the seam to gate them on; the screen wants eyes before it grows
  a fourth state.
* **`run.sh` does not run on a stock Mac.** It uses `mapfile` (bash 4+; macOS ships 3.2 as
  `/bin/bash`) and `ps -eo ppid=,pid=,comm=`, whose `comm` prints a full path there.
* **Case-insensitive filesystems.** APFS folds case by default and `canonicalize` does not
  normalise it, so `cide_fs::ops::check_within` — textual by design — would refuse a terminal
  link naming `/Users/x/proj/…` for a project opened as `/Users/x/Proj`. Unquantified.
* **FSEvents.** `notify` uses it there, not inotify, and the debouncer timings were tuned
  against inotify. ~~Expected to work; unverified.~~ **Expected to be dead under any symlinked
  path** — see the entry below, which is the one macOS finding that is a defect in the product
  rather than in a test.

### The file watcher, and why it is expected to be silent under `/tmp` and `/var`

This one is not a missing API or an untested arm. It is a real defect, reasoned from dependency
source, and it is written here rather than fixed because the fix is not the one line it looks
like.

`cide_fs::filter::Filter::admits` is the only gate on an incoming watch event (`watch.rs`), and
it answers `false` for any path whose `root_of` is `None`. `root_of` is a textual `starts_with`
against `WatchConfig.roots`, and `cide_app::files` fills those from the workspace
**uncanonicalised**. FSEvents, meanwhile, reports paths in the volume's canonical namespace.
`notify`'s own backend corroborates that: `notify-8.2.0/src/fsevent.rs:376,392` canonicalises the
watch path before storing it, and the callback builds each event path verbatim from the C string
and matches with `starts_with` against that canonical copy — a `canonicalize` that only makes
sense because the events arrive canonical.

So a project opened at `/tmp/…` or `/var/…`, or through any symlink, gets a watcher that binds
successfully, reports `WatchBackend::Native`, and then admits nothing at all. `/Users/…` is a
firmlink rather than a symlink, so an ordinary `~/project` is unaffected — which is exactly what
would let this ship. **`cide-fs`'s watcher tests build their fixtures in `std::env::temp_dir()`,
which on macOS is `$TMPDIR` under `/var/folders/…`**, so the macOS CI job should show five of
them failing; `polling_mode_still_reports_changes_and_says_why` should pass, because
`PollWatcher` stats the paths it was handed, and
`writes_under_an_ignored_directory_are_never_reported` should pass *vacuously*, which is the
tell.

**Why it is not fixed here.** Canonicalising the roots is one line, but the roots are shared
identity: `Index` is built from the same list, `Filter` computes `rel` by `strip_prefix(root)`,
and `per_dir`/`excludes` are keyed on the walked directories. Canonicalising only the filter's
copy admits the events and then silently skips every gitignore rule — noise instead of silence,
which is worse. Canonicalising at the source moves path identity for the index, the UI and
`check_within` together, on the platform where all of it currently works and none of it can be
observed failing. It wants a Mac and one change, not a Linux guess in two halves.

### Finding `claude` from a Finder-launched `.app`

Three problems that compound, none of which has a compile or test signal, and together they are
the likeliest way a Mac user concludes cide is broken. The third was reported from a Mac in M17
and is fixed; the first two are still open, and the fix for the third is what makes fixing them
safe. The heading is now narrower than the section — this is about every binary cide runs, not
only `claude`.

**`search_paths()` names the wrong directories for the platform.** It is `PATH` plus
`~/.cargo/bin` and `~/go/bin` (`cide_core::toolchain`), and its own comment names precisely the
failure it is guarding — *added by a shell rc file, so a cide started from a terminal sees them
and the same cide started from a desktop launcher does not* — while fixing only the Linux
instance of it. An app launched from Finder, the Dock or Spotlight inherits **launchd's**
environment, not a shell's: `PATH=/usr/bin:/bin:/usr/sbin:/sbin`. `/etc/paths` and `path_helper`
are a shell mechanism and do not run. So none of `~/.local/bin` (where Claude Code's own native
installer puts `claude`), `/opt/homebrew/bin`, `/usr/local/bin` or `/usr/local/go/bin` is on it.
The pinned Claude tab then reports *claude: not found on PATH* on a machine where the user runs
`claude` in Terminal every day. `rust-analyzer` and `gopls` survive only because `~/.cargo/bin`
and `~/go/bin` happen to be where `rustup` and `go install` put them.

**And the preflight and the spawn do not agree about where to look.** `claude_cli::resolve`
validates against `search_paths()` — PATH *plus* those two extras — but the spawn deliberately
passes the binary through as stored, so a bare `claude` reaches `execvp`, which searches **`PATH`
only**. A `claude` in `~/.cargo/bin` and not on `PATH` therefore passes cide's check and then
fails at exec with no cide-authored explanation. That is latent on Linux today and materially
more likely on macOS, where the two path sets diverge much further.

**And there is a third, one level deeper than either, which is the one a user actually reported.**
The two above are about *finding* a binary. This one is about the binary cide **did** find:

> *"goto in golang project not working on macos (but works on linux), it just prints: gopls no
> views, rust-analyzer also not working, it writes: the language server stopped"*

Discovery worked. `search_paths()` has `~/go/bin` on it, `which("gopls")` returned a path, and
`cide_lsp::discover`'s *not on PATH, install it with…* sentence was never produced — which is why
the report names two runtime failures rather than a missing server. cide then spawned `gopls` and
handed it **cide's own environment, `PATH` included**, because `child_env`'s bundle scrub only
ever *removes* variables; nothing in the workspace set `PATH` on a child. For a Finder-launched
`.app` that `PATH` is launchd's four directories, so the language server cide had successfully
started could not exec `go`. A `gopls` with no usable `go` builds no workspace view and answers
the JSON-RPC error `no views` to every request over that session; a `~/.cargo/bin/rust-analyzer`
installed by rustup is a *proxy* that re-execs through `rustup`, and with no toolchain reachable
it exits. Both strings the user quoted are cide's own, from
`crates/cide-app/src/lsp.rs`'s `format!("{}: {error}", server.binary())` over
`cide_lsp::RequestError::Failed` (which carries the server's own message verbatim) and
`::ServerGone` (whose `Display` is literally *the language server stopped*).

None of this is macOS-only — it is guaranteed there. A Linux cide started from a `.desktop`
launcher or an AppImage hands its children the same too-short `PATH`, for the same reason
`search_paths()`'s own comment already names.

**The fix, taken in M17, is one rule: the directories cide searches to *find* a binary and the
directories it gives that binary's process to search are the same directories.**
`cide_core::toolchain::extra_dirs()` is that one list. `search_paths()` reads it (so `which`
searches it) and `child_path_from()` **appends** it to whatever `PATH` a child would otherwise
inherit; `child_env::prepare_command` — renamed from `scrub_command`, because a function called
*scrub* that adds a variable is a comment waiting to go stale — applies both passes at the single
chokepoint every `std::process` spawn already goes through, and `cmd::session::base_env` does the
same for the PTY lane. On macOS `extra_dirs()` also parses `/etc/paths` and `/etc/paths.d/*`
itself, since that is the mechanism `path_helper(8)` implements and the Go pkg installer writes
`/etc/paths.d/go`; Homebrew deliberately does *not* write there, so `/opt/homebrew/bin` and
friends are a hardcoded floor under the parsed list.

Two deliberate choices, both of which can be got wrong later:

* **Append, never prepend.** Putting `/opt/homebrew/bin` ahead of `/usr/bin` changes which `git`,
  `python3` and `openssl` every child of cide resolves, on a machine where the user's own shell
  may order them the other way. The reported failure is a directory being *absent*, not shadowed.
* **No existence filter**, for the same reason `search_paths()` has none: filtering would let the
  two lists diverge again, and the test `the_path_a_child_searches_is_the_path_which_searched`
  in `cide-core::toolchain` is what keeps them from doing so.

That test is also what makes the *first* problem above safe to fix at all. Widening
`search_paths()` widens `claude_cli::resolve`'s acceptance, and the paragraph this replaces was
right that doing it alone converts a clear refusal into an opaque `ENOENT`. **The exec gap is
closed by the same change rather than by a second one:** the bare name is kept in every case — so
the CLI's self-update property is never lost — because the child's `PATH` now *contains* the
directories `search_paths()` searched, and portable-pty resolves a bare program against the
builder's `PATH` (`cmdbuilder.rs`) exactly as `std::process` swaps `environ` before `execvp`. Two
lists that had to agree became one list.

**What is verified, on Linux.** The pure path building has unit tests, including the one that
holds the two lists together. The macOS list itself is built and asserted from here —
`extra_dirs_in()` takes its `HOME` and its `/etc` as arguments precisely so that the half of this
feature no machine in CI can run is still exercised
(`the_macos_list_is_path_helper_then_the_hardcoded_floor_with_no_repeats`): the order of the four
sources, and that a directory arriving from both `/etc/paths` and the hardcoded floor is listed
once. That the names in that floor are *the right names* is not something a Linux test can say.
The link that had only been read out of `std`'s source — that a
`PATH` set on a `Command` is the `PATH` a **bare** program name resolves against — is now
executed by `a_path_set_on_a_command_is_the_path_a_bare_program_name_is_resolved_on`, which
spawns a bare name that fails on launchd's four directories and succeeds once the directory
holding it is appended. The portable-pty half of the same claim (`cmdbuilder.rs` resolves a bare
program against the builder's `PATH`) is still source-read only.

**What is not verified.** No Mac was in front of any of this. That launchd hands a Finder-launched
`.app` those four directories is taken from Apple's documentation, not observed; the hardcoded
macOS directory names and the `/etc/paths.d` layout are reasoned from Homebrew's, MacPorts' and
Go's installers, not observed; and that `gopls` answers exactly `no views` when it cannot exec
`go` is inferred from its view model plus the user's verbatim report. Getting a name wrong is
cheap (append-only, one failed `stat`); getting the list *short* is the real risk. **Why
rust-analyzer stopped on that machine is still unknown** — `ServerGone` is reported both for
"never started" and for "died", so the string does not discriminate, and the sentence that would
say is the one `cide_lsp::server::start_failure_reason` writes into the log. The fix is necessary
for it (the rustup proxy shells out) and cannot be called sufficient from here.

Two gaps this leaves standing, recorded rather than quietly fixed:

* The shell pane still opens `/bin/bash -l` (see the Platforms list above), which never reads
  `~/.zshrc`. A user whose `PATH` exists only inside `eval "$(brew shellenv)"` now gets those
  directories in cide's *children* and still not in their own shell pane.
* `search_paths()` reads the **unscrubbed** process `PATH` while a child gets the scrubbed one, so
  under an AppImage `which()` can in principle see a binary in `$APPDIR/usr/bin` that no child
  can. Harmless today because that directory holds only cide's own binaries, and left alone
  because closing it means teaching discovery about the bundle.

The option not taken was probing the user's login shell for its `PATH`, as VS Code does. It is
strictly more accurate — it is the only way to learn a `PATH` that exists solely inside a
`~/.zshrc` — and it loses on two counts that are not close: `cide_core::toolchain` opens by
stating that nothing in it spawns a process, and `$SHELL -ilc 'echo $PATH'` runs the user's
interactive rc, which can block on a prompt, an ssh-agent unlock or a slow network mount. VS Code
carries a timeout, a cancel path and a user-facing *resolving shell environment failed* dialog
because that hangs in the field. It is the principled next step if a report names a directory the
static list cannot reach, and it should arrive with a timeout and a log line rather than silently.

### Packaging, and the half that money buys

`cargo xtask package` grew `--app` and `--dmg`, a `crates/cide-app/tauri.macos.conf.json`
overlay, and a **host check**: naming a target this machine cannot build is a preflight failure
with the reason in it, in the first second, rather than a plan that dies inside `codesign`
twenty minutes later. Naming no target means everything the host can build. The `cide-hook`
sidecar rides along unchanged — `externalBin` copies into `Contents/MacOS/`, which is exactly
where `current_exe().parent()` looks — and the overlay must never carry that key, because
`tauri-build` reads the *host's* platform overlay on every `cargo build` and it would break the
workspace build on macOS and nowhere else. A test asserts it does not.

**A multilib host can break the AppImage with a message that names nothing.** On a distribution
that ships 32-bit GTK beside 64-bit — openSUSE's `gtk3-tools-32bit` is the case this was found on
— the *32-bit* package owns the unsuffixed `/usr/bin/gtk-query-immodules-3.0` and the 64-bit tool
is renamed `…-3.0-64`. `linuxdeploy-plugin-gtk`'s `search_tool` tries `command -v` **first** and
returns on the first hit, so it takes the 32-bit binary and never reaches the `/usr/bin/$tool-64`
entry already in its own fallback list. That binary then reads the AppDir's 64-bit immodules,
fails every load with `wrong ELF class: ELFCLASS64`, and exits 1; the plugin is `set -e`, so
linuxdeploy reports `Failed to run plugin: gtk`, and tauri-bundler discards linuxdeploy's stderr
at its default log level. What reaches the user is `failed to bundle project: failed to run
linuxdeploy` — four layers above the fact, naming none of it.

None of that is fixable in cide's source, because the decision is made inside a downloaded shell
script. What is available is `PATH`, which that `command -v` honours, so `xtask package` puts one
symlink named `gtk-query-immodules-3.0` in `target/appimage-gtk-shim`, points it at the 64-bit
tool, and prepends that directory for the bundler step only. `gtk_immodules_shim` installs it
**only** when the unsuffixed tool exists and is genuinely the wrong ELF class: putting a directory
on the front of `PATH` for every build would be a durable hazard bought for nothing. The plan
prints the step like any other, so a host in this state says so before it builds.

**The first Mac packaging run got as far as the frontend and stopped there**, and the fault was
the preflight's rather than the Mac's: `./build.sh` preflighted fourteen things, built
`cide-hook` in release, and only then handed over to `cargo tauri build`, whose *first* action is
`beforeBuildCommand` — `sh -c 'pnpm build'`, on a machine with no pnpm. Exit code 127. Every fact
needed to predict it was on hand before anything compiled: the config names the command and
`PATH` says whether it exists. So the preflight now reads `build.beforeBuildCommand` out of
`tauri.conf.json`, checks that program is on `PATH` and that the frontend's `node_modules` is
installed, and reports both in one pass — the tool it names is whatever the config says, so
renaming the command moves the check with it. See *What the Mac itself needs* above for the
install lines.

What no amount of configuration can do:

* **Notarisation needs a paid Apple Developer Program membership** ($99/yr) for a Developer ID
  Application certificate. A free Apple ID yields a local-run-only certificate that cannot be
  notarised.
* **Without notarisation a downloaded `.dmg` is quarantined** and Gatekeeper refuses it as
  *"damaged and can't be opened"* — which blames the download, not the signature — until the
  user runs `xattr -dr com.apple.quarantine`. The preflight says so in as many words.
* **The updater is a third artefact.** `tauri-plugin-updater`'s macOS channel is an
  `.app.tar.gz`, not the `.dmg`; and no `plugins.updater` is configured on any platform yet.

An unsigned or ad-hoc-signed `.app` and `.dmg` that run locally are entirely buildable, and
that is the honest first target.

**The icon set is left one raster short, knowingly.** The overlay carries its own `bundle.icon`
list because the platform merge *replaces* the array rather than extending it, and the macOS
list drops `48x48.png` — not an ICNS size, so `tauri-bundler` Lanczos-resizes it to 32 and then
discards it because `32x32.png` is already in the family. What is still missing is a 1024px
`icon@2x.png` for the ICNS 512@2x slot, which leaves a Retina icon upscaled from 512. It is one
line in `scripts/gen-icons.sh`; it is not there because an `@2x` name in the *Linux* list would
be installed by `tauri-bundler`'s freedesktop path into `hicolor/512x512@2/apps/`, a directory
no icon theme searches — the exact trap that script's comments record removing `128x128@2x.png`
for. The overlay is the right place to put it and nobody has.

### `--src`: the source tarball, and what its checksum is worth

`cargo xtask package --src --run` writes
`target/release/bundle/src/cide-0.1.0-src.tar.gz` and prints its sha256. It is the artefact a
GitHub release page attaches beside the AppImage, and its point is that the number under it is
**reproducible from the tag**: anyone can check out `v0.1.0`, run the same command, and get the
same bytes. That is why it is `git archive` and not `tar`.

`git archive` is what makes the claim checkable rather than merely plausible. The contents are
exactly the tracked set at `HEAD`, so `.gitignore` decides what ships and there is one rule
instead of two that drift — measured here, 889 tracked files and 889 files in the tarball, the
two lists identical. Every entry is normalised to `root/root`, mode 0644/0755 and the commit's
date, so a fresh clone produces the same tar. And the stream opens with a `pax_global_header`
carrying `comment=<sha>`, which `git get-tar-commit-id` reads back — the tarball says which
commit it is without a synthesised file to say so.

Measured on the reference machine (git 2.54.0): two runs two seconds apart are **byte-identical**,
`1f 8b 08 00 00 00 00 00` — git's built-in gzip writes `MTIME 0`. That is not luck, it is the
reason the plan is `--format=tar.gz` in one step: writing `foo.tar` and then running `gzip` on it
embeds the *file's* mtime and the same commit gets a different sha256 every run, which is a
checksum nobody can reproduce. The one remaining variable is a configured `tar.tar.gz.command`,
which swaps in the machine's own compressor — different bytes, same tar inside — and the
preflight warns when one is set. Across git versions the gzip layer may differ; the tar layer
cannot, so `gunzip -c | sha256sum` is the comparison that always holds.

**A dirty tree is a preflight failure**, and this is the check worth having. `git archive`
packages `HEAD`, so uncommitted work produces a valid tarball of something the developer is not
looking at — a false claim about a commit, discovered much later as a bug report against code
that was never released. The verdict names the commit and the files. Untracked files are only a
warning: they are usually scratch, and the narrow bad case (a new source file the committed code
already imports, so the tarball fails to build while the author's tree builds fine) is worth a
sentence rather than a refusal. There is no `--allow-dirty`; a flag like that exists to be pasted
into a script.

**It is in the Linux default set only**, and that is the one place `Targets::for_host` stops
meaning "what this host can build". A Mac runs `git archive` perfectly well and `--src` there is
honoured — the cross-compilation refusal never names it. But a release matrix that produced it on
both hosts would upload two files called `cide-0.1.0-src.tar.gz` whose gzip layers differ, and
whichever upload lost would leave the published checksum matching neither job. One artefact, one
producer.

Unpacked, the tarball builds: `cargo metadata --locked --offline` resolves the whole graph,
`ui/src/ipc/generated.ts` and every `crates/cide-ipc/bindings/*.ts` are tracked so `tsc` needs no
prior codegen, and `contract-check` runs green inside it. What it correctly does not carry is
`.git`, `target/`, `ui/node_modules` and `ui/dist`. Running `--src` from *inside* an unpacked
tarball is therefore a preflight failure with that sentence in it, rather than a confusing error
out of git.

**What the tarball is still missing is a licence.** `Cargo.toml` declares
`license = "MIT OR Apache-2.0"` and the generated AppStream metainfo publishes the same string,
but there is no `LICENSE-MIT`/`LICENSE-APACHE` at the root, so there is none in the archive
either — and Apache-2.0 §4(a) requires the licence text to accompany redistributed source, which
a tarball on a public release page unambiguously is. Adding the two files fixes it with no code
change (they are tracked, so they land in the archive for free). It has not been done because the
MIT text needs a copyright holder named, and that is not a choice this repository can make for
itself.

### Paths stay XDG, deliberately

`$XDG_STATE_HOME/cide/workspace.json` and `$XDG_CONFIG_HOME/cide/keymap.json` work on macOS —
nothing fails — but they are not the platform convention, which is
`~/Library/Application Support/dev.cide.ide`. They are staying where they are, and this is the
reason rather than an omission: `keymap.json` is a file people hand-edit and hand around, every
piece of documentation here names one path, and `app_config_dir()` would fork that into two
answers for one question. Hook sockets already fall back from `$XDG_RUNTIME_DIR` to
`std::env::temp_dir()`, which is unset-and-therefore-`$TMPDIR` on every Mac — a per-user 0700
directory, with an explicit 0600 chmod on the socket besides.

## Panes, tabs and the reopen stack (M15), and what is not done

| key | what it does |
| --- | --- |
| **Ctrl+Shift+T** | reopen the last closed tab, and the one before that |
| **Ctrl+W** | close a tab — and land on the tab you came *from*, not the one to the left |

**The focus ring is gone from editor panes, and only from editor panes.** It was added in M14 for
every pane kind and the user asked for half of it back: two accent pixels around the file you are
reading are noise. It stays on `claude`, `shell` and `diff` panes, where several terminals share a
tab and nothing else answers "which one am I typing into" — an editor answers that itself, because
CodeMirror hides its cursor when the surface is blurred. **The state behind it did not move.** The
removal is two CSS rules under `.frame[data-kind='editor']`; `PaneTitleBar.tsx` still applies
`frameFocused` from `focused` for every kind and still publishes `data-focused`, so
`keys/context.ts` goes on deriving `paneFocused`, `editorFocused` and `fileTabActive` from
`tree.focused` and the clauses that gate `file.save`, the find bar, the outline, Find Usages and
`pane.navigate.*` are untouched. `check:rows` pins both halves — that the exception exists, and
that the class and the attribute are still unconditional — because the rationale for *adding* the
ring is still a dozen lines above the exception and has to be, the ring is still drawn for three
kinds out of four. The canvas's 3px gutter stays too: it was justified by the ring in the comment,
but what it actually fixes is a pane's ordinary `--border` sitting flush against the app's.

**Closing a tab activates the most recently used survivor, and that moved the MRU into Rust.** The
old rule was the tab to the *left*, which is strip position, which is insertion order — so closing
a file opened an hour ago dropped you beside whatever happened to be opened just before it.
`Project::tab_mru` is now a workspace field, written by the one `set_active` that every writer of
`active_tab` goes through, and `close_tab` picks the first survivor in it. The webview's
`localStorage` copy under `cide.tabMru` is **deleted**; `ui/src/store/workspace.ts` reads the order
off the snapshot and only `reconcile`s tabs the order has never seen onto the end so the switcher
can walk to them.

The argument that had kept it in the webview is in `store/workspace.ts`'s own note, which admitted
it was weak. What settled it: **`close_tab` has to pick a successor and `close_tab` is in Rust**,
and two of its callers have no webview to ask — `cide_app::ide` closes a withdrawn diff from the
MCP server's thread, and the quit ladder closes projects wholesale. A successor computed in a
renderer would have had to exist twice, and the copy in Rust would have been the rule this replaces.
The two objections evaporated: the field moves only inside mutations that already bump `rev`, so it
costs no extra broadcast, and `#[serde(default)]` plus `repair_tab_mru` on load is not a schema
break — `CURRENT_SCHEMA` stays at 2.

**A workspace written by any earlier build still opens.** That is the expensive thing to get wrong:
`tab_mru` defaults to empty, `validate` refuses an empty order, and `WorkspaceState::load` answers a
failed validation by replacing the whole workspace with defaults — so a missing repair would greet
every existing user with no projects, with their file intact on disk and never read again.
`persist::load` repairs to `[active_tab]` and **invents nothing else**: strip order is not use
order, and a plausible-looking history would make the first close land somewhere the user has never
been. `close_tab`'s left-neighbour rule is kept as the fallback for exactly that state.

**Ctrl+Shift+T reopens closed tabs, and keeps going.** The stack is 16 deep (`persist::MAX_RECENT`'s
number), lives in `cide_app::closed_tabs` — Tauri state, *not* `Workspace`, so a close does not
broadcast the tree for a record nothing draws — and is popped **project-scoped**, so Ctrl+Shift+T in
a window showing project A can never resurrect a file from B. A record carries the project, the tab
kind, the strip index and the pane tree *with its original pane ids and session bindings*: the
frontend's `paneHosts` map is keyed by pane id and a tab close does not clear it, so a reopened
`ClaudeFull` tab re-adopts its own parked terminal and its live conversation rather than starting a
second one. Scroll and caret need no record at all — `positions.json` is keyed by path and
`EditorPane` flushes on unmount.

Three things it deliberately does not do. It **does not survive a restart**: the gesture means "I
closed that ten seconds ago by accident", and the first press of a session reopening something
deliberately closed last week is the opposite of that. It **never remembers a diff Claude is
blocked on** — closing that tab cancels the agent's request, so the `request_id` names nothing —
and it filters at *push* rather than at pop, because a record that can never be popped is a hole
the user counts through. And it carries **no `when` clause**: the depth is not in the snapshot by
design, so a flag for it would need a supplier the snapshot has no business carrying, and a flag
with no supplier is the `repoOpen` mistake that hid the whole Git group for a milestone. The row is
always offered and reports "nothing to reopen" instead.

`theme.toggle` gave up `ctrl+shift+t` and ships **unbound** — palette-only, like
`project.switcher.prev`, and one line of `keymap.json` takes it back. No gate requires a registered
command to carry a binding, and `theme_toggle_ships_unbound_and_is_still_a_command` exists to stop
one being added: "every palette row has a key" sounds like an invariant and is not one. **Ctrl+T
still pulls.**

**Not done.** None of it has been confirmed on screen — same reason as M14, KDE will not raise a
shell-launched window. The reopen decision is covered by `reopen_plan`'s test and the stack by
`closed_tabs`'s, but `tab_reopen_closed` itself is a Tauri command and the three lines that wire
`stack.pop` to `reinsert_tab` are checked by nothing. A **`ClaudeFull` tab reopened after its child
has exited** comes back as a Resume splash rather than as a live pane; that is the existing restore
behaviour and not new, but Ctrl+Shift+T is a new way to reach it. The stack is not offered in the
tab strip's right-click menu: a menu item that cannot be honestly disabled — the depth is not in the
webview — and that silently does nothing is the surface where silence is worst, so the two routes
are the chord and the palette.

## Scrolling a Claude session (M16), and what is not done

**The scrolling belongs to Claude Code, not to cide.** A Claude pane runs on the alternate
screen (`ESC[?1049h`, confirmed live from the spawned CLI's first 60 bytes), and the alternate
buffer has no scrollback by definition — so there is no cide-side scrollback to give. What
happens instead is that xterm.js forwards the wheel to the child as an SGR mouse report and the
CLI scrolls its own transcript. That path works: driving a resumed 2.3 MB session, 400 wheel-ups
travelled roughly 1200 transcript lines without ever reaching the top, and every single report
produced a repaint. `PageUp`/`PageDown` scroll it too, and are bound to no command in
`cide_core::keymap`, so the gate passes them straight through.

Two numbers multiply to give what one notch of the wheel is worth, and **cide owns only one of
them**. xterm.js sends at most **one report per DOM wheel event** — `sendEvent` computes a line
count and then discards it — and `CoreMouseService.consumeWheelEvent` scales any event with
`|deltaY| < 50` by `0.3`, believing it to be a trackpad. A high-resolution wheel under Wayland
delivers one notch as several small deltas, which is exactly that regime. The other number is
`CLAUDE_CODE_SCROLL_SPEED`, the transcript lines the CLI moves per report, and that is now
**Settings → Claude sessions → Scroll speed**, 1 to 20.

**Three environment switches were reachable from nothing, and that is case #20.** `disableMouse`,
`altScreenFullRepaint` and `disableAlternateScreen` were declared in `cide-ipc` with doc comments
naming the variable each one sets, persisted, bound to TypeScript, and drawn as switches under a
panel reading *"Applied at spawn — these reach a pane's child process when it starts"*. Nothing in
the workspace read any of them; `resumeAllOnLaunch` was the struct's only field with a consumer.
It mattered more than the usual instance, because *"Disable the alternate screen"* is precisely
the control a user chasing a scrollable transcript reaches for. They are wired now, through
`cide_core::child_env::claude_env` into `base_env`, and `check:claude-env` compares the set of
variables the Settings screen names against the set a spawn actually sets, in both directions.

**Not done.** Whether this machine's mouse is in the `< 50` regime is **unmeasured** — it needs a
physical wheel over a running window, and no lane may launch the app while sibling `claude`
children are alive. The measurement is `./run.sh --inspect`, then
`addEventListener('wheel', e => console.log(e.deltaMode, e.deltaY), {capture: true})` over a Claude
pane, one notch. Several events each under 50 means the trackpad clamp is eating the gesture, and
the fix is `scrollSensitivity` — a real xterm 6.0.0 option, and the *only* lever that does not
require cide to encode mouse reports itself, which ADR 0003 assigns to xterm. It must be applied
**only while the pane is in the mouse-reporting regime**: the normal-screen path is a separate
VS Code `ScrollableElement` with no trackpad clamp, so raising it globally would speed up a bash
pane's scrollback, which is not broken. `shift`+wheel is inert in a Claude pane and stays that
way — xterm drops it in `consumeWheelEvent` before any protocol runs, confirmed at zero bytes.

## The tab strip: drag to reorder, Close to the left, and Back into a closed file (M16)

| gesture | what it does |
| --- | --- |
| **drag a tab** | move it along the strip; an accent caret shows where it will land |
| **right-click → Close to the left** | close everything before this tab, asking about unsaved work once |
| **Back / Forward** (the mouse's thumb buttons) | reopen a closed file *as it was* — split, pane ids and all |

`navigate.back` and `navigate.forward` have **no key binding on Linux or Windows** — the thumb
buttons are it. On **macOS they are ⌘[ and ⌘]**, added to `keymap::platform_layer` rather than to
`defaults()`, because `ctrl+[` *is* the ESC byte on every terminal and the key gate is a window
capture listener: the Linux spelling would take Escape-equivalent from every pane in every window.
The cost on macOS is `defaultKeymap`'s `Mod-[`/`Mod-]`, which are indent and dedent — the
capability survives on Tab and Shift+Tab (`indentWithTab`), which is what IDEA-on-mac binds anyway,
and `check:editor` pins that by command identity.

`Ctrl+Alt+←`/`→` are not them and must not be written down as if they were: `ctrl+alt+right` is
`pane.split.right`, which on any tab but the console splits the pane and spawns a *new* `claude`
child, so a reader who trusted that line got a running process instead of a navigation. *Language
support (M12)* below carries the `keymap.json` entries if you want keys as well —
`ctrl+alt+shift+left`/`right` are free in every layer.

**Reordering was implemented and reachable from nothing, and that is two more for the count.**
`cide_core::workspace::reorder_tab` has been complete, invariant-preserving and covered by three
passing tests since M4, and no `#[tauri::command]` called it: the domain rule existed, the seam did
not, and the tests said nothing about whether a user could do the thing. That is case #18 of this
project's signature defect. **Case #19 is still open beside it**: `project_reorder` is complete end
to end — `workspace.rs:217` → `cmd/project.rs` → registered in `lib.rs` → exposed as
`project.reorder` in `ui/src/ipc/client.ts` — and `grep -rn reorder ui/src` finds that definition
line and **zero call sites**. The header's project tabs have no drag handling at all. It is written
here rather than fixed, because the fix is the same shape as this one and belongs with a gate.

`tab_reorder` takes **ids, not indices**, and a `before` boundary rather than a destination. The
drop position is computed in the webview at pointer-move time against a snapshot, and an agent's
`openDiff` or another window's close can move the strip before `pointerup`; an index would then
move whichever tab now sits there, silently and somewhere nobody aimed. Both ids resolve to indices
*inside* the workspace lock. The boundary→index conversion (`to = boundary > from ? boundary - 1 :
boundary`) is a free function with tests, because the wrong version makes every rightward drag a
no-op while every leftward drag works perfectly — the sort of half-working a manual test passes.

**Pointer events, not HTML5 drag and drop**, the third time this repository has settled that
question: Tauri's native drag-drop handler is on, `windows.rs` never calls
`disable_drag_drop_handler()`, and a gesture that works in `pnpm dev` and does nothing in the
shipped app is the worse failure. The rules live in `ui/src/chrome/tabDrag.ts`, which imports
nothing so `check:tab-drag` can compile and run it; `useTabDrag.ts` is the mechanism and no check
in this repo can execute it. Feedback is a **2px caret**, never a gap that opens between two tabs —
the strip is 30px tall and reflows *under the pointer*, which is the rule its stylesheet already
spends a paragraph on for the awaiting marker. The pinned console is refused three ways so the
refusal is never reached: the grab returns `null` for it, the caret clamps to boundary 1, and the
drop refuses a boundary of 0. Rust refuses it a fourth time, and that is the authority.

**The three bulk closes ask once.** *Close others*, *Close to the left* and *Close to the right*
used to fire one `closeTab` per tab; each did its own risk round trip, each parked its own refusal,
and the queue in `closeConfirmStore` showed them one after another — three modals each headed
*"Closing this tab"*, each naming a different file, for a gesture the user made once. They now go
through one `closeTabs`, which asks `quitRequested` once, narrows it to the tabs actually closing,
and raises **one** dialog under the new `tabs` scope that names every file at stake. `Close to the
left` filters by `closable` rather than testing `index === 0`, which is what makes it skip the
pinned console without knowing it exists — and it is offered-and-disabled rather than dropped when
the set is empty, because that is the *common* state (every tab at index 1 has only the console to
its left) and an item that comes and goes with the tab order is one the user has to hunt for.

**Back reopens a closed file through the closed-tab stack, and peeks rather than pops.** It used to
call `tab_open_file`, which mints one fresh single-pane editor and knows nothing about how the tab
left. Two things were wrong with that. A file tab's pane tree can hold a **live Claude session** —
splitting a File tab defaults to `NewClaude` — and `reinsert_tab` exists precisely to hand those
pane ids back to `paneHosts`. And it silently poisoned Ctrl+Shift+T: Back opened a fresh tab for X
while X's record was still on the stack, so the next press popped that record, found X open and
*consumed it anyway*, landing the user on an unrelated tab with the split gone for good.

`tab_reopen_file` therefore stats first, then **peeks** at the record and **takes it only when it
actually reinserts the tab**. The cost of each alternative, since both were tempting: popping would
hand Back whatever closed most recently rather than the file it has in mind, and consuming on a
mere activation would spend a record with nothing on screen to explain where it went — while
ignoring the stack entirely is the fresh-single-pane behaviour above, every time. A walk into a
file that is gone answers `gone` instead of minting a permanent tab reading *"This file could not
be opened"* with the path nowhere in the sentence; the entry is dropped so the next press does not
walk into the same wall, and `navHistory::abandon` puts the cursor back where the user actually is
rather than where the failed walk left it.

**A refused terminal open is no longer a place you have been.** `App.tsx` recorded the history
entry *before* `openFromTerminal`, so a path the user was shown and **declined** in the
out-of-project dialog stayed on the Back stack — where Back reached it again through a command that
by its own documentation enforces nothing. The entry is now written from the `.then()`. The
*origin* is still read before the call, through `pendingJump`'s closure: the open broadcasts from
inside the workspace lock, so the destination's editor can already have claimed `caretTrack`'s slot
by the time the promise settles, and reading the caret there would record the destination as its
own origin and leave Back with nowhere to go.

A Back entry's recorded position beats the per-file view memory, and that was confirmed rather than
assumed: `planRestore` refuses to restore while a reveal is parked for the path, `EditorSurface`
runs it *before* `registerReveal` spends the request, and `check:editor` pins that ordering. An
`UNKNOWN_LINE` entry parks nothing and correctly defers to the view memory, because that entry
never named a line.

**Not done.** None of it has been confirmed on screen — same reason as M14 and M15, and this batch
adds a pointer gesture, which is the kind that most wants a real window: `--audit-chrome` is worth
one run to confirm the caret changed no measured dimension. The strip **clips rather than scrolls**
(`.tabs` is `overflow: hidden`), so a tab past the right edge is neither painted nor hit-testable
and cannot be a drop target; the caret clamps to the last visible boundary. That is inherited, not
introduced — making the strip scrollable interacts with the awaiting marker's reserved box and is a
separate change. Half of it has since been answered: the strip's right end now carries a `▾` that
lists the tabs currently out of view and activates the one picked (`chrome/tabOverflow.ts`,
`check:tab-overflow`), so a clipped tab is **reachable** again. The rest of the sentence still
stands — a clipped tab is still not a drop target, and activating one from that list switches the
pane without bringing its tab back into view, because the strip still does not scroll.
`tab_reopen_file` resolves its plan under one lock and reinserts under another, so
a second shell window opening the same file in between can produce two tabs over one path; that is
the same window `tab_reopen_closed` has always had. And the **bulk close stops at the first
unanticipated refusal** rather than marching on — a file that turns dirty between the one question
and the closes parks a dialog about itself, and the tabs after it stay open.

## The find bar spans the file, and the path trail is clickable (M16)

Two reports about the bottom and the top of an editor pane, and both of them are about a control
that is drawn and does nothing.

### The cluster steps below the find bar, instead of the find bar stopping short of the cluster

The pane's ⊞ ⛶ ⧉ × cluster floats in the top-right corner, and a CodeMirror top panel is
`position: sticky; top: 0` across the editor's full width — so the find bar lands on exactly those
pixels, and the cluster wins the paint order (`.body` is a stacking context at `z-index: 0`, which
was added deliberately after the *reverse* shipped and buried all four buttons). The M15 answer was
to reserve horizontally: `margin-right: var(--pane-corner-clear)`, 221px on an editor pane. The user
rejected it — a search field is the width of the thing being searched — and it was also failing on
its own terms, because `.findBar`'s own controls need about 217px, so under roughly 440px of pane
the reserve started clipping the buttons it was protecting.

So the bar keeps its width and the **cluster moves down by exactly one bar**. Where it rests:

| state | find bar | cluster `top` |
| --- | --- | --- |
| nothing pinned above the buffer | — | `0` |
| find bar open | full width of the file content | `--h-findbar` (33px) |
| conflict bar up | — | `0` |
| both | full width, *below* the conflict bar | `0` |

The cluster moves in exactly one state, and it is the common one. In *both*, the find bar is no
longer the topmost strip, so none of it is under the cluster and nothing needs to move.

**The conflict bar keeps its reserve, and that is not an inconsistency — it is the rule.** A strip
whose height is a constant can be stepped over; a strip whose height depends on its content cannot.
`.conflictText` is `flex: 1; min-width: 0` with no `white-space`, so *This file changed on disk while
you had unsaved changes* wraps and the bar grows a line on a narrow pane, and a step sized for the
unwrapped case would land the cluster in the middle of **Keep mine** — which is the exact bug M13
fixed there, arriving back by a different route. That bar therefore goes on reserving
`--pane-corner-clear` horizontally, `EditorPane.tsx` marks it `data-pane-strip="fluid"`, and the
frame's rule stands down while it is up. Both bars are fully clickable in every one of the four
states above.

**"Full width" means the width of the file content, not of the pane.** The bar stops at
`var(--w-minimap)`, which is where `.cm-scroller` stops: `.cm-panels` is `z-index: 300` and the
minimap is a sibling canvas at `right: 0`, so a literal 100% would paint over its top 33px and take
its clicks. The bar's `border-bottom` now meets the minimap's `border-left`.

**A too-short pane.** `MIN_RATIO` is 0.1 and splits nest, so a pane under 60px tall is reachable by
dragging, and `.frame` has no `overflow: hidden` — an unclamped 33px would paint the cluster over
the pane *below*. It is `clamp(0px, var(--pane-top-strip), calc(100% - var(--h-panetitle)))`, and
the comment there is honest that below ~59px there is no correct resting place at all: the clamp
only chooses *inside my own frame, over my own find bar* rather than *outside my frame, over
somebody else's pane*. When the maximum goes negative the spec yields the minimum, which is `0px`.

**Terminals do not move, and that is the same rule returning zero** rather than an exception: the
cluster sits at the top of the pane's *content*, and a terminal pins nothing above its transcript.
Forcing every kind down 33px for symmetry would park four buttons over line 2 of every transcript
for ever, and the reveal is `.frame:hover`, so two clusters are essentially never on screen at once
and there is no misalignment anybody could observe.

The mechanism is `:has()` on the frame, which is this codebase's first — custom properties inherit
*downwards*, so nothing the editor declares can reach a box that is its ancestor's sibling, and
`:has()` is the only selector by which a descendant's existence reaches an ancestor's computed
style. The five alternatives that lost (including CSS anchor positioning, which is literally the
feature for this problem and falls back to `top: 0` on any WebKitGTK older than Safari 26 — and
`top: 0` *is* the bug) are written up in `PaneTitleBar.module.css`. `check:rows` pins the whole
thing: that the panel does **not** read `--pane-corner-clear` any more, that both ends of the step
name `--h-findbar`, that the token's value re-derives from the bar's own padding and control
height, that exactly one rule sets the step and it is scoped and excludes the fluid strip, that the
conflict bar declares itself, that the `top` stays clamped — and that the find bar is still the
**only** top panel in `src/editor/`, since a second one would stack inside the same box and leave
the cluster resting on it.

**Not verified by a gate:** `:has()` invalidation cost with a live terminal in a sibling pane. The
argument is that `.cm-panels-top` never appears in xterm's mutations so WebKit should not
invalidate, but nothing in `ui/scripts/` runs a browser; the four states and a deliberately short
split want `./run.sh --audit-panes` and a pair of eyes.

### Every segment of the status bar's path trail that leads somewhere is clickable

`crates › cide-core › src › lib.rs › impl Parser › parse` is drawn from `editor/statusReadout.ts`.
Three things were wrong with it and they are separable.

**It only rebuilt when the buffer was focused.** The claim stack was moved by exactly two things —
mounting and DOM focus — and a tab switch is neither: `TabContent` never unmounts an inactive tab,
so every open file holds a live claim for as long as the project does, and `file_open` is
open-*or-activate*. Switching to an already-open file therefore moved nothing until the user clicked
into the buffer, and on restore every tab mounted at once in `file_read` completion order and **the
last one to land owned the bar**, whichever tab was in front. This is *not implemented* rather than
implemented-and-unreachable — but it is a near miss of the usual kind: `TabContent.renderTree`
has taken `(tab, active)` since M4 and documents that flag for exactly this class of consumer, and
`App.tsx` wrote `renderTree={(tab) =>` and threw it away. The flag now reaches `EditorSurface`,
where a mount behind another tab is inserted at the *bottom* of the claim stack and a tab coming
forward calls `focus()`. Two further rebuilds were missing: the **outline arriving late** (the parse
is asynchronous, so a freshly opened tab had a path and no symbols until the caret moved) and, worse,
the late-*root* repair published the path alone and so **truncated** a symbol tail that was already
on screen. There is now exactly one `setTrail` call in that file, and `check:editor` counts it.

**A third bug, found on the way:** `segments` is memoized on `[path, root]` while the build effect
is keyed on `[path, reloadKey]`, so a root arriving late gave the update listener a stale array —
the repair landed and the very next caret move published the absolute path back over it. It reads
through a ref now.

**Which segments are clickable.** A segment is revealable exactly when its absolute path is at or
under a *reveal root* — a path the file tree can hang a row from. That is `rootOf`'s longest-match
containment, the same test `Groups::owner_of` applies in Rust, and the list is `fs_reveal_roots`:
the project's roots plus **each group's top-level children**. So on a dependency source,
`serde-1.0.229 › src › de › mod.rs` is live and `home › u › .cargo › registry › src ›
index.crates.io-…` is not, which is the report. The list has to come from Rust: those are *package*
directories, not the caches above them, and the SDK row's directory comes out of
`rustc --print sysroot`, so a depth rule guessed on the frontend would mark
`lib › rustlib › src › rust › library` clickable and every one of those clicks would fire the notice
this feature exists to stop showing.

The rule is `sidebar/rowPaths.ts::crumbTargets` — pure, import-free, and driven by
`check:tree-status` over all three populations (a project file, a library source, a file under
nothing) plus a scratch, a nested second root, and the degradations. It takes `pathCount`, the
number of leading crumbs that are path rather than symbol, because without it `parse` classifies as
`…/lib.rs/impl Parser/parse`, comes out inside the project root, and is drawn live.
`StatusBar.tsx` used to say so itself: *"deliberately does not know where the path ends and the
symbols begin… no crumb is clickable yet"*.

**An inert segment gets no notification, because it is not a control.** It is drawn inert instead:
the revealable run is `--dim` against the trail's `--faint`, so the boundary is legible at rest with
no badge and no icon, and only the live ones underline under the pointer. The cursor is `default`
and never `not-allowed` — a forbidden cursor is a refusal, which is the same message the user
rejected with a different renderer. A live crumb runs `runCommand('file.reveal', { path })`, the
same command ⌃⇧E, the palette row, the Explorer's ⌖ button and a Ctrl+click on a directory in
terminal output run; there is no second reveal path. In a window with no sidebar the handler is
withheld entirely, so the whole trail is inert rather than live-and-refusing.

**Where it is still only a necessary condition.** Containment is all a pure function can know:
inside a reveal root, whether a *particular* directory has a row also depends on the project's
ignore rules, so a gitignored file's crumbs are drawn live and report themselves the way ⌃⇧E
already does for the file itself. That residue is uniform — if the file is ignored, so is every
ancestor crumb of it — and closing it would mean an IPC round trip per crumb. And a library crumb
is inert until *External Libraries* has resolved: `fs_reveal_roots` deliberately never triggers a
resolution (a `cargo metadata` inside a status bar's render is not a trade worth making), so the
crumbs light up on the `cide://fs-status` the resolver emits. The store re-asks exactly when the
composed row count moved, which is provably every burst that can grow the list and no ordinary one.

**No tab stops.** `BranchSelector` is the bar's one focusable control and six to ten new focus stops
for a path trail would cost more than the gap they close; the keyboard route to the same place is
⌃⇧E and is bound. Crumbs carry `role="link"` and stay inline `<span>`s — `.path` needs
`text-overflow: ellipsis`, which requires inline content, so a `<button>` would silently coarsen the
ellipsis to whole-crumb granularity. Clicking the *last* crumb to open File Structure at its
siblings, which is IDEA's other breadcrumb gesture, is still not done.

## Which `claude` is launched, and with what (M16)

**Settings → Claude sessions now names the binary, extra arguments and extra environment**, and
every Claude pane, plus the one-shots behind *Generate commit message* and *Explain selection*, is
spawned from it. Before this the program was the literal `claude` in five places. Nothing was
un-stranded: grepping for every shape of it (`cli_path|claude_path|binary_path|extra_args|extra_env`)
across `crates/` and `ui/src/` returned zero hits, so this is **not implemented**, not
built-and-unreachable.

### The binary is checked at save, and the check is a warning far more often than a refusal

A bare name stays a bare name. `claude` is passed to `execvp` unresolved so the OS resolves it at
each spawn — the CLI self-updates, and pinning the answer at launch would keep panes on a version
that no longer exists. cide resolves it only to *check* it and throws the path away.

Only three verdicts are hard, and they are the three the OS cannot `exec`: blank, a bare name on no
`PATH` directory (`~/.cargo/bin` and `~/go/bin` included, because a desktop-launched cide has a
different `PATH` from a terminal-launched one), and a path that is not an executable file. Everything
else warns. `mise`, `asdf`, `direnv` and a plain shell wrapper are all legitimate ways to name a
`claude` and none of them answers `--version` in a shape worth refusing over — the project already
paid for the opposite arrangement once, with `~/.cargo/bin/rust-analyzer`, which is a symlink to
`rustup` and passes any on-PATH-and-executable probe before failing at exec.

**The parenthesis in that paragraph was a half-truth until M17**, and it is worth naming because
it is the shape of bug this whole area produces: cide accepted a `claude` in `~/.cargo/bin` that
was not on `PATH`, then passed the bare name to a spawn whose child searched `PATH` only, and the
check and the exec disagreed. `base_env` now appends `toolchain::extra_dirs()` to the child's
`PATH`, so the two consult the same list and the bare name is kept in every case. See *Finding
`claude` from a Finder-launched `.app`* under Platforms.

The verdict is unlatched and per-binary, which is a change: `version::check_once` describes whichever
binary was probed first in the process, which was free while `claude` was a constant and became a lie
the moment it was a setting — a user who fixes a typo would read back the verdict for the binary that
answered ten minutes ago. `run_headless` still calls the latched one for its one-line-per-process log
warning, and its comment now says which binary that line is about.

A binary that cannot be run also fails at *spawn*, as `SessionError::NoClaudeBinary`, whose written
sentence `spawnFailureText` prefers verbatim and `TerminalPane` writes into the failing pane's own
transcript. It is deliberately outside `isRecoverableSessionError`: retrying cannot help, and a pane
that retried would spin.

### What is refused, and where the refusal is legible

Seven arguments and seventeen variables. The spellings were read out of `claude --help` on 2.1.233
and every variable name was checked against the strings in the installed binary, rather than
remembered.

| refused | why it is not merely discouraged |
| --- | --- |
| `--session-id`, `--resume`/`-r`, `--fork-session`, `--continue`/`-c` | the uuid **is** the pane's `SessionId`; a second one makes every hook frame name a session this process never heard of |
| `--settings` | duplicates cide's inline hook payload; if the user's wins, every hook dies silently |
| `--bare` | authentication becomes strictly `ANTHROPIC_API_KEY` or `apiKeyHelper` — an auth failure in every pane for a Max subscriber |
| `--print`/`-p` | turns an interactive pane into a one-shot that exits |
| `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` | a key outranks subscription OAuth, so it would silently bill a Console org — and the sentence already on this screen promising cide never sets one would become a lie |
| `CLAUDE_CODE_SSE_PORT`, `CIDE_HOOK_SOCK` | cide's own; a wrong value binds a pane's `claude` to another editor's lockfile |
| `TERM`, `COLUMNS`, `LINES`, `TMUX` | cide sets these and this list is folded *after* them, so an accepted value would win |
| the four `CLAUDE_CODE_*` names above | there is a switch for each; two controls writing one variable is how the one you can see loses |
| the four proxy names | the proxy screen has three modes and three scopes so "no proxy" and "do not interfere" stay distinguishable, and it is applied *after* this list |
| `CLAUDE_CONFIG_DIR` | `lifecycle::claude_projects_dir` reads it from **cide's own** environment to find a transcript; set for the child alone, every Resume button vanishes |

`ANTHROPIC_BASE_URL`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX` and `--safe-mode` are
**warned, not refused** — they work, they mean something serious, and they are the user's decision.
The line is whether *cide's own* behaviour becomes wrong.

**Nothing is rejected on save.** `useSettings.ts` sends its patch fire-and-forget with
`.catch(() => {})`, so a `settings_set` that returned `Err` is invisible — the field snaps back and
says nothing. Everything is therefore stored verbatim and the readout says what the child actually
gets: a refused token stays in its row, struck through (a strike, not a colour: a dim monospace input
against a dim placeholder is a distinction nobody makes at a glance and one that vanishes entirely
for a colour-blind reader), absent from the resolved argv below it. The proxy screen already made
this argument about a password and answered it the same way.

Enforcement is at the spawn as well, and that is not belt-and-braces: `workspace.json` is
hand-editable and `settings_set` is one `invoke` away from being bypassed, so a UI-only filter would
let a hand-edited file cost somebody their `--resume`. `apply_patch` makes the same argument three
times over for its clamps.

### The two orderings that are load-bearing

**The user's arguments go first.** `--add-dir`, `--mcp-config` and `--tools` are variadic and collect
every following non-flag token, and every argument cide appends begins with `-` — so a user flag
placed first can never swallow cide's session id, and one placed last would swallow whatever cide
wrote. `cide_claude::headless::argv` refuses the same wager for the same reason.

**The hooks decision is made before the binary is substituted.** `program_is_claude` matches on a
file name, and its own note predicted this exact failure: *"the moment anything passes an absolute
path as the program — a configurable CLI location in Settings, say — this returns false, no
`--settings` is attached, and every session runs with no hooks."* It is closed by ordering rather
than by teaching that function about paths: the decision is made from what the frontend asked for,
which is still the bare string `claude`, and the substitution happens downstream of it. Teaching it
about the setting would still answer `false` for `~/.local/share/claude/versions/2.1.233`, which is
the value a user pins when they want a specific version.

### Two deliberate asymmetries, stated so they are not "fixed" later

* **The extra environment reaches `claude` panes and the one-shot lane, and not shell panes** — where
  the four `CLAUDE_CODE_*` switches above *do* reach. Those names are inert to `bash`; an arbitrary
  `NODE_OPTIONS` or `PATH` is not, and a field labelled *the environment claude is spawned with* must
  not quietly become the user's shell's.
* **The extra *arguments* do not travel to the one-shot lane at all.** `headless::argv` is cide's own
  machinery — `-p --output-format json --no-session-persistence --tools` — and a user `--model` or
  `--tools` folded into it does not customise anything, it breaks the parse of a reply that never
  arrives. `ProxyScope::claude` covers both lanes because a proxy is one thing to a user; an argument
  vector is not.

**`ClaudeSettings` lost `Copy`** (a `String` and two `Vec`s cannot be) and `ClaudeCli` implements
`Debug` by hand, redacting values and keeping names — `Settings` derives `Debug`, so a
`tracing::debug!(?settings)` anywhere would otherwise print a token the user typed into the env
editor. Two tests guard it, modelled on `ProxySettings`'s, one of which reads the field list off
`serde_json` so it cannot fall behind either.

**Fixed while here:** `cide_claude::version::probe` built a bare `Command` and did not scrub, while
the identical probe in `cmd/app.rs` did and said why. Under the AppImage it ran a Node with
`PYTHONHOME` pointing inside the bundle — the ADR 0007 environment — so the version cide reported
came from a process launched in a way no pane is ever launched in.

**Not done.** No end-to-end confirmation: nothing in `ui/scripts/` mounts React against a live
backend, so "typing a bad path makes every pane print the same sentence" is argued, not observed. The
refusal list is a list of somebody else's flag names and **will go stale**, exactly as `SUPPORTED_CLI`
does — it belongs in `version.rs`'s table of what a patch release can retract, and the honest gate is
an `#[ignore]`d real-CLI test putting these shapes in front of the installed binary, which is not
written. `CLAUDE_CONFIG_DIR` is refused rather than supported; teaching `claude_projects_dir` to read
the setting is the better answer and is a change to the resume path, not to this screen. And there is
no way to spell *remove* in the environment editor, deliberately: a removal control would let a user
delete the `ANTHROPIC_API_KEY` their own login environment carries, which is the only credential some
Console customers have.

### The arguments cide adds are switchable, and renameable (M17)

**Settings → Claude sessions → *What cide adds to the command line* has one row per argument cide
injects** — the session id, the resume, the fork flag and the hook settings — each with a switch and
a spelling. It exists because a pane is the obvious place to drive a different harness (`opencode`,
or a wrapper around Claude Code) and cide always added `--session-id` and `--settings` regardless of
the configured binary: `program_is_claude` answers from what the *frontend* asked for, which is the
bare string `claude` for every Claude pane however the binary is set, and that ordering is correct
for the reason above and is not going to change.

**Defaults preserve today's behaviour exactly**, and that is the one property with no runtime symptom
if it breaks. `bool::default()` is `false` and `ClaudeCli` carries `#[serde(default)]`, so a
*derived* `Default` on `ClaudeInjection` would load every `workspace.json` already on disk with all
four injections off — no hooks and no resume, for every user, on the launch after an upgrade, from a
screen they never opened, with the pane starting perfectly well and simply reporting nothing. The
`Default` is written by hand with that paragraph beside it, and pinned from three sides:
`cide-ipc`'s old-JSON round-trip test, `cide-core`'s `the_default_injects_exactly_what_this_build_injected_before_the_switch_existed`,
and `check-claude-cli.mjs`, which reads the `impl` out of the Rust.

**What each switch costs is stated on its own row**, not in a note at the foot of the section, because
every one of them silently disables a feature that gets reported later as something else:

| off | what stops |
| --- | --- |
| `--session-id` | no transcript is filed under the id the pane is saved with: **resume stops working**, and the Resume button over a dead pane goes away. *Not* the status bar — token figures and busy/idle chrome ride on the hooks and on `CIDE_SESSION`, which cide sets separately |
| `--resume` | every restored pane starts fresh; cide stops *offering* resume rather than offering a button that quietly starts a new session (`plan_restore` and `session_resumable` both read the switch) |
| `--fork-session` | *Split → fork* continues the conversation instead of branching it |
| `--settings` | **no hooks at all**: no token or cost figures, a close confirm that cannot tell busy from idle, buffers reloading on a poll instead of on a tool write, no finished-turn notification — and no `theme`, so Claude Code draws itself dark inside a light pane |

**The refusal table and the injection table are one fact.** `REFUSED_ARGS` refuses `--session-id`,
`--resume`, `--fork-session`, `--settings` and `--continue` *because cide passes them*; stop passing
one and the refusal has to lift, or the screen disables a feature and then forbids the replacement.
Rather than two lists, each row carries `because: Option<Injection>`, and a rename moves the refusal
to the new spelling: rename the session id to `--sid` and `--sid` becomes refused while
`--session-id` becomes the user's to pass. `--continue` carries `Injection::SessionId` and not a
variant of its own — it is illegal *beside* an injected session id — while `--bare` and `--print`
carry `None` and stay refused whatever cide adds. `every_injected_flag_is_refused_and_every_injection_refusal_is_injected`
and `disabling_an_injection_relaxes_exactly_its_own_refusals` are the guards, and
`check-claude-cli.mjs` compares the `because` column across the two implementations the way it
already compares aliases and `takes_value`.

**A rename is validated by two rules, both of which discard rather than reject.** It must begin with
`-`, because `claude`'s first positional argument is a *prompt* and a bare `sid` would start every
pane by asking the model something; and it must not be another injection's spelling, defaults
included, because one flag written twice is the silent breakage this whole area is about. Either way
cide falls back to its own spelling and Rust's sentence appears under the row.

**The degraded argv shapes are the risk, and they are unit-tested.** `--resume <parent> --session-id
<minted>` without `--fork-session` is rejected outright by 2.1.227 — every Resume click failing
before a pane appears — so a fork whose fork flag is switched off falls back to the plain resume, and
a resume whose resume flag is off degrades to a fresh session rather than emitting a lone
`--fork-session`. **Those shapes have never been put in front of the real binary**: `tests/
real_session_args.rs` is `#[ignore]`d and drives the shipped defaults only.

**Not done.** Nothing here has been run against `opencode` or any other harness — the honest claim is
"cide stops adding flags", not "another harness works". The headless one-shot lane keeps its own
fixed argv (`-p --output-format json …`) and is deliberately not configurable, so *Generate commit
message* and *Explain selection* fail against a binary that is not Claude Code whatever is set here;
the screen says so. And no automated check forks a `claude` and reads a status bar, so "with
`--settings` off the pane loses exactly the four named features" is argued from where the hooks are
consumed, not observed.

## Ctrl+P can reach External Libraries (M16)

**⌥L while the picker is open — or the footer's `⌥L Libraries` chip — widens Ctrl+P to the source of
this project's resolved dependencies.** Off at every launch. This is **not implemented** rather than
stranded: every supporting piece existed and was reachable, but library files were candidates
nowhere, because `Index::build`'s sink is the only thing that ever fills the file matcher and it only
walks project roots.

### It cannot be a filter, and it is not one

The exclusion is a *structural* property, not a rule: `cide_fs::groups`'s header lists the four
couplings a grafted synthetic root would trip, and line 17 names this exact cost — *"the walk's sink
is the picker's injector | 593 crates' worth of files in Ctrl+P"*. **That decision is untouched.**
`Index` is not modified; library candidates go into a *second, opt-in matcher* on `ProjectFs`, so
`dir_paths()` (the watcher's list), `Filter::build`, `show_roots` and `path_of` are the project's
alone. "Never watched" stays true because there is still no code that could watch them.

### Measured on this repository

| | |
| --- | --- |
| external packages `cargo metadata --frozen` resolves | **593** (0.21 s, 3.1 MB of JSON) |
| files under them, with cide's exact ignore settings | **31,865** (36,643 entries including directories) |
| the project's own index | 875 files |
| walk, 593 roots at `threads: 1`, warm | **130 ms** |
| the same loop at `BuildOptions::default()` (`threads: 0`) | **1.24 s** |
| the whole of `~/.cargo/registry/src` | 127,055 files |
| `~/go/pkg/mod` | 347,777 files |

The last two rows are the point: *index the libraries* and *index everything this machine has ever
built* differ by two orders of magnitude, and only the first is bounded by the project the user has
open. The `threads: 0` row is the trap — the cost is not the walking, it is spawning and joining 32
walker threads 593 times over directories that hold 60 entries each — and it has a source assertion in
`check:picker`, because nothing else in the suite can see a 20× slowdown.

### Lazily, and visibly

Nothing is walked until the scope is switched on for the first time in a process. `picker_index_
libraries` claims the walk and returns; the overlay watches it fill through the poll it already runs,
so `running` stays true and the counter climbs — the same shape `symbols_index` and `symbol_query`
have had since M12. If nobody has opened *External Libraries*, the walk asks
`ProjectGroups::resolve_now` for the packages, blocking on `cargo metadata` on a pool thread: the
same call `fs_reveal` already makes for *Select Opened File*, on the same argument — the gesture
explicitly asked for this, it happens at most once per project per process, and the alternative is
not "faster", it is an empty answer.

A `cargo add` or `cargo update` invalidates it. The lockfile stamp already notices (it is a stamp and
not a watcher subscription because `Cargo.lock` is gitignored in plenty of repositories and no
`cide://fs-changed` would ever mention it), and that path now calls `forget_libraries` *before*
spawning the resolver — the stale set is wrong now, and the resolver runs for a quarter of a second
during which Ctrl+P would otherwise keep offering it. It does not re-walk; the next query does.

### Two matchers and a merge

Forced, for the reason `symbol_matcher` is — `Matcher::query` holds one query per matcher — plus one
specific to this: **nucleo is append-only**, so a toggle-off could not un-inject 32,000 candidates,
and post-filtering a 200-row frame would break both the row limit and the `6 of 2,418` counter, which
come from the snapshot.

Merging is exact and free. `Pattern::indices` **returns the score** and `frame()` was already calling
it for the highlight offsets and throwing the return value away; `frame_scored` stopped throwing it
away. The rows are already sorted within each side, so it is a two-finger merge, `matched`/`total` are
sums, and `running` is the OR. **Ties go to the project**, and that decides the common case rather
than an edge one: an empty query scores every row identically, and `lib.rs` typed in full matches this
project's and `serde`'s identically too.

### Telling the rows apart

A library row draws `serde-1.0.229/src/lib.rs` — the package directory prefixed for free by
`Visitor::item`, so a user can narrow to one crate by typing its name — and a right-aligned dim chip
carrying `serde 1.0.229`. That is one nullable `PickerRow.source` and not a flag beside a label, so a
row can never be marked as a library with nothing to show for it. The version is in the chip and not
in the matched column: it is a label you read to tell two rows apart, not a thing you search for, and
in the haystack it would make every crate's files match every other crate's version digits. `.source`
is `flex: none` and `.path` is `flex: 1`, so the path ellipsises first — a path is reconstructible
from a name and a package, and a package name cut in half is not.

### The chord, and why it is a command

`⌥L` is bound in `cide_core::keymap` under a new `filePickerOpen` context flag, not handled locally
in the overlay. A local `if (ev.altKey && ev.key === 'l')` would be unrebindable, unlisted and
undiscoverable — the shape that produces this project's recurring defect. The clause is
`filePickerOpen` and **not** `overlayOpen`, which is true for any of nine overlays: the gate is a
window *capture* listener, and ⌥L is `ESC l` to a shell (readline's `downcase-word`), so the wider
clause would have taken it from every terminal pane in every window whenever a menu happened to be
up. `ctrl+alt+l` was the obvious alternative and is KDE's Lock Screen on a stock install, so on the
development platform it never reaches the app.

The footer chip is an `aria-pressed` button (the app's precedent is `SearchPanel`'s; nothing in the
overlay language uses a checkbox) that commits on `mousedown` with `preventDefault` — a click blurs
the input between the two events and `ModalShell`'s `selectionchange` listener repaints the caret,
after which typing stops working. **One element documents the chord and is the mouse target.**

The scope is window-session state in the overlay store: it survives closing and reopening the picker
within a run, and resets on relaunch, so "disabled by default" stays true in the sense a user checks
it. Persisting it would make that true exactly once in a user's life and would silently change what
Ctrl+P costs on every project thereafter.

**Not done.** No end-to-end confirmation on screen — the merge, the chip and the toggle are asserted
in Rust and in `check:picker`, and nothing in `ui/scripts/` mounts React against a live backend. Go
projects are **untested at this scale**: `go list -m all` bounds the set the same way, but a Go
module's directory is the whole module rather than a package, and the numbers above are cargo's alone.
`.a` and `.so` files and trybuild `.stderr` fixtures are indexed like everything else the walk admits
(2,829 `.a` files in this dependency set), because a second ignore rule beside `Filter` is a thing
this codebase resists on principle. The registry is machine-global and immutable, so two open projects
sharing 500 crates each pay their own 130 ms and ~12 MB; a process-global cache keyed by package
directory is the obvious later move. And `Matcher::dirty()` is still an orphan — declared,
implemented, and called by nothing in the workspace, not even a test. That is pre-existing and
unrelated, and it is recorded here because it was found on the way through.

## Speed search in both sidebar trees (M15)

**Type into the explorer or the changes tree and it filters to rows whose name contains what you
typed.** Up and Down walk the matches in tree order, Enter opens the one you are on, Escape leaves,
and two seconds of silence clears it. A small box at the top-left of the panel shows the query and
`3 of 17`.

| key, while a query is armed | what it does |
| --- | --- |
| a letter, digit, `.`, `-`, `_` | extends the query — a bare keystroke *starts* one |
| Space | extends it, **once something has been typed**; a bare Space is still the changes tree's tick |
| ↑ ↓ | previous / next match, wrapping |
| Enter | opens the row and leaves |
| Escape, Backspace past the first character | leaves |
| Delete | **swallowed** — a bare Delete is *Move to Trash*, and a typo must not put a delete dialog on screen |
| ← → Home End, and every modified chord | leaves, then does exactly what it always did |

Nothing is taken from the keymap: `cide_core::keymap` binds no unmodified printable key, which
until M15 was a coincidence and is now `no_default_binds_an_unmodified_printable_key`. It had to
become an assertion, because the key gate resolves global bindings on a **window capture**
listener — a default of `r → git.pull` would eat the letter `r` in the file tree, the rename box,
the commit message, CodeMirror and every terminal, and speed search would go dead for that one
letter with nothing reporting it.

**One matching rule, in Rust, with two entry points.** `cide_fs::speed` decides what matches and
where; `fs_tree_match` walks the explorer's flattening and `tree_match_labels` takes a list the
changes tree supplies. That is the `picker_rank` shape rather than the `score.ts` shape, and one
fact decided it: the explorer's rows are not in the webview — it holds 200-row chunks of a
flattening Rust owns, so a match at row 40,000 exists only if Rust is the one looking. Once that
exists, a TypeScript copy for the other tree is a second answer to a settled question. The spans
come back in **UTF-16 code units**, because the only consumer is `String.prototype.slice`; a byte
offset agrees for ASCII and puts the highlight four characters early on any name with an emoji in
it. The *key* rule — which keystroke does what — is `ui/src/sidebar/speedSearch.ts`, pure and
import-free, driven row by row by `check:speed-search`.

**Only what is expanded is searched**, and the empty answer says so: `no match in the expanded
tree`, not `no match`. Expanding to reveal a match would re-flatten the tree and renumber every
index in the list being built, so each keystroke would invalidate its own results — and Ctrl+P
already searches the whole repository including collapsed directories. A capped list says `first
1000 matches` rather than answering short.

**Not done.** There is no way to reach speed search from the palette or a menu — typing *is* the
gesture, and nothing on screen advertises it. Matching is substring, not fuzzy, and not ranked. A
match list is dropped and re-issued when the rows move underneath it, so a burst mid-query costs
one round trip and a frame with no highlight.

## The Find usages popup, and the scratch type picker (M15)

**"It doesn't see where is filename and where is code."** The Find usages popup drew its file
headings with `.path` and its source lines with `.usageText`: same family, same weight, same colour
token, and **half a pixel** of font size between them — with the heading being the *smaller* of the
two. Three more defects were in the same three lines: `.path` ellipsises at the tail, and the tail
of `crates/cide-lsp/src/progress.rs` is the filename; `.group` is `flex: 1`, so a two-character hit
count claimed half a 620px card; and `.usageFile`'s `background: var(--chrome)` is exactly the
card's own ground, so the one rule written to make the heading stand out painted it the colour it
already was. The heading is now the UI face at 12.5px in `--w-bold`, front-truncated, with the
file's icon beside it — the search panel's treatment, because the two lists show the same thing.
Go to symbol had the same defect in a smaller form (two adjacent `.path` spans reading as one grey
run) and is fixed in the same pass.

**The gate that missed it is the finding.** `check-theme.mjs` has a table asserting exactly this —
"a name is the UI face, code is the mono face" — over three stylesheets, and
`src/overlays/Overlay.module.css` was not one of them. So the one overlay that grew a
heading-plus-source-line list in M14 was never swept. It is in the table now, plus assertions that
the heading is *larger* than the source line and carries a weight it does not.

**SQL is a real grammar, not a row.** `ui/src/editor/languages/sql.ts` is a `streamGrammar` data
module: `--` line comments, case-insensitive keywords (the flag exists for this one language), a
doubled quote rather than a backslash escape, and `capitalisedIsType`/`callSyntax` both **off**,
because `Users` is a table and `Sessions (` is a table followed by a bracket. The last of those was
found by the check rather than reasoned about — with `callSyntax` on, every table in a schema was
drawn as a function. There is no `@codemirror/lang-sql` in this project and adding one is the
100 KB-of-parser-tables decision `streamGrammar.ts` exists to avoid; `check:editor` would have
failed a bare `{ label: 'SQL', ext: 'sql' }` outright, which is the gate doing its job.

**The scratch picker has a filter field, and it is focused on the first frame.** `useLayoutEffect`,
not `useEffect` — `ModalShell` records the reason and it is a bug this project has already shipped:
the overlay is opened by a keystroke, and a frame in which the field is not yet focused is a frame
in which the next character goes to whatever had focus before, which is a terminal. The ranking is
`filterScratchTypes` in `editor/languages.ts` beside the list it filters (exact extension, then
extension prefix, then label prefix, then label substring), so `sql`+⏎ is the whole gesture. A query
that matches nothing says `No type matches ‘xyz’` and Enter does nothing — deliberately not
creating a scratch of the typed extension, which would be a free-text path into `check_ext` and a
separate decision.

## Autosave (M15)

**On by default.** A changed file is written when it loses focus, and after a minute with no edits.
*Settings ▸ Editor ▸ Save automatically*.

The signal is CodeMirror's own `focusChanged`, on the edge this listener never had, and one hook
covers everything: a click into another pane, into the file tree, into a terminal; a **tab switch**
(hidden tabs are `visibility: hidden`, never unmounted, so switching tabs is a blur); and the OS
window being deactivated, which is IDEA's frame-deactivation save arriving free. The idle timer is
a 60-second debounce **with a five-minute ceiling** measured from when the buffer went dirty,
because a restarting debounce is starved by continuous input — somebody typing steadily for twenty
minutes would otherwise never autosave, a trap `docSync.ts` and `gitCountStore.ts` had both already
written down.

**Every refusal, and how it is enforced.** All of them are `shouldAutosave` in
`ui/src/editor/autosave.ts` — pure, import-free, and driven as a truth table by `check:editor`,
because a feature that writes the user's files on a timer must not keep its rules inside a
`useEffect`.

| autosave refuses | because |
| --- | --- |
| the setting is off | and the toggle is *read*, which `check:editor` asserts — see below |
| the buffer is clean | a save is a `didSave`, which re-runs flycheck; otherwise every alt-tab is a `cargo check` |
| **the buffer is read-only** | External Libraries and toolchain sources; writing one corrupts a crate every project on the machine builds against |
| **the conflict bar is up** | it is the user's unanswered question, and autosave would answer it in the direction that discards whatever changed the file |
| **a Claude `openDiff` of this file is open** | `openDiff` blocks the agent's turn. Clicking the diff tab to look at it is a tab switch, is a blur, is a save underneath a proposal computed against the old bytes |
| focus is still inside the editor | the find bar is a CodeMirror panel inside `view.dom` |
| an overlay or context menu is open | Ctrl+P is not leaving the file |

Window deactivation is checked **before** the last two: `document.activeElement` does not move when
an OS window is deactivated, so testing "is focus still in the editor" first would veto the one
save this feature is best known for. An explicit Ctrl+S ignores all of it — that is the user
deciding.

**A failed autosave produces a sentence.** `void diag.log(...)` writes to a file nobody opens, and
a save that happened on a timer while the user was looking at a browser is the one that most needs
saying out loud — this is also the population where writes actually fail, because a root-owned
mode-644 file reports `writable: true` (the mode bits, not "can *you* write it"). The notice
dedupes by text, and the timers are not re-armed after a failure, so a full disk gives one toast
rather than sixty. The tab stays dirty either way, which is what keeps the close confirmation in
front of the user.

**The `sed -i` hole is closed, for autosave.** The conflict bar is raised by `cide://session-tool`,
which a `cargo fmt`, a `sed -i` or a `git checkout` never sends. Before autosave, clobbering one of
those took a deliberate Ctrl+S; with autosave-on-blur it would take *switching to the terminal,
running `cargo fmt`, and clicking back* — three things nobody decides to do. So `FileDoc` now
carries a `FileStamp` (mtime + length) and a background write hands it back as `ifUnchanged`;
Rust refuses with a **tagged** `CoreError::FileChanged`, and the pane turns that into the same
conflict bar. Subscribing the editor to `cide://fs-changed` was the alternative and is worse: our
own write emits one, so it needs a self-write suppression window, which is a race with a timer in
it. What remains open is the microsecond between the `stat` and the `rename`, and an explicit
Ctrl+S, which still forces.

**The dirty dot is unchanged**, and deliberately: its meaning is "this buffer differs from disk",
which stays exactly true, and it is load-bearing rather than decorative — `tab_set_dirty` is what
makes `close_tab` refuse without `force` and what `app_quit_requested` reports by name. There is no
save-on-quit either. The honest consequence is that the unsaved-at-quit dialog becomes rare, which
is the feature working; the guard stays for the cases autosave refuses, which are precisely the
cases where the user most needs to be asked.

**Migration is explicit.** `Workspace::CURRENT_SCHEMA` moves to **3** and `persist::v2_to_v3`
*writes* `settings.editor.autosave = true` into every existing document. `#[serde(default)]` would
have loaded a schema-2 file perfectly well and given the same behaviour — deleting the migration
changes nothing today. It is there for the day the default moves: "make it opt-in" is the obvious
next request, and a defaulted field would silently turn autosave *off* for everyone relying on it,
with nothing on their disk to explain it. Same argument as 1 → 2's proxy scope, applied to a
setting whose blast radius is the contents of files. A value the user already chose is left alone.

### Finding: five of the seven editor settings are wired to nothing

`tabSize`, `insertSpaces`, `showMinimap`, `wordWrap` and `trimTrailingWhitespaceOnSave` all
persist, survive a relaunch, and change nothing — `EditorSurface` hardcodes `tabSize.of(4)`,
`indentUnit.of('    ')`, an unconditional `minimap()` and an unconditional `lineWrapping`, and
nothing anywhere reads the trim flag. `tsc --noEmit`, `codegen --check` and `contract-check` are
all green over that, because every one of them is a *field nobody reads*, which no existing gate
can see. **Not fixed in M15.** The direct lesson was applied instead: the autosave toggle ships
with an assertion in `check:editor` that it is read, not merely offered.

## Three things that shipped and did not work (M15), and why no gate saw it

Each of these was built, verified and reported broken by the user. In every case the mechanism was
correct and something outside it — a dependency, a child process, an absent population — decided
the outcome. The second half of each entry is the more useful one.

**The mouse's thumb buttons did nothing, because wry got the press first.**
`wry-0.55.1/src/webkitgtk/synthetic_mouse_events.rs` connects its own `button-press-event` handler
to the WebKitWebView inside `WebviewWindowBuilder::build()`; for buttons 8 and 9 it returns
`Propagation::Stop` and spends them on `window.history.back()`, which in an SPA with one history
entry is a silent no-op. GTK3's `button-press-event` uses the true-handled accumulator, so the
*first* handler returning TRUE ends the emission — and `install_mouse_nav` runs after `build()` and
defers itself onto the GTK loop besides, so it was always second and **never ran**. The handler now
goes on the generic `event` signal, which `gtk_widget_event_internal` emits before any specific one
whatever the connection order (measured against the real GTK 3, both directions). The window-label
filter was checked first and exonerated: both sides derive from the same `label.as_str()`.
*The comments in `windows.rs` and `emit.rs` claiming WebKit flattens both buttons to `button === 0`
were false and are corrected in place* — a DOM-only implementation was possible all along, and the
wrong premise is what stopped anyone looking at wry. **No gate saw it** because every gate tested
one link of a five-link chain and the broken link was the dependency's: `check:keys` starts one
call downstream of the whole broken segment, `keymap.rs` proves the binding resolves,
`contract-check` records the event's *name*. Nothing asserted that a GDK button reaches a
`Decision`. The new gate is structural — the signal cide connects to, over comment-stripped source,
with the whole diagnosis in its failure message — plus `nav_action` extracted from the closure so
the press rules are drivable at all. A refused Back also went to `diag.log` and now goes to
`notify`, so an empty history can be told from a dead button; that indistinguishability is most of
why this took a milestone to notice.

**A ctrl+click on a path in a Claude pane opened the desktop file manager on the parent folder.**
Not cide's matcher — `(` and `)` are not body characters, so `Update(/home/…/Foo.tsx)` yields the
*file*, verified for every shape Claude Code prints. The gate returned early when nothing had
hovered yet, xterm's own always-on `mousedown` then wrote an SGR mouse report to the pty, and
`claude` answered it with `dbus-send … org.freedesktop.FileManager1.ShowItems`. In a Claude pane
the early return is the common case, not the exotic one: the alt-screen TUI repaints under a
stationary pointer, so no `mousemove` fires and no hover is ever recorded. The press is now claimed
**unconditionally** and resolved against the buffer afterwards, which also fixes the mirror-image
bug in the same three lines (a *stale* hover surviving a repaint). Claude Code stands down for
xterm.js hosts — `xtversionName?.startsWith("xterm.js")` — and cide failed that test only because
`@xterm/xterm` 6 implements no XTVERSION at all; it now answers `DCS > | xterm.js(6.0.0) ST`
truthfully, which is the only thing covering **alt+click**, a chord the CLI also claims and cide
deliberately does not. **No gate saw it** because `check-paths.mjs`'s own pin — *"a path parsed out
of terminal bytes reaches a workspace tab and nothing else — not the desktop opener"* — was true of
cide's source and false of the user's screen: a grep over cide cannot see a file manager opened by
a program cide forwarded the click to. *Anything cide does not swallow is a gesture cide has
delegated.* The rules moved into `ui/src/terminal/clickGate.ts`, whose `pressVerdict` is **not
given the hover state**, so the early return cannot come back; and the old pin turned out to name
only Rust command strings that could never appear in a frontend module, which a mutation found.

**Select opened file said "not in this project's file tree" about a `std` file on screen.** Nothing
was broken: the rustup sysroot is a population no part of cide knew existed. `cargo metadata`
reports the `Cargo.lock` graph — measured here, 547 packages, none of them `std`/`core`/`alloc` —
so there was no row for a reveal to find and `is_unlisted_library_path` never even asked the group
to resolve. The **second, unreported** half is worse: `…/library/core/src/option.rs` is mode 644 and
user-owned, so `writable` was true and Ctrl+S wrote into the toolchain every project on the machine
compiles against — bit for bit the bug the read-only rule was written for. `dependency_roots()` now
covers the whole `~/.rustup/toolchains` directory (textual, no toolchain-name resolution, no
syscall), and `cide-deps::sdk` adds one SDK row per unit from `rustc --print sysroot` with the
project's cwd — which is the *only* honest way to get the toolchain rustup would pick, and this
repo proves it: `rust-toolchain.toml` pins 1.92.0 while the machine's default is `stable`. **No
gate saw it** because both existing tests were self-referential: the end-to-end one is `#[ignore]`d
*and* builds its target out of `dependency_roots()`, and the predicate test took `caches.first()` —
neither can notice a *missing* root. The new ones name the rustup path shape from the outside and
sweep every root. One more gap was found by mutation while writing this: deleting `sdk::probe`'s
single call site left every gate green, which is this project's recurring defect appearing inside
the batch that was fixing three instances of it.

**Not done.** None of the three has been confirmed on screen (KDE will not raise a shell-launched
window); `CIDE_INPUT_PROBE=1 ./run.sh` is the one-command runtime confirmation for the thumb buttons
and must now print `button=8` on a press. The GTK signal choice is held by a source assertion, not
by a synthesized GDK event — an `--audit-windows` leg that injects one through `gtk_main_do_event`
and asserts `cide://mouse-nav` arrives is the check that would have caught the original bug for
real, and it would also be the first execution of `client.ts`'s label filter in any test. Go's
standard library stays **writable** unless `$GOROOT` is exported: deriving it needs
`canonicalize(which("go"))`, and `cide_core::toolchain` forks and `stat`s nothing on the per-open
path. A directory ctrl+clicked in a *detached pane* is offered no link at all, because that window
has no file tree.

## Switching, and the panel toggle (M14), and what is not done

Four keys, and one of them is a chord that moved.

| key | what it does |
| --- | --- |
| **Ctrl+Tab** / **Ctrl+Shift+Tab** | walk the active project's **tabs**, most-recently-used |
| **Ctrl+`** | walk this window's **projects**, most-recently-used (was Ctrl+Tab) |
| **Ctrl+1** | go to the pinned Claude console |
| **F4** | hide the left panel, or bring back the last one that was open |

**The two switchers are one implementation.** `ui/src/keys/switcher.ts` is the arithmetic over
opaque id strings — hold the modifier, press the key to walk, release to commit, Escape to cancel —
and `ui/src/keys/switcherStore.ts` holds *one* open walk, *one* capture-phase keyup latch and a
`SwitchTarget` that says what is being walked. That is not tidiness: the key gate has exactly one
`capture` hook, so a second store would give two walks of which only one could ever be consulted,
and the loser's popup would be immortal — swallowing nothing, closing never, while the winner ate
Tab for the rest of the session. `check:switcher` counts the `keyup` registrations and fails at two,
and it runs its whole scenario table twice, once per walk key.

**Ctrl+` is bound as `backquote`, not as the literal tilde**, although the tilde is what was asked
for. `ui/src/keys/chords.ts` derives a stroke from `KeyboardEvent.code`, and the key left of `1` is
`Backquote` on every layout whatever `key` reports — so a binding spelled `ctrl+~` would normalise to
a key no keystroke can produce and would be silently inert. `~` has been added to the alias table, so
a `keymap.json` that writes it still means the chord. Two more reasons the literal is wrong: it needs
Shift, so the whole walk would be a Ctrl+Shift hold, and the capture reads Shift as the walk's
*direction*. **`project.switcher.prev` ships unbound** — `ctrl+shift+`` is `terminal.splitBelow` —
and is reached from the palette, or by holding Shift while the popup is up.

**The tab order is a real MRU stack and it survives a restart.** There was no per-tab focus history
anywhere: `Tab` has no timestamp, `p.tabs` is insertion order, and `close_tab` picked the *left
neighbour*. M14 derived one per project in `ui/src/store/workspace.ts` from every
`cide://workspace-changed` snapshot and mirrored it to `localStorage` under `cide.tabMru`, with a
note in the code admitting that the argument for keeping it out of Rust was weak. **In M15 it moved
into `Project::tab_mru` and the `localStorage` key is gone** — see the M15 section above for what
settled it. The list contains **tabs**, console and settings included, which is the point: one
Ctrl+Tab from a file gets you back to the conversation about it.

**What Ctrl+Tab costs a user who already rebound it.** Nothing, and no migration: their
`{"key":"ctrl+tab","command":"project.switcher.next"}` has no `when` and the new default has one, so
the two coexist and the *last applicable* binding — theirs — wins everywhere. The one case that
changes is an **unbind**: `{"key":"ctrl+tab","command":"-project.switcher.next"}` now matches
nothing, is reported as `RemovalMatchedNothing` in Settings → Keymap, and Ctrl+Tab starts running
the tab switcher. `ctrl_tab_switches_tabs_and_an_old_keymap_json_still_wins` asserts both.

**F4 was the sixteenth thing built and reachable from nothing.** Hiding the sidebar has worked since
M3 — clicking the lit rail button does it — with no command, no binding and no palette row; the
mouse was the only way in and the only way back out. The new fact is *which panel comes back*, and
it lives in `ui/src/chrome/sidebarView.ts`, import-free so `check:sidebar` can execute it. The
settings view counts as **closed** (⚙ opens a workspace tab and has no panel), so F4 with it lit
opens the last real panel; `last` is never `settings`, or "bring back the last panel" would bring
back a state with nothing in it.

**It is not remembered across a restart, deliberately.** Every launch still opens on Files with the
panel showing, bit for bit as before. `Settings` is global and rides every snapshot to *every*
window, so a persisted "hidden" would mean F4 in one window closing another's sidebar in
`perProject` mode; and a Rust-owned flag arrives an IPC round trip after first paint, so the panel
would render and then be yanked away — the exact jump `chrome/sidebarWidth.ts` writes thirty lines
about avoiding. The two widths beside it stay persisted.

**What these keys cost, named rather than discovered.** All five bindings carry `when: shellWindow`,
so a detached pane or tab window keeps them — that is a small improvement on the old unconditional
`ctrl+tab`. In the shell window they are unconditional and the gate is a window *capture* listener,
so:

* **F4** takes `ESC O S` from every terminal pane. That is a real key to `mc` (Edit), `htop`
  (Filter) and any curses TUI. `{"key":"f4","command":"-sidebar.toggle","when":"shellWindow"}` in
  `~/.config/cide/keymap.json` gives it back — the `when` is not optional on that line, and the test
  named after it says why.
* **Ctrl+`** costs the pty nothing (xterm encodes no control byte for keyCode 192) and CodeMirror
  nothing. It costs legibility: Ctrl+` switches project and Ctrl+Shift+` splits a terminal below,
  one Shift apart, and it is VS Code's toggle-terminal chord.
* **Ctrl+1** costs nothing either — keyCode 49 is not in xterm's plain-Ctrl encode set.
* **Ctrl+2..9 are not shipped.** They are the browser convention and were not asked for; `tabs[1..]`
  are positional and shift under the user, where `tabs[0]` is an identity Rust enforces; and
  Ctrl+3..8 are *not* free, encoding ESC, FS, GS, RS, US and DEL. IDEA uses Alt+1..9 for tool
  windows and has no Ctrl+digit tab selection at all.

**Not done.** None of it has been confirmed on screen — the gesture needs a held modifier, KDE will
not raise a shell-launched window and the Wayland compositor exposes no capture protocol; the
coverage is `check:switcher`, `check:keys`, `check:sidebar` and `check:commands`. `tab.next` /
`tab.prev` still have the latent issue the new commands avoid: they carry no `shellWindow` clause,
so from a detached-tab window they would move the *shell* window's active tab. The palette's key
chip prints `f4` lowercase, because `chipLabel` upper-cases single characters only and has no f-key
glyph — `ctrl+f12` has always rendered `⌃f12`.

## Editing the keymap from Settings (M14), and what is not done

Settings → Keymap could read the keymap and nothing else. There was no write path anywhere in
the workspace — `keymap_report` was the only keymap command on the wire, and the only non-test
writer of `keymap.json` in `crates/` was a test helper. Genuinely absent, not the usual "built and
reachable from nothing".

**Every command has a row now, not every binding.** The screen used to map `report.bindings`, the
*resolved table*: 32 rows for a registry of 63 commands, so half the app was unbindable
from the screen whose job is binding — not disabled, not explained, simply absent. Rows are built
from `Bootstrap.commands` left-joined onto the bindings, in `ui/src/settings/keymapModel.ts`, which
is import-free so `check:keymap` can compile it and drive every rule.

**The file is still a diff, and that is the whole design.** An edit names a command, a context and
a key; `cide_core::keymap::apply_edit` works out the entries. Saving what the screen shows would
write every shipped binding into the user's file and freeze out every future change to a default
for anyone who had ever touched a shortcut. One gesture is routinely two entries:

```json
[{"key":"ctrl+p","command":"-picker.files"},
 {"key":"ctrl+shift+o","command":"picker.files"}]
```

…and the removal has to carry the `when` of the binding it removes, or it matches nothing and
reports `RemovalMatchedNothing` to a user looking at a screen that says the key is now free. The
`when` is therefore **copied from the row**, never re-derived, and never from `Command::when` — that
one gates the palette, and writing it into a binding would silently scope a chord the user asked to
be unconditional. Unbinding F4 produces exactly the one-liner this README prints two sections up,
`when` included; there is a test named after that.

**Restore default deletes, it never writes the default in.** Both halves of the rebind above go, and
the compiled-in binding reappears underneath. Rebinding a command back onto its own default chord
therefore empties the file rather than filling it.

**Conflicts warn; they never block.** `keymap.rs` states the policy — which of two commands should
win is a judgement only the user can make — and blocking would stop a user halfway through a
legitimate two-step edit. The box says what holds the chord, and *which kind* of collision it is:
over a **default** the new binding silently shadows it and nothing will be reported afterwards; over
another **user** entry both survive and the conflict banner will appear. It offers "bind and unbind
the other" as one atomic edit, because writing that removal by hand is the part a user cannot get
right. It also catches the two silent ones — binding `ctrl+k` when `ctrl+k ctrl+s` exists (the new
binding could never fire) and the reverse.

The preview is the one place the frontend knows something Rust does not. `normalize_key` renames no
keys by design, so `` ctrl+` `` and `ctrl+backquote` are two keys to it; `ui/src/keys/chords.ts`
folds them, and `clashesFor` compares through it. `conflicts()` still cannot see that pair.

**Recording a chord means owning the keyboard.** The key gate is a window *capture* listener, so a
dialog with its own `keydown` would never see Ctrl+P — the gate would have opened the file picker on
top of the box asking for a shortcut. So the recorder is the gate's `capture`, ranked above the
keymap, and while it is armed **every** stroke is consumed. `check:keys` sweeps the whole
modifier × key space a third time with it armed and asserts no command is ever dispatched.

What that costs, named rather than discovered: **bare Enter saves and bare Escape cancels**, so
those two spellings cannot be recorded from this screen (VS Code makes the same trade). Every
modified spelling still can, and a *bare* Enter or Escape is a binding this app must not have
anyway — the capture gate would swallow it in every text field and every terminal. A fumbled second
chord **replaces**; a two-stroke sequence is asked for with the "+ second stroke" button, so
`⌃⇧P ⌃⇧O` is never what a mistake produces. There is no live "⌃⇧…" preview while modifiers are
still held: bare modifiers never reach the capture, and the only way to get one would be a second
keydown/keyup listener beside the gate.

**Two-stroke sequences have never run in production and now can.** `defaults()` contains no
multi-stroke binding — `ctrl+k ctrl+s` appears only in a doc comment and a test fixture — so the
prefix machine has only ever been exercised by `check:keys`. It stays that way by default:
**`settings.keymap` is deliberately still unbound.** Binding it to `ctrl+k ctrl+s` would arm a
prefix on Ctrl+K globally, and Ctrl+K is readline's kill-to-end-of-line in every terminal pane in
every window. The recorder is what makes sequences reachable without cide taking that from
everybody.

**What happens to a `keymap.json` somebody wrote by hand.** Saying it plainly, because half of it is
a loss:

* **Comments were never legal and are not lost by this.** `load_user` is `serde_json::from_str` —
  strict JSON, not JSONC — so a `//` comment has always made the *whole file* fail to parse, and a
  file that fails to parse contributes **zero** overrides. That used to be a `tracing::error!`
  nobody reads; it is now a problem line on the screen that says the file is strict JSON.
* **A file that does not parse is never rewritten.** The edit command refuses and names the file.
  The one exception is **Reset all**, alone, which is the gesture that already means "throw away
  what is in there" and is the way out for someone who does not want to go and find it.
* **Formatting is lost. Entry order is kept.** The vector is re-serialised with `to_vec_pretty`, so
  indentation and the order of fields inside an entry become serde's. Order *between* entries is
  semantic — `apply_layer` walks the file in sequence and a removal only matches what is already
  accumulated — so the file is mutated in place and never rebuilt from the resolved table.
* **Unknown fields are dropped.** `{"key":…,"command":…,"note":"why I did this"}` parses today and
  the `note` is gone on the first rewrite. Nothing warns, because nothing parsed it.
* **The previous contents are kept as `keymap.json.bak`, on every write**, through the same
  `write_atomic` as the file itself. That is the answer to the two points above rather than an
  apology for them, and the screen says so under the table.
* **The mode becomes 0600** (`write_atomic` publishes by renaming a private temp file) and **a
  symlink is followed rather than replaced** — `keymap.json` symlinked into a dotfiles repository is
  how a hand-authored one usually arrives, and a plain rename would strand the repository copy.

**Every window is told.** A new `cide://keymap-changed` carries the resolved table, because
`applySnapshot` builds `{ ...current, workspace }` and keeps the old `keymap` — so
`cide://workspace-changed`, the one thing already broadcast to every window, is precisely the path
that cannot refresh a binding. The array identity matters as much as its contents: `gate.ts` caches
its indexed keymap against what `bindings()` returns.

**Not done.** None of it has been confirmed on screen; the coverage is `check:keymap`, `check:keys`,
`check:switcher` and the Rust suite, plus `./target/debug/cide-headless keymap` for the file itself.
The recorder caps a sequence at two strokes (a UI cap — the format has no limit). A command bound to
two chords in the *same* context is representable by hand and the screen shows two lines for it, but
*Restore default* on either line deletes both, because a reset matches on (command, `when`). Rust's
`conflicts()` still cannot see that `` ctrl+` `` and `ctrl+backquote` are one key; only the
pre-save preview can. There is no undo beyond `keymap.json.bak` and *Restore default*.

## Ctrl+hover and Find usages (M14), and what is not done

Hold Ctrl over an identifier and it underlines; click it and you go to the declaration — or, when
you are already **on** the declaration, you get its usages. IDEA's arrangement. `Alt+F7` and a
*Find usages* item in the code pane's context menu run the search unconditionally, from a reference
as happily as from a declaration.

**The underline is a promise about what the click will do**, so the two are one gesture and are kept
in agreement structurally rather than carefully. `ui/src/editor/codeIntel.ts::resolveWord` is the
only caller of `diagnostics_probe` in the app and both gestures go through it; both normalise the
pointer to the same word range, so both compute the same cache key and hit the same entry; both ask
`intent()` what the answer means, and `underlines()` is *derived* from `intent()` rather than being
a second list. Both handlers live in one file, `ctrlLink.ts`, on one CodeMirror extension — the
Ctrl+click that used to sit in `EditorSurface.tsx` moved there for exactly that reason. `check:editor`
counts the probe call sites and asserts the other three files never reach past it.

**The discriminator is "the definition is where I already am."** Ask `textDocument/definition` —
the request Ctrl+click already made — and compare the reply against the position asked about: same
file, caret inside the returned range, means the caret *is* the declaration. VS Code's rule verbatim.
On a reference, the common case, it is **free**: it is the one request that was already being made.
Only on a declaration is anything extra paid, and there it is a definition round trip in front of a
references search that dwarfs it.

Wrong in each direction, because it will be:

* **"Declaration" when it was a reference** needs a server to answer a non-declaration with the
  caret's own position. Neither shipped server does that short of a bug. Cost: a usages popup
  instead of a jump. One Escape, nothing moved.
* **"Reference" when it was a declaration** is real and reachable — `fn fmt` inside
  `impl Display for Foo` resolves to the *trait's* `fn fmt`, so Ctrl+click on it jumps to the trait
  rather than listing implementations. Cost: a jump nobody asked for. Which is why the jump goes on
  the Back stack, and why `navigate.usages` ships as its own bindable command that skips the
  discriminator entirely. **That command is not optional**; it is the only honest answer to "the
  guess was wrong".

**What a drag costs, measured.** `hoverPlan` is a pure function precisely so the budget can be
driven headlessly, and `check:editor` replays a synthetic drag through it with a clock. One second
of pointer motion across a line of eight identifiers — 120 samples at 8 ms — is **zero requests**;
stopping is **one**; a there-and-back drag over six identifiers is **one**, not one per identifier,
because the cache is keyed on the *word range* and not the pointer position; and a pointer that
settles on a comment, a keyword or punctuation is **zero**, because the local CodeMirror token
answers that without asking anybody. A naive `mousemove → invoke` would be 60–120 requests a second.

That budget is not politeness. `diagnostics_probe` is `spawn_blocking`, so a blocking thread is
parked per in-flight hover, and the outbound queue to a language server is `bounded(256)` and
**drops notifications** when it is full — a hover flood would drop a `didChange` and desync
rust-analyzer's copy of the buffer permanently, which is the exact failure `docSync.ts` exists to
prevent.

**One usage jumps, with no popup**, through `jumpTo` so it lands on the Back stack. Zero usages is a
sentence in the notice stack (`info`, not an error — "used nowhere" is a *result*), and it is a
different sentence from "there is no declaration under the caret": `textDocument/references` answers
`null` for the second and `[]` for the first, and collapsing them would tell a user who clicked a
keyword that their function is unused. Two or more opens the 620px card after a 150 ms grace, so a
fast answer never flashes a popup and a slow one says *"Finding usages of ‘parse’…"* with
rust-analyzer's own progress detail appended, rather than appearing empty and reading as "none".

**Escape genuinely cancels.** `Requester::cancel` releases the blocked thread *and* sends
`$/cancelRequest`, so rust-analyzer stops searching rather than merely being ignored — without the
second half, "Escape cancelled it" and "Escape stopped showing it" are indistinguishable on screen
and only one is true. A timeout now cancels too, which fixes a small pre-existing leak on the
five-second definition path.

**Nothing gates the request on a source being `Ready`.** That is the trap this repository has
already shipped once: rust-analyzer's indexing is a *sequence* of progress tokens and "nothing in
flight" is true in every gap between them — against this workspace it flapped eight times in the
first 0.9 s — which is what `READY_SETTLE` exists for. The status is used for the popup's prose and
nothing else; the timeout is the readiness answer. `check:editor` asserts the orchestrator never
reads a readiness flag.

**Ctrl+B is unchanged.** `navigate.definition` still means Go to definition, exactly, and so does the
menu item that carries its chip. Only the *mouse* gesture is the smart one. Making Ctrl+B also
switch behaviour would have left a command called "Go to definition" that sometimes does not, which
is a worse trade than one chord that does one thing.

**What is not done.**

* **No preview pane.** IDEA's Find Usages tool window has one; here it would be a second CodeMirror
  instance inside a modal, for a surface that exists to be dismissed in under two seconds. The
  preview is the row: the source line, clipped in Rust, with the occurrence marked through the same
  `splitHighlight` the search panel uses.
* **The list is capped** at 200 usages across 100 files and says so when it hits the cap. The
  alternative is reading four thousand files because somebody Ctrl+clicked `fn new`.
* **No usages panel**, and no grouping controls: the popup does not collapse groups, because a fold
  state nothing can save is a control that only ever costs a click.
* **The hover needs a focused editor to react to a Ctrl *keydown*.** `mousemove` carries the
  modifier, so the ordinary case needs no keyboard listener at all — but a pointer parked over an
  *unfocused* pane before Ctrl goes down gets its underline on the first pixel of movement instead.
  Fixing it means a window-level keydown listener beside the key gate, and this feature does not
  need one.
* **`supports_references()` is dead weight today.** Both shipped servers advertise the capability,
  so the fifteen lines that read it only ever prevent a future lie — a server built without the
  method would otherwise spend the whole twenty-second deadline and then be reported as "probably
  still indexing".
* **Read-only buffers opened outside the project** (`~/.cargo/registry`, `$GOMODCACHE`) were never
  `didOpen`ed unless a pane mounted them, so what rust-analyzer answers about them is whatever its
  own index holds. Unverified either way.
* **`Requester::cancel` is not reachable for the probe path**, only for usages. A superseded hover is
  dropped on arrival by a generation counter and the server finishes the work — which is acceptable
  because the ladder means there is almost never more than one in flight.

## Dragging rows into a folder (M13), and what is not done

Press a row, move four pixels, and the file tree picks it up — the whole selection when the row is
part of it, that row alone when it is not. A drop is a **cut and paste**, and deliberately not a new
operation: `fs_paste` in `PasteMode::Cut` is what runs, so every refusal, the collision plan and the
cross-device fallback are the ones Ctrl+X/Ctrl+V already had. There is **no `fs_move`** and no Rust
change at all in this feature.

**A collision asks the same question the paste asks.** `fs_paste_plan` runs first and writes
nothing, so a drop onto a name that is taken raises `PasteConfirm` — the same dialog, the same
*Keep both* default, the same promise that nothing has been written yet. One hazard, one dialog:
two would eventually disagree, and the one that would go wrong is the drag, which has no undo
anywhere and no keystroke to repeat.

**Every refusal is a sentence on the ghost, before the release**, because a drag that simply does
not land is indistinguishable from a broken feature. The rules are `ui/src/sidebar/treeDrag.ts`,
pure and executed by `check:tree-drag`; the pointer state machine is `useTreeDrag.ts`, which nothing
in this repo can run. A project root is not moved; a dependency source under *External Libraries* is
neither dragged nor dropped into; the *Scratches* drawer is refused as a destination although Rust
would accept it, because *New File…* and *Paste* are already refused there; a group header is a
heading, not a folder; and a folder cannot be dropped into itself or into anything under it — the
one that destroys a directory tree if it is merely unlikely rather than refused. A **scratch** is
draggable, which is how one leaves the drawer for the project.

Feedback is four channels and none of it is polish: the rows in flight dim, the destination folder
takes an accent ring, the rows *inside* it take a tint (it is usually scrolled off the top by the
time the pointer is over its files), and a ghost under the pointer names the load and the verdict.
Folders spring open after 600 ms — except under a refused drop, which is what stops resting the
pointer over *External Libraries* from launching `cargo metadata`.

**Divergences and what is not done.** Pointer events, not HTML5 drag and drop, for the three
reasons in `useTreeDrag.ts`'s header — so there is **no drag into cide from Dolphin or Nautilus and
none out**, the same missing desktop-clipboard flavours the tree's Copy already documents. Dropping
on empty space is refused rather than meaning "the project root", because a pointer released over
the editor resolves to no row either. Nothing retargets an **open editor tab** when its file moves —
`fs_rename` and Cut+Paste have always had that gap and this does not widen it. And none of it has
been confirmed on screen: the gesture needs a pointer, KDE will not raise a shell-launched window,
and the Wayland compositor exposes no capture protocol.

## Project Notes (M17), and what is not done

**Project Notes** is one pinned row at the **top** of the file tree — above *External Libraries*
and above *Scratches* — that opens one markdown file per project in an ordinary editor tab. Double
click it, press Enter on it, right-click it and choose *Open*, or run *Open Project Notes*
(`file.projectNotes`) from the palette; all four go through the one command, which is why they
cannot come to behave in four ways.

**Where the file is.** `$XDG_STATE_HOME/cide/notes/<blake3 of the project's canonical first
root>/notes.md`, beside the scratch drawer and keyed identically — `cide_core::notes` reuses
`cide_core::scratch::key` rather than writing a third copy of the hashing rule. Not inside the
project, for the reasons a scratch is not, plus two that are specific to a pinned row: cide would
be creating an untracked file in somebody's repository on a single click, and the file would be
drawn **twice** (once as the pin, once as a walked row) or — once gitignored, since the walk is
gitignore-aware — as the pin and never as itself. A sibling `<key>.json` records which project the
directory belongs to. A multi-root project has **one** notes file, keyed by `roots[0]`, which is
the identity `RecentProject` and the scratch drawer already use.

**Nothing is created at project open.** The row is drawn from a constant label; the first click
runs `fs_notes_ensure`, which creates the directory and the file. That call is idempotent and uses
`create_new(true)`, so the second and hundredth clicks cannot truncate what the user has written —
that is the one line in the feature that could destroy data, and `cide_core::notes` has a test
whose only job is to prove it.

**The tab is an ordinary File tab**, deliberately and with no new tab kind: save, the dirty marker,
undo, find-in-file and the markdown grammar all work because nothing about it is special.
`file_read` and `file_write` apply no containment check — only the dependency-cache read-only rule,
which this path is not under — so the editor saves it.

**The row is not a file, and everything that assumes a row is a path excludes it.** It is a new
`TreeRowKind::Pin` whose `path` is the same `cide://group/<id>` sentinel a group header carries, so
it is not absolute and `cide_fs::ops::check_within` refuses it: rename, delete, drag, drop, *Copy
Path*, *Reveal*, the clipboard and the trash confirmation all decline it by rules that already
existed. `rowVerbs('pin')` grants `openable` and nothing else. The fs watcher never sees it — the
file is outside every root, so it is never walked, never watched, never a Ctrl+P or content-search
candidate.

**The write/rename asymmetry is deliberate.** The editor saves the file; the tree will not rename,
cut or trash it, because the notes directory is **not** in `ProjectGroups::writable_dirs`. A rename
would leave the pin pointing at nothing and the next click would create a second, empty `notes.md`
beside the user's real notes. Adding the directory to `writable_dirs` to "make it consistent"
re-opens that bug; the comment there says so.

**Divergences and what is not done.** The notes cannot be committed or shared with a team, and a
project that is moved or renamed gets a new key and appears to have lost its notes — the same cost
scratches, changelists and the shelf already pay, unmitigated beyond the breadcrumb. If
committable notes are ever wanted the honest shape is a setting with two values, not a change of
default: `cide_core::notes::file_for` is the only thing that decides the path. There is no default
binding (the palette and `keymap.json` are one step away), no garbage collection, and no watcher —
an edit made by another program is noticed by the editor's own conflict machinery on the next tab
activation, not by the tree. **And none of it has been confirmed on screen.** The coverage is
`cide_core::notes`, `cide_fs::groups`, `cide_app::notes` and `check:notes`; no check script can
mount CodeMirror, so that the tab really highlights markdown and that the row really draws the
markdown glyph above *External Libraries* are claims only a manual `./run.sh` can settle, and
neither audit covers this feature.

## Scratch files and Select opened file (M13), and what is not done

**Scratch files** (⇧⌥S, *New scratch file…*) are buffers that are not part of the project. They
live in `$XDG_STATE_HOME/cide/scratches/<blake3 of the project's canonical first root>/`, flat, one
drawer per project — the same keying `cide-git` uses for changelists and the shelf, and for the
same reasons. Not inside the project, because the walk is gitignore-aware and a scratch in the
working tree would either show up in `git status` or be hidden from cide's own file tree by the
user's own ignore file. A sibling `<key>.json` records which project a drawer belongs to, so an
orphan left by a moved project is findable by hand; **there is no garbage collection**, exactly as
for `repos/`.

Choosing the type first is what makes the file `scratch.rs` rather than an untitled buffer, and the
extension is the whole of how the editor decides the language. The offered list is
`SCRATCH_TYPES` in `ui/src/editor/languages.ts`, beside the table it must agree with; `check:editor`
asserts every offered extension resolves through that same table to that same label, so a type
cannot be offered that opens with no highlighting.

Scratches are a second synthetic group in the file tree, under *External Libraries*, and the only
one cide writes into: `ProjectFs::writable_paths()` is the roots **plus** the drawer, and it is
what the mutating `fs_*` handlers check against instead of the roots alone. So a scratch is saved,
renamed (which changes its language), cut, pasted and trashed like any other file. Nothing watches
the drawer — the group is re-listed by whatever changed it, which is also what emits the
`cide://fs-status` a second window redraws on.

**Select opened file** (⌃⇧E, and the ⌖ button in the Explorer header) scrolls the tree to the file
the active tab is about and selects it, for a project file, for a scratch, and for a dependency
source opened by Go to definition. That last case is the one that would otherwise be missing: the
*External Libraries* group holds no packages until somebody expands it, so `fs_reveal` resolves it
inline — once per project per process, gated on an I/O-free "is this path in a dependency cache"
test — rather than telling the user that a file they are looking at is not in the tree.

That gate is only as wide as `dependency_roots()`, which is how it kept saying exactly that for
**standard-library** files until M15: the rustup sysroot was in no root, no cache and no group. See
*Three things that shipped and did not work* above.

**Divergences and what is not done.** ⌃⇧E is *Recent Locations* in IDEA, where Select Opened File
has no default chord at all; cide has no Recent Locations, so nothing is lost, but the chord will
surprise somebody. ⇧⌥S costs every terminal pane the `ESC S` byte pair, unconditionally and in
every window (rebindable in one line of `keymap.json`). Scratches are **not** in Ctrl+P: the picker's
candidates come from the walk, and groups are not walked. *New File…* and *Paste* are refused
**inside** the drawer, deliberately — it is not a folder anybody organises, and the menu label would
be a blake3. A scratch renamed or deleted from one window reaches a second window's tree at once;
one *pasted* out of the drawer reaches it on that window's next refresh, because `fs_paste` has no
`AppHandle` to emit from. And none of this has been confirmed on screen — see the note under
*External Libraries* about the audits; the coverage is `cide_app::cmd::fs::scratch_tests`, which
drives the real command layer.

## External Libraries (M13), and what is not done

The file tree grows a second row source. `cide_fs::groups` is a **mechanism**, not a feature: a
group is a header row plus a lazily materialised subtree, drawn after the walked index's rows and
composed at the command layer (`cmd::fs::compose`). *External Libraries* is the first group;
Scratches is meant to be the second, and needs nothing new from that crate.

**Nothing happens when a project opens.** The first tree read runs a `has_marker` probe — a handful
of `stat`s to depth two — which decides only whether the header row exists. Resolution happens when
the user expands it: `cargo metadata --format-version 1 --frozen --manifest-path <m>` or
`go list -m -e -json all`, on a `cide-deps` thread of its own, with the group already showing
*Resolving dependencies…*. `--frozen` is the only cargo flag combination that resolves the full
graph **without writing `Cargo.lock`** — `--offline` was measured creating one — and go gets
`GOFLAGS=-mod=readonly` plus `GOPROXY=off`. Nothing under `~/.cargo/registry` or `$GOMODCACHE` is
ever walked, watched, indexed or offered to Ctrl+P.

**A group is never silently empty.** Every path ends in package rows or in a `Note` row the user
can read: `cargo` is not on PATH (with the install command), the lockfile is out of date (cargo's
own sentence, verbatim), a module go could not load (`-e` gives that per row), or *No external
dependencies*.

**The toolchain's own library is a row too (M15).** `Rust  1.92.0-x86_64-unknown-linux-gnu` and
`Go  go1.25.5` sort above the alphabetised crates, pointing at `<sysroot>/lib/rustlib/src/rust/
library` and `$GOROOT/src`. It is a `Package` with an `sdk` flag rather than a third group — a
group would need an id, a probe, state fields, a `STEMS` entry, an icon and a `view.*` command with
a dispatch case *or it is unreachable*, and IDEA puts its SDK node inside External Libraries too.
Neither resolver can report it (`cargo metadata` describes the lockfile graph; the Go standard
library is not a module), so it comes from `rustc --print sysroot` / `go env GOROOT` on the
resolution thread, independent of whether the dependency resolution itself succeeded — a stale
lockfile still gets a browsable `std` beside the sentence explaining its missing crates. **A
missing `rust-src` is an ordinary state, not an error**: the row stays, and says
`rustup component add rust-src`. `rust-toolchain.toml` joins the stamp, so editing the pin
re-resolves.

**Dependency sources are read-only, and that fixed a live bug.** Cargo unpacks a crate mode 644, so
before M13 a Go-to-definition into `serde` gave an editable buffer whose Ctrl+S wrote into the copy
every project on the machine builds against. `cide_core::toolchain::read_only_reason` now clears
`FileDoc::writable` for anything under a toolchain's dependency cache and `file_write` refuses it a
second time — unless the user opened that directory as a project root, which overrides. **M15 found
the same bug still live one directory over**: `~/.rustup/toolchains/…/library/core/src/option.rs` is
also mode 644 and user-owned, and the rustup tree was simply never enumerated, so a Ctrl+S in a
`core` buffer wrote into the toolchain `rustup update` then silently replaces. That directory is now
a root in full — sources, `bin/`, `lib/`, everything, since nothing under a rustup toolchain should
be written by an editor. `$GOROOT` joins it when it is set, which it usually is not; Go's std
therefore stays writable on most machines, and that is stated rather than hidden.

**And a read-only buffer could not be focused at all, which killed the entire editor keymap in
it.** (M16) Reported as *"Ctrl+F opens no find bar in a std-library file"*; the find bar was never
the problem. `findExtensions()` is unconditional in `EditorSurface`'s extension list, and
right-click **Code ▸ Find…** opened the bar in those buffers the whole time. The cause is one
missing attribute: `EditorView.editable.of(false)` gives `.cm-content` `contenteditable="false"`,
such an element with **no `tabindex` is not focusable**, and CodeMirror registers every DOM handler
— `keydown` included — on `contentDOM`. Events bubble up, so the listener never fired. Clicking the
buffer focused `.cm-scroller` (contentDOM's *parent*, which carries `tabIndex = -1`) instead, and
every `view.focus()` in the codebase was a no-op against it.

So it was never one chord. `Mod-f`, `F3`/`Shift-F3`, `Mod-d`, `Mod-Shift-l`, `Escape`, `Alt+Enter`
(*send lines to Claude*) and **the whole of `defaultKeymap`** — arrows, Home/End, PageUp/PageDown,
`Mod-a` — were dead in every library and toolchain buffer. It read as alive because `.cm-scroller`
is the focused overflowing element, so those keys still scrolled the pane; what was actually missing
was the **caret**, which the base theme hides outside `.cm-focused`. And `view.hasFocus` is
`activeElement == contentDOM`, so it was permanently false — which is why the update listener's two
slot re-claims never fired, and why Ctrl+G, Ctrl+F12, ⌥F7, Ctrl+B and Back all acted on whichever
*editable* file was touched last while the status bar went on naming it.

The fix is `EditorView.contentAttributes.of({ tabindex: '0' })` beside the `editable` facet, which
merges over the computed attributes and leaves `contenteditable="false"` alone. Dropping
`editable.of(false)` would also work and loses: it makes `.cm-content` a root editable element
again, which is exactly what `codeMenu.tsx`'s focus-restore note relies on read-only buffers not
being, and it puts an IME and a native caret in a document that cannot change. Binding `ctrl+f` in
`cide-core::keymap` fights `nothing_binds_the_find_bars_f_keys` and would have fixed one chord out
of twenty. `check:editor` asserts the pairing inside the `if (readOnly)` block, on
comments-stripped source, because that file now argues about tabindex at length.

**Still unmet:** the same shape exists on the `a` side of `panes/DiffPane.tsx`, where it is
*deliberate* — "a click does not place a caret in a document that cannot be changed" — so Ctrl+F in
a split diff still only works after clicking the right-hand side. That is a separate decision and
was not folded in here. And the end-to-end confirmation (open a toolchain path, click, assert
`document.activeElement` is `.cm-content` and `.cm-editor` carries `cm-focused`) needs a display and
belongs in the `CIDE_AUDIT_PANES=1` harness; no check script anywhere constructs an `EditorView`.

**Not done.** *Reveal in File Manager* is disabled for a row outside the project rather than
relaxing `fs_show_in_manager`'s containment check; the resolver has no timeout (`--frozen` and
`GOPROXY=off` make the network impossible, so the only unbounded wait left is cargo's own
package-cache lock); a project that gains its first `Cargo.toml` while open does not grow the group
until the next launch; and there is no default key binding — the palette's *Show External
Libraries* (`view.externalLibraries`) is the keyboard route — as is *Show Scratches*
(`view.scratches`) for the group below it, which is otherwise the last row of a virtualized tree.

## Language support (M12), and what is not done

Rust and Go get a tree-sitter symbol layer (`cide-lang`) and a language-server client
(`cide-lsp`). The honest state, because half of this is data with no surface on top of it yet:

**Works, and is checked.** `Ctrl+F12` (File Structure popup), `Ctrl+Alt+Shift+N` (Go to Symbol in
project), `Alt+Up`/`Alt+Down` (previous/next member), `Ctrl+G` (Go to line), per-file position
memory, the `getDiagnostics` MCP tool answering from a real store,
and the whole Rust pipeline underneath: extraction for both languages, a parallel project walk, the
merged diagnostic store, the LSP codec/session/supervisor.

**Keys into the editor, and what `Ctrl+G` cost.** Undo/redo (`Ctrl+Z`, `Ctrl+Y`, `Ctrl+Shift+Z`) is
CodeMirror's `historyKeymap` and is deliberately absent from `cide-core::keymap` — a binding there
is consumed by the key gate's *window capture* listener before any text surface sees it, and in a
terminal `Ctrl+Z` is SIGTSTP. Two tests keep it out. The find bar focuses its field on `Ctrl+F` and
bridges `F3`/`Shift+F3`/`Ctrl+G` into `search-panel` scope, so the keys work with the caret in the
box as well as in the buffer; the counter reads `3 of 12`, which is the only signal that the search
wrapped. `Ctrl+G` is Go to line, which **takes find-next away from that chord inside an editor** —
`F3` is find-next now, `Ctrl+Shift+G` is still find-previous, and one line of `keymap.json` —
`{"key":"ctrl+g","command":"-navigate.line","when":"editorFocused"}` — gives CodeMirror's `Mod-g`
back. The `when` is not optional in that line and the test named after it says why.

**And what `Alt+Up`/`Alt+Down` cost was a comment, not a compensation.** `keymap.rs` has claimed
since M12 that "`EditorSurface` re-homes `moveLineUp`/`moveLineDown` to `Mod-Shift-Arrow` — IDEA's
own chord for the same thing — which is free in both layers, so a capability moves rather than
disappearing." Both halves were false and it took until M16 to check: `grep moveLineUp ui/src`
returned nothing, so move-line-up/down had simply been gone from every buffer since the member walk
claimed Alt+Arrow — with no palette row and no menu item either; and `Mod-Shift-Arrow` is *not* free
on macOS, where `standardKeymap`'s `{ mac: 'Cmd-ArrowUp', shift: selectDocStart }` claims ⌘⇧↑. The
binding exists now, spelled `Mod-Shift-Arrow` off macOS and `Mod-Alt-Shift-Arrow` on it, and
`check:editor` **computes** the freedom of both — expanding the composed CodeMirror keymap the way
CodeMirror expands it, `shift:` sub-bindings and `mac:` overrides included — rather than trusting
the documentation, since trusting the documentation is what produced the sentence being replaced.
`check:keys` holds the other end: the re-homed chord is read out of `EditorSurface.tsx` and asserted
**unbound in `cide_core::keymap`**, on both layers, because the gate is a window capture listener
and a `ctrl+shift+up` added there next year would delete the capability a second time in exactly
the way it was deleted the first — with no conflict visible to either side alone.

**Still chord-only.** Move line up/down has a key and nothing else: no palette row, no *Code* menu
item. `moveLineUp` is a `@codemirror/commands` function, not a `cide-core::commands` id, and giving
it one means an id, a `when`, a dispatch arm and a route from `keys/dispatch.ts` into a specific
pane's `EditorView` — which is the seam `paneHosts.ts` exists to keep closed. Named here rather than
half-built, since naming a cost and then not paying it is the failure this whole section is about.

**Verified against real servers.** `cargo test -p cide-lsp -- --ignored` drives the real binaries;
**CI does not run it**, so run it by hand after touching that crate. All eleven pass — `gopls`
reporting on a module, rust-analyzer indexing this workspace and reporting an introduced type
error, the shutdown ladder actually stopping a server, the on-disk-edit test below, a real
`textDocument/definition` resolving a reference to its declaration, (M14) a real
`textDocument/references` listing the call site of a declaration and *not* the declaration itself,
and (M18) the four that back the section after next: the flycheck kick clearing a diagnostic an
on-disk edit alone would not, gopls re-diagnosing a file it was only *told* had changed, and
definition-versus-implementation giving two different answers at one caret in Go and the concrete
`impl` in Rust. Run against rust-analyzer 1.92.0 and gopls v0.21.0.

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

## Diagnostics that follow the disk, and a button to re-run them (M18)

Reported as *"rust-analyzer doesn't catch every change — after Claude fixes some hints I still see
them, and clicking one goes to some comments rather than the line, and I have no manual button to
re-run it in the Problems panel."* Three separate defects wearing one sentence.

**Nothing carried an out-of-editor write to a language server.** Document sync (above) covers the
buffer the *user* is typing in. It says nothing about the file an agent just rewrote in a tab
nobody opened — which, in an editor whose centre of gravity is a Claude session, is most of the
writes. The watcher's events reached the file tree, the picker and `cide-lang`'s symbol index and
stopped there. `FsEvents` now has a third method, `files_changed`, and `ProjectDiagnostics::
files_changed` decides what each server needs:

- **gopls gets `workspace/didChangeWatchedFiles`, at once.** It has no watcher of its own — it
  registers watch patterns through `client/registerCapability` and then waits — so this
  notification is how it learns anything the editor did not tell it. That also means the client
  capability has to be declared, and it now is, **for gopls only**.
- **rust-analyzer gets `rust-analyzer/runFlycheck`, debounced.** Deliberately *not* the
  `didChangeWatchedFiles` capability: declaring it transfers responsibility for file notifications
  to the client, and cide's watcher is gitignore-filtered and confined to the project's roots,
  where rust-analyzer's own watcher additionally sees path dependencies and `~/.cargo/registry`
  sources. Narrowing what it sees would trade a stale-diagnostics bug for a wrong-answer bug. Its
  problem was never file notification anyway — semantic analysis already followed the disk; it is
  *flycheck*, the `cargo check` behind every `E0308`, that only ran on a save. The kick is
  coalesced (500 ms trailing, 5 s ceiling) because a `cargo fmt` over four hundred files must
  produce one `cargo check` and not four hundred.

`an_on_disk_edit_alone_never_refreshes_diagnostics` is unchanged and still asserts the negative —
it sends nothing, so it stays true. `a_flycheck_kick_refreshes_diagnostics_an_on_disk_edit_alone_
would_not` is its exact inverse and is the only gate on the extension still existing: a
notification produces no reply, so a server that stopped implementing it would drop the kick in
silence.

**A stale row kept its old line number, and said nothing about it.** That is the "clicking it goes
to comments" half, and it is true for a window even with the above fixed. `Diagnostic` now carries
`stale`, set in `DiagnosticStore::snapshot` from a `(source, path)` dirty set the watcher marks and
a publish clears. Keyed by source and not by path alone, because tree-sitter re-parses the open
buffer on every keystroke while rust-analyzer's flycheck may be twenty seconds away, and a
path-keyed set would let the first un-mark the second's rows — under-reporting, which is the defect
itself. The row stays clickable and says *"changed since it was checked"*: a jump that may be a few
lines off beats a dead row, as long as it is labelled.

**The panel had no controls, and one of them already existed.** `diagnostics.restart` shipped in
M12 fully implemented, unit-tested, registered in `contract/commands.json` and wrapped in
`client.ts` — with **no caller anywhere in the app**. So did `snapshot.sources`: on the wire since
M12, converted by `adapt.ts` on every emit, rendered nowhere, which meant *"rust-analyzer is not on
PATH"* — the one sentence that explains an empty list — crossed the IPC boundary and was thrown
away. The panel now has a footer listing every analyser with its status, a **Restart** button on
the two that are processes (tree-sitter and Claude get none, because `restart` returns immediately
for them and a button that does nothing is the failure this panel exists to avoid), and a
**Re-run** button beside it. Re-run is the cheap one — seconds, no re-index — and is also
`problems.refresh` in the palette. **No default binding**: IDEA and VS Code ship none for their
equivalents, and a chord is taken from every terminal pane in every window.

## Go to implementation, which is a different question from Go to definition (M18)

Reported as *"goto for golang goes to the interface declaration, but should go to the
implementation."* gopls is answering correctly: `textDocument/definition` on a call through an
interface resolves to the interface's method, because that is where the callee is declared. cide
simply never asked the other question.

**Ctrl+Alt+B**, IDEA's own chord, on a new id `navigate.implementation`, scoped `editorFocused` for
the reason every other member of that group is — the key gate is a window *capture* listener, and
Ctrl+Alt+B is `ESC ^B` to a shell. Ctrl+B and Ctrl+click are untouched and still mean "definition".

Teaching Ctrl+B to try implementation first and fall back was the obvious fix and it regresses
Rust: rust-analyzer answers `textDocument/implementation` on a struct name with its `impl` blocks
and on a trait with its implementors, so Ctrl+click on an ordinary type name would stop opening the
declaration. Changing the most-used gesture in one language to fix a complaint in another is the
trade `navigate.usages` already refused once, for the same reason.

It reuses the Find usages popup wholesale — same 0/1/≥2 rule, and ≥2 is the *common* case here
because an interface with many implementors is the normal shape — with one difference and one
correction. The difference: **0 falls through to Go to definition**, so the command always does
something wherever a caret is. The correction: the popup's prose branches on which question was
asked, because telling a user who pressed Ctrl+Alt+B *"no usages of ‘Reader’ outside its
declaration"* claims their interface is unused, which is not what they asked and not true.

`gopls_goes_to_the_implementation_rather_than_the_interface` asserts **both** halves at one caret —
definition landing on the interface's method, implementation landing on the concrete one — because
a test that only asked the second would pass on a build where the separate command had become
pointless. It also caught something worth knowing: gopls answers `textDocument/implementation` on a
position that is not a type with a JSON-RPC **error** (*"s is a var, not a type"*) rather than with
`null`, so `RequestError::Failed` is folded into `NotFound` — the server's words go to the log, and
the user gets the definition they were reaching for instead of a sentence that reads like a fault.

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

**Position memory: a file reopens where you left it.** Per file, cide remembers the caret and the
first visible **line** — lines and not pixels, because wrapping is on for every file under the 1 MB
limit and the code font size is a runtime setting, so a pixel offset is wrong after a window resize
or a font change rather than merely imprecise. It survives a tab switch, a project switch, a reload
from disk when the agent edits the file you are reading, a close-and-reopen, and a relaunch.

It is a separate store from `workspace.json` for the same reason `recent.json` is: the record you
most need is the one closing the tab has just removed from there. **A scroll gesture is not a
workspace mutation** and must never become one — `WorkspaceState::update` clones the tree twice,
re-validates it and broadcasts it to every window, per accepted change. The ladder is four rungs,
each an order of magnitude cheaper than the next: a `ViewPlugin` coalesces to one animation frame,
`EditorPane` trailing-debounces to one IPC call per 500 ms, `positions_state.rs` debounces to one
atomic write per 2 s, and `lifecycle::shutdown` flushes. `$XDG_STATE_HOME/cide/positions.json`, 0600,
256 files, LRU, **no TTL** — an expiry means the file you come back to on Monday opens at line 1,
which is the complaint. An explicit navigation always outranks a remembered position: `planRestore`
refuses while a reveal is parked for that path, so Go to definition into a file you had scrolled
lands on the definition.

**Mouse back / forward.** The thumb buttons walk a per-project navigation history. **It has to come
from Rust**, and that is not a preference: WebKitGTK's `buttonForEvent` maps GDK buttons 1–3 and
leaves the rest at `WebMouseEventButton::None`, which `MouseEvent`'s constructor reports as
`button === 0` — so both thumb buttons arrive in the DOM as an ordinary left click and cannot be
told apart there at all. A GTK handler on the `WebKitWebView` widget reads the raw button and emits
`cide://mouse-nav` to that one window. It also **fixes a phantom click that was already live**: a
Ctrl+thumb-press over a path in terminal output used to open the file, and over a buffer used to
fire Go to definition.

The handler names a *button*, never a command. `mouseback`/`mouseforward` are ordinary entries in
`cide-core::keymap`, resolved by the key gate's **third entry point** over the same `resolveStroke`
the other two use — so they get `when` clauses, composed modifiers, and rebinding for free:
`{"key":"mouseback","command":"-navigate.back"}` unbinds. `navigate.back` / `navigate.forward` are
in the palette and are **unbound to any key** by default; `ctrl+alt+shift+left` / `right` are free
in every layer if you want them:

```json
{"key":"ctrl+alt+shift+left","command":"navigate.back"}
{"key":"ctrl+alt+shift+right","command":"navigate.forward"}
```

IDEA's own `Ctrl+Alt+Left/Right` is not free here — `ctrl+alt+right` is `pane.split.right`, and on
KDE both are usually the compositor's virtual-desktop shortcuts — and `alt+left`/`alt+right` are
readline word-motion the window capture listener would take from every terminal.

**Only an explicit navigation is recorded — and, since M16, a far pointer click.** The list of what
deliberately is *not* is in `ui/src/editor/navHistory.ts`: typing and arrow keys, scrolling,
find-as-you-type, the `Alt+Up`/`Alt+Down` member walk (a held key, so ten presses would be ten
entries), switching between already-open tabs, edits, and a Back/Forward move itself. A history that
records caret moves is what makes Back useless.

The pointer was the **one input device that moved the caret and wrote nothing**, which left Back
empty for anyone who navigates by pointing and Forward empty for everyone — Forward only fills once
you have gone Back. `navHistory.ts`'s header had predicted the complaint and written down why the
entry was missing; the objection was that the rule would have to live inside `EditorSurface`, which
is documented as pure. It was answered by moving the *rule* rather than the component:
`recordsClick` is in the import-free module `check:editor` already compiles standalone, and
`navRecorder.ts` is a `ViewPlugin` that reads four facts off a `ViewUpdate` and asks. A click
records when it is a **single empty selection**, from a `select.pointer` transaction, with no
document change, landing **more than 25 lines away or in another file**. So a drag records at most
the one entry its `mousedown` earns; a double-click, a triple-click, a shift-click and an Alt+click
multi-caret record nothing; and clicking about inside the function you are reading — by far the most
common click there is — is not going anywhere. Twenty-five must exceed the merge distance of three,
or `record` would collapse every entry the rule produced.

Two orderings make it work and both compile backwards. CodeMirror runs plugin updates **before**
update listeners, which is what lets the recorder read `caretTrack`'s slot before `EditorSurface`'s
listener hands it to the editor being clicked into — so a click from one pane into another records
the pane you left. And `jump.ts::recordClick` **takes** the origin rather than reading one: read it
a listener later and the origin is the destination, `near` merges them, and the feature is a silent
no-op. The recorder falls back to `update.startState` whenever the live caret is already in this
file, so if that CodeMirror ordering ever changes the cost is a *missing* entry, never a wrong one.

Two consequences, stated rather than left to be met on screen. **The first click after Ctrl+Tab
records the tab you came from**, however little it moved the caret: `caretTrack`'s claims are
released on unmount and an inactive tab is never unmounted, so the slot still names the old file
until something in the new tab takes focus. The switch alone still records nothing — what records
is a pointer landing in a document the caret was not in, which is the rule applied exactly, and it
is what makes Back after a tab switch go somewhere. And **clicking to and fro between two panes of
a split fills the stack with the alternation**, one entry per crossing, because two different files
are never `near` each other; `NAV_CAP` is 50 and that is now a number a session can actually reach.
Both are IDEA's behaviour. The alternative — take the origin from the clicked pane's own previous
caret instead of the slot — is more consistent with "a tab switch is not a jump" and loses twice:
it breaks the split case the feature is for, and it makes Back from a click disagree with Back from
Go to definition about what an origin is.

**Unmet, and written here rather than left to be found.** The navigation history is **per JavaScript
realm and session-scoped**: a detached-pane window keeps its own (always empty, since such a window
never renders an editor) and refuses Back with a sentence rather than walking the shell window's
stack; it does not survive a window reload or a relaunch. Making it durable means a Rust-owned
history and an event per jump to every window, which is not worth it for gesture memory. Also
unmet: `WorkspaceState::flush_if_due` documents itself as "called from the app's tick and before
quitting" and **is called from neither** — there is no app tick — so `workspace.json` is in fact
written exactly once per run, at shutdown, and its 500 ms debounce is inert. The position store does
not inherit that: it runs its own flusher thread rather than assuming a tick exists.

**A context menu no longer scrolls the buffer to the top.** Right-click in an editor, move the
pointer over the menu, dismiss it, and the buffer used to jump to line 1 — but only if the pointer
touched the menu. Every step is WebKit's: a right-click leaves `.cm-content` focused, hovering an
item moves focus to a `<button>`, `FocusController::setFocusedElement` clears the document selection
on *every* focus change, and the `previous.focus()` on the way out then runs
`Element::updateFocusAppearance` on a root editable element whose frame has no selection — which
invents one at the start of the element and *reveals* it. Without the hover, focus never left, so
`Element::focus` took its refocus early return and nothing happened. `preventScroll` fixes the
scroll for every surface; the editor additionally supplies a `restoreFocus` calling
`EditorView.focus()`, because the `setSelection` runs before the reveal and `preventScroll` does not
suppress it — the caret would still be collapsed to the top and read back into state.

Two more found in the same twenty lines. The menu's `box.focus()` was a **no-op** — it ran in the
same commit as the measure effect, before React flushed `setPlacement`, so the box was still
`visibility: hidden` and WebKit refuses focus on such an element. Escape, the arrows, Home/End, Tab
and Enter were therefore reachable only after the pointer touched an item, and a keyboard-invoked
menu (Shift+F10, the Menu key) could not be driven at all. And the `scroll` capture listener that
dismisses the menu did not exempt the menu's own scroller, so arrowing past the fold of a long menu
would have closed it.

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


### And the one position where Go to definition asks the other question

The above shipped as a second binding, and the report came back unchanged: *"goto for golang works
not as i expected - it goes to interface declaration, but should go to implementation."* A feature
on a chord the person who asked has not been told about is the same non-delivery as a feature
behind a default they never saw.

So Ctrl+B and Ctrl+click now redirect — from **one** position, decided by parsing the file the
answer points at rather than by looking at the language. `cide_lang::interface_method_at` asks
whether the definition landed on a method inside an `interface { … }` block. Only that answers
yes, which is exactly the case the report is about and exactly the case where the declaration is
useless. Everything else is untouched, and the two obvious regressions are both unreachable by
construction:

* a Rust struct or trait name lands on its own declaration, never inside an interface body, so
  Ctrl+B never turns into a jump to an `impl` block;
* a Go interface **type** name — `io.Reader` — lands on the type spec, which is *outside* the
  brace block holding the methods, so the declaration still wins.

`navigate.implementation` keeps Ctrl+Alt+B, because "what implements the thing I am standing on"
is a question worth asking from the positions the redirect deliberately never fires on. The two
gestures share `interfaceMethod` on the wire so Ctrl+click and Ctrl+B cannot drift apart, and the
redirect is *injected by the caller* rather than called from inside `goToDefinition`: that
function is `goToImplementation`'s own empty-answer fallback, so wiring it the other way round
would make an interface nobody implements bounce between the two for ever.

## Hidden and ignored files in the trees (M18)

The walk was `hidden(true).git_ignore(true).ignore(true)`, so a dot-prefixed entry and anything a
`.gitignore` covered were simply absent — including `.claude`, which is a directory a user of this
particular IDE has every reason to want to open.

Both are now settings, **and both default to on.** The cost of showing ignored files is real and
asymmetric — dotfiles add tens of entries, while `target/` and `node_modules/` add a very large
number of rows to walk, hold, offer to Ctrl+P and *watch*, and one `Filter` serves the tree, the
picker, content search and symbol indexing alike, so it lands on all four at once. That cost is
why the setting shipped `false`, and shipping it `false` was the wrong call: the feature exists to
answer "cide doesn't show gitignored files - but should", IDEA shows them with nothing configured,
and a feature that stays off until the person who asked finds a switch has not been delivered. The
expensive behaviour is the one that was asked for; the cheap one is a toggle away. `.git` itself
stays hidden either way.

Correcting it needed a schema rung, which is the interesting part. `ExplorerSettings` carries
`#[serde(default)]`, so documents written *before* the field take the new default for free — but
those are exactly the users who do not have the feature yet. Every workspace written by the build
that shipped it contains `"showIgnoredFiles": false`, put there by a constant rather than by a
person. So `persist::v3_to_v4` overwrites `false`, and only `false`: a `true` already agrees, an
absent key is serde's to answer. It is the only rung in the ladder that overwrites anything, and
the exception is bounded to a value that can be shown never to have been a choice. Ignored rows are drawn in a muted tone from the token palette rather than a literal
colour copied from IDEA, so they read as present-but-excluded in both themes.

Two things worth knowing, neither of them hidden in the code: the walk and the *watcher* share the
same matchers, so both halves move together or the tree and its events disagree; and flipping
either toggle rebuilds the `Index`, which is where folder expansion state lives, so the tree comes
back collapsed.

## A find bar in terminal panes, and what Ctrl+V now means (M18)

Ctrl+F opens a find bar over a Claude or shell pane, driven by `@xterm/addon-search`, which was
already a dependency and previously unused.

The scope is the honest half. A full-screen TUI runs on the **alternate** buffer, which *is* the
visible screen — there is no scrollback under it, and `cide_pty`'s vt100 mirror holds the same
alternate screen, so the transcript a user remembers reading is genuinely not being kept by anyone
while Claude owns the display. Search there still works, it simply cannot see one line further
than the user can, and the bar says so (`visible screen only — this program is drawing a
full-screen view`) rather than quietly returning nothing for a word that is plainly ten lines up.
Writing `\x1b[?1049l` to reach the scrollback was rejected: taking the child's screen away in
order to look at it is the `layout/paneHosts.ts` class of bug one layer down.

The cost is deliberate and named in `terminal/keys.ts`: plain `^F` (0x06) no longer reaches the
child, so readline's `forward-char` and vim's page-forward are gone in a terminal pane, on the
same argument that already applies to Ctrl+C and Ctrl+V.

Ctrl+V changed meaning in a Claude pane. It used to pass `^V` straight through so that Claude's own
paste — including **image** paste, which cide cannot do — handled it. That works at Claude's main
prompt and nowhere else: a numbered-choice prompt has no paste handler, so the keystroke vanished
and the only way in was the context menu.

The ordering is what makes the fix non-obvious. xterm's custom key handler is **synchronous**, and
its return value is the only thing that can stop `^V` reaching the pty — but "does the clipboard
hold text?" is an `await`, so the decision to intercept has to be made before the answer exists.
So the keystroke is now *always* intercepted, and re-emitted as `\x16` to the CLI when the
clipboard turns out to hold no text. Text pastes through cide; an image still reaches Claude's own
handler, one round trip later. Both live in the single implementation in `terminal/clipboard.ts`
that the keystroke and the context menu share.

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

### The pane's id and the CLI's conversation are two different things (M17)

`SessionId` is cide's handle for a pane's child: it keys the registry, the webview addresses
panes by it, and it is stable for the pane's whole life. It is **not** reliably the id the CLI
is running under. On 2.1.224, `claude --resume <parent>` runs under a *fresh* conversation id
rather than announcing the parent's, and `/clear` mints another mid-session. The code assumed
otherwise in two places, and both assumptions were load-bearing:

* Hook frames were routed by the payload's `session_id`, so every frame from a resumed pane
  named a uuid no pane held and its effects were emitted to nobody. Measured on a live
  workspace: 2 session ids arriving from hooks, 12 held by panes, **zero overlap**. That is
  the whole of "the finished-turn notification never fires" — it worked in a freshly opened
  pane, where the two ids agree, and nowhere else. Four attempts read the chain and found every
  link correct, because the defect was in which id the links were keyed by.
* Resume named the pane's own id, whose transcript is still on disk after a `/clear`. So a
  restart kept resuming the conversation the user had cleared — "it always restarts with almost
  the first session that was in this panel".

The fix stops inferring identity and carries it. A Claude child is spawned with
`CIDE_SESSION=<the id cide filed it under>`; `cide-hook` reads that from its own environment
and puts it in every frame as `spawned_as`; `HookFrame::owner()` is what effects are keyed by.
The CLI's id is not discarded — it is recorded on the pane as `Pane.conversation` and is what
`restore_for` resumes, so a restart picks up the conversation the user was last looking at.
`Pane.conversation` stays `None` for a pane whose CLI never diverged.

Two consequences worth keeping in mind. `run.sh` builds `cide-hook` as well as `cide-app`,
because the app resolves the hook binary as `current_exe().parent()/cide-hook` and a stale one
reports in an old frame shape **silently** — no error, no log line, just chrome that never
updates. And the hook log now prints `session` and `cli` separately; when they differ, only
`session` addresses a pane.


## Images, and the four things that would have failed silently (M18)

Opening a `png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `ico` or `svg` shows the image rather than
its bytes. The interesting parts are all refusals.

**The bytes go over Tauri's asset protocol, not the IPC control plane.** `emit`/`listen`
string-interpolates JSON into an `eval`'d script and is control-plane only; a 32 MB PNG marshalled
that way is the pathological case that argument exists for. The protocol needs three things to
line up, and any one of them missing fails *silently* — a blank pane, no error anywhere: the
`protocol-asset` cargo feature, `assetProtocol.enable` in `tauri.conf.json`, and
`img-src … asset: http://asset.localhost` in the CSP. All three were already at HEAD.

The static `assetProtocol.scope` is **empty, and stays empty**. A scope wide enough to serve a
repository is a scope wide enough to serve `~/.ssh`, so instead `image_read` grants
`asset_protocol_scope().allow_file()` for the one path the user opened — and grants it *after*
the size, type and header refusals have run, so a directory, a FIFO or a 2 GB tarball never
reaches the protocol on the strength of somebody having clicked something.

**`MAX_IMAGE_BYTES` is 32 MB and is checked against the editor's own cap**, so the two cannot
drift into a state where a file is too big to view and small enough to open as text.

**SVG renders through `<img>` and is never inlined.** An SVG is a document that can carry script,
and inlining one into the DOM is a straightforward XSS vector for a repository you cloned. `<img>`
does not execute script, which is what makes rendering an untrusted one safe at all. The cost is
recorded here rather than hidden: `.svg` no longer opens as editable XML in CodeMirror, which it
used to. That is a real capability loss for anyone who edits SVG source by hand, and it is the one
part of this feature that may need an *Open as text* affordance if it turns out to matter.

**A file that is not what its extension claims fails with a sentence.** `cide_core::image::sniff`
reads the header and says so, rather than mounting a pane that draws nothing. `tiff`, `avif`,
`jxl` and `svgz` are deliberately absent from the extension table: WebKitGTK either cannot decode
them or decodes them only in some builds, and a format that works on one machine and not another
is worse than one that works nowhere — a refusal from Rust is at least readable.
## Opening a file a pane printed, including one outside the project

Ctrl+click a path in any terminal pane and it opens as a tab, at the line and column the
producer named. A **directory** is shown in the file tree instead (M15) — the same `file.reveal`
Ctrl+Shift+E runs, so the sidebar comes to Files first and a path with no row says so. The bytes a
pane prints are attacker-influenced by definition — a build log, a tool result, an agent's
transcript — so `terminal_open_path` is the one command in the app whose path argument is
untrusted, and it is the only route from a pane to the tab list.

**The press is cide's the moment it happens**, before anything is known about what is under it.
That is not a detail: until M15 the gate waited for a completed hover, and in a Claude pane — where
the alt-screen TUI repaints under a stationary pointer and no `mousemove` fires — it usually never
came, so the press reached xterm, xterm wrote a mouse report to the pty, and `claude` answered a
ctrl+click by forking a desktop file manager. cide also answers XTVERSION now, truthfully, which is
how the CLI knows to stand down from ctrl+click and alt+click on an xterm.js host. Every claimed
press ends in an open, a reveal, or a sentence; none ends in silence.

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
  cide-deps/       What a project depends on, and where its source is unpacked.   (M13)
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
