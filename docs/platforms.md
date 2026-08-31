# Platforms

This file is the record of what "Linux-first" costs: what a `cfg` arm does on macOS instead,
which guarantees have no equivalent there, what has actually been compiled, and what still
needs a Mac at the keyboard.

It was the *Platforms (M16)* section of `README.md` until the README became a landing page. The
text below is unchanged apart from its heading levels — most of it exists to keep the
distinction between what is known and what is only read, and that survives only if it is
quoted rather than summarised.

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

## What compiles, and what nobody knows yet

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

## What the first Mac build actually reported

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

## And what the first Mac *bundle* reported: the filesystem, not the code

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

## Type-checking for macOS from Linux — partly, and not where it would have helped

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

**A `cfg(macos)` module inside `cide-app` can still be checked, by lifting it out.** The exclusion
above is about `git2`, not about the module — so a throwaway crate carrying a *verbatim* copy of
the module, with only the cide-side types stubbed and the same third-party dependencies at the
versions the workspace pins, checks under the recipe above with nothing excluded. That is how
`dock.rs`'s AppKit half was checked, and it earned its keep immediately: a missing
`objc2::MainThreadOnly` import (a hard error) and two `clippy::missing_transmute_annotations`
(errors under CI's `-D warnings`). It proves nothing about linking or runtime and it is not
committed — a stubbed copy is a copy, and a committed one drifts — but the recipe is: copy the
module, stub what it imports from this crate, pin the same dependency versions, run the command
above plus `cargo clippy --all-targets`.


## What the Mac itself needs, and what it will spend

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

## And then `cargo test`, which was the next thing to go red

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

## Where a Linux guarantee has no macOS equivalent

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

## What needs a Mac at the keyboard

Everything here is a decision, not a port, and each one has a consequence somebody has to look
at before choosing:

* **The dock icon's own menu — written, checked for Darwin, never run.** Right-clicking the dock
  icon should list every open project and bring the picked one to the front, and
  `crates/cide-app/src/dock.rs` does that: `entries` is the model (all open projects in header
  order, names disambiguated by path only when they collide) and a `cfg(target_os = "macos")`
  module adds `applicationDockMenu:` to tao's app delegate class at runtime, because AppKit offers
  exactly one hook for a dock menu and neither tauri, tao nor muda exposes it. It refuses to
  shadow an implementation that is already there, and the pick goes through
  `cmd::window::focus_project`, which is mode-independent by construction — the menu says the
  same thing in `Stacked` and `PerProject`, and only *which window* a pick raises differs.
  The model is unit-tested here; the AppKit half type-checks and passes `clippy -D warnings` for
  `aarch64-apple-darwin` (see the note in *Type-checking for macOS from Linux* below, which is how
  two real errors in it were found); and **none of it has ever been linked or run.** What a Mac
  has to confirm: that the delegate's class at runtime is what tao's source says, that
  `class_addMethod` returns true against it, that the rows draw live rather than disabled, and
  that a pick on a background app both activates cide and raises the right window. Note this is
  deliberately *not* tied to the window mode — see the module header for why the mode that most
  needs it is the one where the desktop cannot see projects at all.
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
* ~~**The shell pane opens `/bin/bash -l`, hardcoded.**~~ **Fixed in M35**, on the report it
  predicted: *"macos bash and claude problem — it doesn't see all PATH, for example it doesn't
  see nvm/npm"*. The entry read: `ui/src/panes/TerminalPane.tsx`'s `DEFAULT_SHELL` ignores
  `$SHELL` and `getpwuid`; on macOS `/bin/bash` is 3.2.57 (2007, the last GPLv2 release Apple
  shipped) and the user's login shell since Catalina is `/bin/zsh`, so `bash -l` reads
  `/etc/profile` and `~/.bash_profile` and the user's entire `~/.zshrc` never runs — including
  the `eval "$(/opt/homebrew/bin/brew shellenv)"` that is how Homebrew tells people to get on
  `PATH`. The pane now asks for *the login shell* rather than for a program: `TerminalPane` sends
  an empty `program` and `cmd::session::session_spawn` substitutes `cide_core::shell::login_shell`
  (`$SHELL` if executable, then `getpwuid_r`, then `/bin/zsh` on macOS and `/bin/bash` elsewhere,
  then `/bin/sh`), with `-l`. `cide-headless` reads the same function, so there is one ladder.
  **Still unconfirmed on a Mac**: the ladder's rungs are unit-tested over injected inputs on
  Linux and the platform default is a `cfg!`, so what a real `.app` gets from `getpwuid_r` and
  from launchd's `SHELL` has been reasoned about and not observed. Settings → Terminal
  deliberately did *not* grow a field — the OS knows the answer, and a text box pre-filled with
  it has only two reachable states.
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

## The file watcher, and why it is expected to be silent under `/tmp` and `/var`

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

## Finding `claude` from a Finder-launched `.app`

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

Two gaps this left standing were recorded rather than quietly fixed. **The first is closed in
M35 and the second is not:**

* ~~The shell pane still opens `/bin/bash -l`~~ — see the Platforms list above; it now opens the
  account's own login shell.
* `search_paths()` reads the **unscrubbed** process `PATH` while a child gets the scrubbed one, so
  under an AppImage `which()` can in principle see a binary in `$APPDIR/usr/bin` that no child
  can. Harmless today because that directory holds only cide's own binaries, and left alone
  because closing it means teaching discovery about the bundle.

The option not taken was probing the user's login shell for its `PATH`, as VS Code does. It was
described here as *"the principled next step if a report names a directory the static list cannot
reach"* — and in M35 a report did, so **it is taken**. The argument against it stands and shaped
the result, so it is preserved:

> It is strictly more accurate — it is the only way to learn a `PATH` that exists solely inside a
> `~/.zshrc` — and it loses on two counts that are not close: `cide_core::toolchain` opens by
> stating that nothing in it spawns a process, and `$SHELL -ilc 'echo $PATH'` runs the user's
> interactive rc, which can block on a prompt, an ssh-agent unlock or a slow network mount. VS
> Code carries a timeout, a cancel path and a user-facing *resolving shell environment failed*
> dialog because that hangs in the field. It is the principled next step if a report names a
> directory the static list cannot reach, and it should arrive with a timeout and a log line
> rather than silently.

`cide_core::login_path` is the answer to all three objections and each is visible in its shape:

* **`toolchain` still spawns nothing.** The probe is its own module. `toolchain::extra_dirs`
  stays pure and static; a new `toolchain::discovered_dirs` composes it with what the probe
  found, and `search_paths` and `child_env::child_path` both read *that*, which is what keeps
  find-it and give-it one list (`the_path_a_child_searches_is_the_path_which_searched` now
  asserts over it).
* **The hang is bounded twice.** A four-second deadline on the child, and a three-second wait for
  any caller that reaches `dirs()` before the answer lands — timed out, it answers empty rather
  than waiting again, so a wedged rc costs one pause and not one per spawn. It runs once per
  process, on its own thread, started as the first line of `cide_app::run`; `CIDE_NO_SHELL_PATH`
  turns it off.
* **It is not silent.** One `tracing::info!` per launch naming the shell, the elapsed time and
  every directory added — or a `warn!` naming the failure. It is the only way anybody diagnoses
  this: an empty answer and a perfect one look identical from a pane.

Two things it deliberately does not do. The reported `PATH` is **appended**, never prepended, so
the "append, never prepend" rule above is unbroken and nothing the probe finds can shadow a
toolchain the user arranged. And the shell's output is read only from **between two markers** the
probe prints, because an interactive rc file prints things — an nvm banner, a `command not found`
warning, a `motd` — and without the markers a banner becomes a directory name and fails silently
for ever after.

Enabled on Linux as well as macOS, deliberately: a desktop-launched AppImage inherits a narrow
`PATH` for the same reason a Finder-launched `.app` does, and it is the only platform where the
code path can be exercised at all. Measured here on 2026-08-31 with `PATH=/usr/bin:/bin`: 430 ms,
and it recovered nvm, pyenv, linuxbrew, sdkman and `~/go/bin` — every one of them a directory no
static list can name.

## Every extension was dead on macOS, and the reason was four words in a comment

Reported from a Mac: the yaml, graphql and protobuf panels each drew *its worker could not be
loaded from `http://code-ext.localhost/…`*. Three extensions at once is not three bugs; it is one
URL, and it was the platform split in `extAssetUrl`.

Tauri serves a custom scheme two ways, and the boundary is **Windows and Android on one side,
everything else on the other**:

* Windows, Android — `http://<scheme>.localhost/<path>`
* macOS, iOS, Linux — `<scheme>://localhost/<path>`

That is the whole rule, and both ends of Tauri state it: the injected `convertFileSrc` in
`tauri-2.11.5/scripts/core.js` branches on `osName === 'windows' || osName === 'android'` and on
nothing else, and `App::register_uri_scheme_protocol`'s doc comment says it in prose. cide's
`cmd/file.rs` had it right for `asset://`. `extAssetUrl` had it as *"Linux and Android"* versus
*"macOS and Windows"* — one platform on the wrong side, matched by a
`/Windows|Macintosh|Mac OS X/` test that did what the comment said.

So on a Mac every extension asked for an origin nothing serves, the module fetch 404'd, and a
module worker that fails to *load* fires a plain `Event` rather than an `ErrorEvent` — no
`message`, no `filename` — which is why the panel could only name the URL. `host.ts`'s
`describeError` was already written for exactly that and is the reason the report arrived with the
wrong URL in it rather than as `YAML: undefined`; it is the second time that decision paid.

**Why nothing caught it.** The failing arm is unreachable from the machine cide is developed on,
the check suite never ran it, and the two documented safety nets miss it by construction: the
Darwin type-check compiles `cfg(target_os = "macos")` arms and this is a *runtime* string
comparison in TypeScript, and the `macos` CI job builds but does not launch. It is the same shape
as the shell-launcher `PATH` bugs above — correct on Linux, total elsewhere, silent in between.

**What holds it now.** The spelling moved to `ui/src/ext/assetUrl.ts`, import-free so that
`scripts/check-ext.mjs` compiles it standalone and drives `extAssetUrlFor` with one user agent per
platform — Linux, macOS, Windows, Android — asserting the URL each gets. Putting macOS back on the
Windows side fails the check on a Linux machine, which is the only property that matters here.
`cide_ext::assets::split_path` states the same rule from the Rust end, and states it as the reason
the identity pair rides in the path: on two of the four platforms the host is the scheme's own
name, so an identity put there survives only where nobody would have tested it.

## Packaging, and the half that money buys

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

## `--src`: the source tarball, and what its checksum is worth

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

**The tarball carries the licence.** `LICENSE` is MIT, at the root, tracked — so it lands in the
archive for free, which is the whole reason this step is `git archive` and not `tar`. It used to
be missing, and the dual `MIT OR Apache-2.0` claim it was missing the text of made that worse
rather than better: Apache-2.0 §4(a) requires the licence text to accompany redistributed source,
and a tarball on a public release page unambiguously is that. The project is MIT, singly, and
three files say so — `Cargo.toml`'s `license`, `LICENSE` itself, and the `<project_license>` the
AppStream metainfo publishes, which is a literal in `xtask/src/package.rs` rather than something
derived from `AppInfo`, so it has to be changed by hand when the other two are.

## `--tarball`: the binary one, which promises the opposite

`cargo xtask package --tarball --run` writes
`target/release/bundle/tarball/cide-0.1.0-linux-x86_64.tar.gz`, unpacking into `cide-0.1.0/` with
`bin/cide`, `bin/cide-hook`, `bin/cide-rust-analyzer` (cide's own rust-analyzer build, from the
sibling fork pinned by `packaging/rust-analyzer.lock` — M25), the desktop entry, the six
`hicolor` icon sizes, `README.md` and `LICENSE`. It is for the person who wants the program
without a package manager, without root, and without an AppImage's runtime.

Before M25 this archive was 8.1 MiB against the AppImage's 86.5 MiB — a tenth the size, because
it carried no WebKitGTK. The bundled rust-analyzer (a ~50 MiB release binary) ends that ratio:
both archives grow by the same absolute amount, and the tarball's remaining advantage is the
runtime it still does not carry, not a headline size.

**Read it against `--src` above, because every promise inverts.** That one is `git archive`, so
its checksum follows from the commit and anyone can reproduce it from the tag; this one contains
`rustc` output, and its sha256 describes one build on one machine. That one runs on any host;
this one is Linux-only and `impossible_on` refuses it on a Mac, because the archive holds ELF
binaries linked against the host's WebKitGTK. The `--sort=name --owner=0 --group=0
--numeric-owner` flags on its `tar` are for a clean extraction and are *not* a reproducibility
claim — they are there so the archive does not carry the build machine's username in it.

The one thing it asks of whoever unpacks it: `bin/` has to go on `PATH`. The desktop entry it
installs is the same generated `dev.cide.ide.desktop` the Flatpak and the `.deb` use, and its
`Exec=cide %U` is unqualified — so the entry appears in a launcher and starts nothing until
`cide` is findable.

Nothing about the layout is hand-written twice. The icon sizes are a literal in `package.rs`, and
`every_icon_the_tarball_installs_is_one_the_config_declares` ties that literal to
`tauri.conf.json`'s `bundle.icon`, because the failure mode otherwise is an `install` that fails
twenty minutes into a release over an icon somebody deleted.

## Releases are a workflow, not a checklist

`.github/workflows/release.yml`, dispatched by hand with a version. It cuts `release/v<version>`
from master, writes that version into the four files that carry it, regenerates the flatpak files
from it, commits, tags, and only then builds — so artefacts never exist for a tree that was never
tagged. Linux produces the AppImage, the binary tarball and the source tarball; a `macos-15`
runner produces the `.dmg`, blocking rather than advisory. The release notes' download table is
built in the publishing job from the files that were actually uploaded, and the changelog runs
from the previous *release* (falling back to the previous tag, then to the whole history).

Every artefact is built by `cargo xtask package` and none by calling `cargo tauri build`
directly — the task is the packaging brain, and a workflow that reimplemented the runtime
prefetch, the GTK shim or the sidecar would be a second one that drifts from it. What the
workflow adds is the toolchain that task's preflight insists on and CI deliberately omits:
`patchelf`, `librsvg2`, `desktop-file-utils`, a pinned `cargo-tauri`, and
`APPIMAGE_EXTRACT_AND_RUN=1`, because `linuxdeploy` and `appimagetool` are AppImages themselves
and a GitHub runner has no libfuse2.

**The version bump used to be the interesting part and is now a guarded one.** `Cargo.toml` said
`0.2.0` while `tauri.conf.json` said `0.1.0`, and since every artefact name comes from the
latter, a release tagged `v0.2.0` would have shipped `cide_0.1.0_amd64.AppImage`. The job rewrites
one line per file and then asserts exactly that: `git diff --numstat` must read `1 1` for each,
which is what catches a rewrite that produced the right version and reformatted everything around
it. A `json.dump` round-trip did precisely that during development — correct version, twenty
lines of reflow, and a file list the guard had no objection to.

That guard is also why **master must never be hand-bumped to the version being released**: a file
already holding it produces no diff at all, the numstat reads empty rather than `1 1`, and the
release is refused for a reason that reads like a broken regex. It fails before anything is
pushed, so nothing has to be undone — but the fix is to leave the trunk alone, not to weaken the
guard. The `bump` job at the end of the workflow is what keeps the trunk honest instead: after
`publish`, it puts the base branch on `<next patch>-dev` through `scripts/bump-version.sh`. The
`-dev` suffix is what makes the next release possible at all (a bare `0.6.3` would walk into the
guard above), and running after `publish` rather than inside `prepare` is deliberate — a trunk
claiming to be past a release that was never published is the one wrong state available here, and
it is what a failed `linux` or `macos` job would leave behind. It skips, saying so, when the trunk
is already ahead. A rejected push is a warning with the manual commands in the job summary and not
a failure: the release is already out by then, and a protected trunk would otherwise paint an X on
every release for ever. Before that job existed, master sat at `0.6.0` while `v0.6.1` and `v0.6.2`
were published — the release branch is cut, tagged and never merged back.

## Paths stay XDG, deliberately

Under a profile the leaf is `cide-<profile>`; see *`./run.sh` is a separate instance*. It is
decided in `persist::xdg_dir` and nowhere else, so the argument below is unaffected — there is
still one answer per instance to the question "where is my `keymap.json`".

`$XDG_STATE_HOME/cide/workspace.json` and `$XDG_CONFIG_HOME/cide/keymap.json` work on macOS —
nothing fails — but they are not the platform convention, which is
`~/Library/Application Support/dev.cide.ide`. They are staying where they are, and this is the
reason rather than an omission: `keymap.json` is a file people hand-edit and hand around, every
piece of documentation here names one path, and `app_config_dir()` would fork that into two
answers for one question. Hook sockets already fall back from `$XDG_RUNTIME_DIR` to
`std::env::temp_dir()`, which is unset-and-therefore-`$TMPDIR` on every Mac — a per-user 0700
directory, with an explicit 0600 chmod on the socket besides.

