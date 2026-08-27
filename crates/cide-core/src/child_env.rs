//! What every child of cide is given on its way out of `fork`: an environment with the bundle
//! filtered out of it, and a death signal tied to ours — and, once it is running, the one way
//! to reach it with a signal.
//!
//! # Three rules, one module
//!
//! All three answer the same question — *what does a spawn site here owe a child?* — and all
//! three are wrong to skip, silently, in ways that surface three processes away. `CLAUDE.md`
//! already names this module as the one every `Command::new`/`SpawnSpec` in the workspace
//! passes through, so this is where the others belong too.
//!
//! The arming half ([`arm`], [`set_parent_death_signal`], [`on_spawn_thread`]) lived in
//! `cide_claude::orphans` until a second long-lived, memory-hungry child appeared — a language
//! server — and the layering inverted: `cide-lsp` would have had to depend on the *Claude
//! supervisor* to hand the kernel a pid to kill. `cide_claude::orphans` re-exports all three,
//! so no call site moved. The hook-socket sweep stayed behind, because it needs
//! `cide_ide_mcp::lockfile::pid_is_alive` and this crate must not depend on the MCP server.
//! See `docs/adr/0008`.
//!
//! ---
//!
//! # Part one: what a child must *not* inherit
//!
//! # The failure
//!
//! Started from the AppImage, every MCP server a pane's `claude` spawns over stdio dies before
//! it can speak one byte of protocol:
//!
//! ```text
//! Fatal Python error: Failed to import encodings module
//! ModuleNotFoundError: No module named 'encodings'
//! ```
//!
//! which the CLI reports as `CONNECTION_CLOSED` against a server whose configuration is
//! correct — the same `~/.claude.json` entry works in a terminal. Nothing in the message names
//! cide, and nothing in cide's logs mentions it: the death happens two processes down, in a
//! `uvx`-spawned interpreter that never gets to say why.
//!
//! The cause is one variable. AppImageKit's `AppRun` — the entry point of the bundle, four
//! processes above us — rewrites the environment so the *bundled* binary finds the *bundled*
//! libraries, and everything it sets is inherited by every descendant we ever spawn:
//!
//! ```text
//! PATH=$APPDIR/usr/bin/:…:$PATH          LD_LIBRARY_PATH=$APPDIR/usr/lib/:…:$LD_LIBRARY_PATH
//! PYTHONHOME=$APPDIR/usr/                PYTHONPATH=$APPDIR/usr/share/pyshared/:$PYTHONPATH
//! XDG_DATA_DIRS=$APPDIR/usr/share/:…     GSETTINGS_SCHEMA_DIR=$APPDIR/usr/share/glib-2.0/schemas/:…
//! PERLLIB=$APPDIR/usr/share/perl5/:…     QT_PLUGIN_PATH=$APPDIR/usr/lib/qt4/plugins/:…
//! GST_PLUGIN_SYSTEM_PATH[_1_0]=$APPDIR/usr/lib/gstreamer…
//! ```
//!
//! `PYTHONHOME` is the fatal one: it is absolute, it outranks everything, and it points a
//! Python 3.13 interpreter at a prefix that contains no stdlib at all — cide's bundle has no
//! Python in it. `LD_LIBRARY_PATH` is the same class of hazard for any child that links
//! something the bundle also carries (160 libraries, GTK 3 and WebKitGTK among them), and it
//! fails as a symbol lookup error rather than anything legible.
//!
//! None of this is AppImage being wrong. Those variables are exactly right *for the process
//! inside the bundle* and wrong for every process a terminal emulator launches on the user's
//! behalf, which is what cide is. A shell in a pane is the user's shell, not part of our
//! package.
//!
//! # The rule
//!
//! **A path that lives inside the bundle is removed from a child's environment; everything
//! else is left exactly as it was found.** `AppRun` *prepends*, so filtering the bundle's
//! entries out of a list-shaped variable restores the value the user's session actually had,
//! and a variable left with nothing is unset rather than left empty.
//!
//! This is a rule about values, not a list of variable names, deliberately. The list above is
//! what AppImageKit 13 and the `linuxdeploy` GTK hook write today; a plugin added tomorrow
//! (`QML2_IMPORT_PATH`, another loader cache) writes more of the same shape, and a name list
//! would silently stop covering it. Scanning values costs one pass over an environment we are
//! already about to copy into a child.
//!
//! Emptiness matters as much as the prefix. `PYTHONPATH=$APPDIR/usr/share/pyshared/:` — the
//! literal residue of the prepend when the user had no `PYTHONPATH` — filters down to one
//! *empty* entry, and an empty entry in a path list means the current directory. Handing a
//! child `PYTHONPATH=:` would replace a broken interpreter with an interpreter that imports
//! from whatever directory the pane happens to be sitting in, which is worse.
//!
//! # What this cannot do, and does not try
//!
//! * A variable the bundle **overwrote** rather than prepended to is gone before we run.
//!   `PYTHONHOME` is the only one, and a user who had their own is left with none — correct
//!   far more often than the alternative, since the value we would otherwise pass on names a
//!   prefix that ceases to exist the moment cide exits.
//! * `GDK_BACKEND=x11`, `GTK_THEME=Adwaita:dark` and `PYTHONDONTWRITEBYTECODE=1` are set by
//!   the same launcher and are **left alone**: their values name nothing inside the bundle, so
//!   there is no way to tell them from the same variables set by the user's own profile. What
//!   they cost a child is cosmetic (a GUI app launched from a pane runs on XWayland, in
//!   Adwaita) where the ones here cost it its life.
//! * Nothing is scrubbed when `APPDIR` is unset, which is every development run, every `.deb`
//!   install and every Flatpak. `./run.sh` produces an identical environment before and after
//!   this module existed.
//!
//! ---
//!
//! # Part one and a half: what a child must be *given* (M17)
//!
//! The scrub only ever takes away, and that turned out to be half a rule. A macOS user reported
//! *"goto in golang project not working on macos (but works on linux), it just prints: gopls no
//! views, rust-analyzer also not working, it writes: the language server stopped"*. cide had
//! **found** `gopls` — `toolchain::search_paths` adds `~/go/bin`, so discovery never failed and
//! the user never saw the sentence about installing it — spawned it, and handed it launchd's
//! `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, because that is what a Finder-launched `.app` inherits
//! and because everything below this line only removes. A `gopls` that cannot exec `go` cannot
//! build a workspace view and answers `no views` to every request; a rustup `rust-analyzer`
//! proxy with no toolchain reachable exits, which reads as `the language server stopped`.
//!
//! So [`child_path`] is the second half: **append** the directories
//! [`crate::toolchain::extra_dirs`] names — the same ones `which` searched — to whatever `PATH`
//! the child would otherwise have got. [`prepare_command`] applies both passes together, which
//! is why it is no longer called `scrub_command`: a function named for removing that also adds
//! is exactly the drift the comments in this file exist to prevent, and the alternative — leave
//! the name and add a second call at each of the seven spawn sites — recreates the
//! forgettable-second-line failure mode ADR 0008's `arm` already suffers from.
//!
//! The composition order is load-bearing and is stated in [`child_path_in`].

use std::ffi::OsStr;
use std::process::Command;

/// One change to make to a child's inherited environment: `Some(value)` sets it, `None`
/// removes it.
pub type EnvChange = (String, Option<String>);

/// Variables that describe the bundle itself rather than a path inside it.
///
/// `APPIMAGE` is the path of the `.AppImage` file, `ARGV0` the name it was invoked as, and
/// `OWD` the directory the user was in when they ran it — none of which is true of a child, and
/// all of which make a program that knows about AppImages (`appimageupdate`, a self-relaunch,
/// anything asking "am I bundled?") answer yes on cide's behalf. `APPDIR` is listed for
/// symmetry; the value rule below would remove it anyway, since its value *is* the bundle root.
const BUNDLE_MARKERS: [&str; 4] = ["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

/// What must change in a child's environment, computed from this process's own.
///
/// Empty — and cheap — when cide was not launched from a bundle.
pub fn bundle_scrub() -> Vec<EnvChange> {
    let Ok(appdir) = std::env::var("APPDIR") else {
        return Vec::new();
    };
    bundle_scrub_from(std::env::vars(), &appdir)
}

/// The rule itself, over an environment handed in rather than read.
///
/// Pure, because the alternative is a test that mutates the process environment: `set_var` is
/// `unsafe` in edition 2024 precisely because it races every other thread reading it, and this
/// crate's tests run in the same process as everything else.
pub fn bundle_scrub_from(
    vars: impl IntoIterator<Item = (String, String)>,
    appdir: &str,
) -> Vec<EnvChange> {
    // Trailing slashes are stripped so the boundary check below has exactly one shape to
    // handle. An `APPDIR` that is relative or empty is not something we can reason about — no
    // launcher produces one, and treating a relative prefix as "inside the bundle" could match
    // an entry that has nothing to do with us.
    let root = appdir.trim_end_matches('/');
    if !root.starts_with('/') {
        return Vec::new();
    }

    let mut changes = Vec::new();
    for (name, value) in vars {
        if BUNDLE_MARKERS.contains(&name.as_str()) {
            changes.push((name, None));
            continue;
        }
        // Nothing from the bundle in here: leave the variable completely untouched rather than
        // rewriting it to an equal value. A child's environment should differ from its
        // parent's only where we can say why.
        if !value.split(':').any(|entry| under(entry, root)) {
            continue;
        }
        let kept: Vec<&str> = value
            .split(':')
            .filter(|entry| !entry.is_empty() && !under(entry, root))
            .collect();
        changes.push((name, (!kept.is_empty()).then(|| kept.join(":"))));
    }
    changes
}

/// The environment a [`cide_ipc::ClaudeSettings`] asks for.
///
/// # Why this exists at all
///
/// Three of the four toggles this reads — `disable_mouse`, `alt_screen_full_repaint`,
/// `disable_alternate_screen` — were declared in `cide-ipc`, persisted, bound to TypeScript,
/// and rendered as switches in Settings under a panel headed *"Applied at spawn: these reach
/// a pane's child process when it starts"*. They reached nothing. `resume_all_on_launch` was
/// the only field of that struct with a consumer anywhere in the workspace. That is this
/// project's most-repeated defect — built, correct, and wired to nothing — and it mattered
/// here more than usual, because *"Disable the alternate screen"* is precisely the switch a
/// user reaching for a scrollable transcript would press.
///
/// # Why a `Vec<EnvChange>` rather than a `SpawnSpec`
///
/// This crate must not link `cide-pty`, and `base_env` already folds one of these lists
/// ([`bundle_scrub`]) through the same helper. Handing back the same shape means the settings
/// pass and the bundle pass compose instead of being two mechanisms, and it keeps the rule
/// testable without a PTY: the failure this guards against is a *missing* variable, which is
/// invisible unless something can read the list back.
///
/// # Why `false` removes rather than omits
///
/// A toggle that is off states that the variable must not be set — it does not merely decline
/// to set it. The alternative, omitting the entry, loses to one case that is not exotic: a
/// user with `CLAUDE_CODE_DISABLE_MOUSE=1` exported from their shell profile would see the
/// switch sitting at *off* while mouse reporting stayed dead in every pane, with the Settings
/// screen quietly wrong about the state of the program. Since cide promises this panel is what
/// a child gets, the panel has to be authoritative in both directions. The cost is real and
/// accepted: an expert who exports one of these deliberately is overridden by a switch they
/// never touched, which is why the removal is total rather than silent — it applies to a
/// variable cide's own UI names.
///
/// # Why removal, and never `=0`
///
/// The CLI tests these with `V.CLAUDE_CODE_DISABLE_MOUSE !== undefined` before reading the
/// value, so *defined* is most of the decision. Writing `0` or `false` to say "off" is the
/// trap: those are non-empty strings, and on the truthiness test that follows, a `0` turns the
/// mouse **off** — the exact opposite of the switch that produced it. `None` is unambiguous
/// and is the only safe way to spell "off".
pub fn claude_env(settings: &cide_ipc::ClaudeSettings) -> Vec<EnvChange> {
    // Clamped rather than validated-and-rejected. Out of range there is no useful error to
    // raise at a spawn site — the pane must still open — and the CLI's own failure mode is the
    // reason this cannot simply be passed through: it drops a value it dislikes and falls back
    // to a per-renderer default which, for a terminal announcing itself as xterm.js (which
    // cide's XTVERSION reply does, deliberately), is 1. So an unclamped 0 arriving here would
    // not mean "no change", it would mean a third of the scrolling the default gives.
    let range = cide_ipc::ClaudeSettings::SCROLL_SPEED;
    let speed = settings.scroll_speed.clamp(*range.start(), *range.end());

    let flag = |on: bool| on.then(|| "1".to_string());
    vec![
        (
            "CLAUDE_CODE_SCROLL_SPEED".to_string(),
            Some(speed.to_string()),
        ),
        (
            "CLAUDE_CODE_DISABLE_MOUSE".to_string(),
            flag(settings.disable_mouse),
        ),
        (
            "CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT".to_string(),
            flag(settings.alt_screen_full_repaint),
        ),
        (
            "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN".to_string(),
            flag(settings.disable_alternate_screen),
        ),
    ]
}

// ==========================================================================================
// Part one and three-quarters: `EDITOR`, and the editor it names. (M20)
// ==========================================================================================

/// The variable Claude Code's Ctrl+G reads — and `git commit`, and `crontab -e`, and everything
/// else that hands a human a file and waits.
const EDITOR_VAR: &str = "EDITOR";

/// Where the editor [`EDITOR_VAR`] names reports back to. Set beside it and never without it.
const EDIT_SOCK_VAR: &str = "CIDE_EDIT_SOCK";

/// The subcommand [`editor_env_in`] spells and `cide_app::edit_wait::cli` parses.
///
/// A constant in this crate rather than a literal at each end, because the two ends are in
/// different crates and a typo in either is a `$EDITOR` that fails at the moment a user presses
/// a key, with the message coming from a process three removes away.
pub const WAIT_FLAG: &str = "--wait";

/// Said once per process, when this executable's path cannot be spelled in `$EDITOR`.
static EDITOR_UNSPELLABLE: std::sync::Once = std::sync::Once::new();

/// `EDITOR` for a PTY child, and the socket the editor it names reports back over. (M20)
///
/// # The report
///
/// > *"Why in claude session i see: `ctrl+g to edit in VS Code` but we here have integration
/// > with cide IDE, why ctrl+g not opens plan in cide and opens it in vscode?"*
///
/// **Ctrl+G was never part of the IDE integration.** `cide-ide-mcp` publishes `ideName: "cide"`
/// in `~/.claude/ide/<port>.lock` and a real CLI connects to it; that channel carries
/// `openDiff`, `openFile` and `getDiagnostics`, and it was working. The external editor is a
/// separate mechanism with a different shape. The CLI resolves `$EDITOR`, and failing that takes
/// the first of `code`, `vi`, `nano` that is on `PATH`; it then `spawnSync`s that command with
/// the file, **blocks its own turn until that process exits**, and reads the file back off disk.
/// cide set no `EDITOR` and `/usr/bin/code` existed, so the hint read *VS Code* on a machine
/// where nothing about VS Code was otherwise involved — it would have said the same on one with
/// no VS Code integration at all.
///
/// The read-back-after-exit is also why this cannot be an `openFile` over the MCP socket, which
/// is the obvious-looking fix and the wrong one: the CLI needs *a process whose exit means the
/// human is finished*, and a call that returns as soon as the tab appears contains no such
/// moment. Hence a second mode of the `cide` binary — `cide --wait <file>` — and hence this
/// variable, which is the only way a child can be told which running cide to report to.
///
/// # Three refusals, each avoiding a state worse than the default
///
/// **No socket, no `EDITOR`.** The two go out together or not at all. `cide --wait` with no
/// `$CIDE_EDIT_SOCK` can do nothing but exit non-zero, so a spawn site that set the command
/// without the socket would hand its children an editor that fails *every* time — strictly
/// worse than the `code` fallback it displaced. Returning both from one function is what makes
/// that combination unrepresentable at a call site instead of a rule to remember at three.
///
/// **A path with whitespace in it, no `EDITOR`.** The CLI splits this variable on `" "` and
/// takes the first field as the program. There is no quoting, no escaping and no shell, so an
/// executable at `/Applications/My App.app/…/cide` cannot be spelled here at all and the value
/// that would go out names a program called `/Applications/My`. Refusing leaves the CLI's own
/// guess in place, which at least opens something. This is a macOS-shaped hazard and there is
/// no way to fix it from this end; it is a `warn` rather than a silence for that reason.
///
/// **An `EDITOR` the user already set is never overwritten.** Someone with `EDITOR=nvim` in
/// their profile has already answered this question, for `git commit` as much as for Ctrl+G,
/// and a terminal emulator that quietly replaced it would be answering one nobody asked. The
/// case in the report is the *unset* one — which is exactly the case where the CLI guesses — so
/// this reaches precisely the population that had no answer of its own. The user's shell still
/// has the last word either way: an `export EDITOR=…` in a `.bashrc` runs after the environment
/// is inherited and wins, which is how a desktop-launched cide with an empty environment ends
/// up doing what someone's dotfiles say rather than what this function assumed.
///
/// # Why this is not folded into [`terminal_child_env`]
///
/// That function composes from constants and from the user's settings, and takes nothing that
/// only the running application knows. The socket path is exactly that: it carries cide's pid
/// and does not exist until `cide_app::edit_wait::EditWaitServer` has bound it. It is applied
/// where `CIDE_HOOK_SOCK` and `CIDE_AGENT_SOCK` are, beside the two other sockets a child is
/// told about, and for the same reason they are there.
pub fn editor_env(socket: Option<&std::path::Path>) -> Vec<EnvChange> {
    let exe = std::env::current_exe().ok();
    let inherited = std::env::var_os(EDITOR_VAR);
    let changes = editor_env_in(exe.as_deref(), socket, inherited.as_deref());

    // The whitespace refusal, and only that one: the other two are ordinary states (no socket
    // because the server did not bind; an `EDITOR` because the user set one) and neither is
    // worth a line. This one is a packaging fact the user cannot see and cannot act on from
    // inside cide, so it goes in the log a bug report carries.
    if changes.is_empty()
        && socket.is_some()
        && inherited.is_none()
        && let Some(exe) = exe.as_deref()
    {
        EDITOR_UNSPELLABLE.call_once(|| {
            tracing::warn!(
                exe = %exe.display(),
                "this executable's path cannot be spelled in $EDITOR; Ctrl+G in a Claude pane \
                 falls back to the CLI's own guess"
            );
        });
    }
    changes
}

/// The rule itself, over values rather than over this process. See [`editor_env`].
///
/// Pure for [`bundle_scrub_from`]'s reason: the alternative is a test that calls `set_var`,
/// which is `unsafe` in edition 2024 precisely because it races every other thread reading the
/// environment, and this crate's tests share a process with everything else.
pub fn editor_env_in(
    exe: Option<&std::path::Path>,
    socket: Option<&std::path::Path>,
    inherited: Option<&OsStr>,
) -> Vec<EnvChange> {
    // Empty counts as unset, because that is how the reader treats it: the CLI's test is
    // `if (V.EDITOR)`, and the empty string is falsy there. Leaving an empty value in place
    // would be deferring to a preference nobody expressed.
    if inherited.is_some_and(|value| !value.is_empty()) {
        return Vec::new();
    }
    let (Some(exe), Some(socket)) = (exe, socket) else {
        return Vec::new();
    };
    // Both have to survive as UTF-8, since [`EnvChange`] cannot carry anything else. Lossy
    // would be worse than nothing in both slots: it would name a path that does not exist
    // rather than fail to name one, which turns a missing feature into a failing one.
    let (Some(exe), Some(socket)) = (exe.to_str(), socket.to_str()) else {
        return Vec::new();
    };
    // `char::is_whitespace` and not `== ' '`: the CLI splits on a literal space, so a tab in a
    // path would survive this check and then be handed to `spawnSync` as part of the program
    // name. Both are absurd in a path and both are cheaper to exclude than to reason about.
    if exe.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    vec![
        (EDIT_SOCK_VAR.to_string(), Some(socket.to_string())),
        (EDITOR_VAR.to_string(), Some(format!("{exe} {WAIT_FLAG}"))),
    ]
}

/// Is this path-list entry inside the bundle?
///
/// The boundary is a whole path component, so `/tmp/.mount_cideAAA` does not swallow
/// `/tmp/.mount_cideAAAA` — the mount points AppImage generates are one random suffix apart,
/// and a user running two AppImages at once is ordinary.
fn under(entry: &str, root: &str) -> bool {
    entry
        .strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The `PATH` a child should be given, or `None` to leave the inherited one alone.
///
/// The impure wrapper: reads this process's `PATH` and [`crate::toolchain::extra_dirs`], and
/// hands both to [`child_path_in`]. `scrub` must be the [`bundle_scrub`] the same spawn is about
/// to apply — see [`child_path_in`] for why that is not optional.
pub fn child_path(scrub: &[EnvChange]) -> Option<EnvChange> {
    child_path_in(
        scrub,
        std::env::var_os("PATH").as_deref(),
        crate::toolchain::extra_dirs(),
    )
}

/// The rule itself, over a scrub, an inherited `PATH` and a list of directories handed in.
///
/// # Why it takes the scrub, and why that ordering is not a detail
///
/// `bundle_scrub` may rewrite `PATH` — under an AppImage it does, dropping `$APPDIR/usr/bin`,
/// which is the whole of ADR 0007's first line. A `PATH` recomputed from the **process's** value
/// and applied afterwards would put those entries straight back and reintroduce ADR 0007 through
/// the door built to close it: a child would once again resolve binaries out of a mount point
/// that ceases to exist the moment cide quits. So the base is the scrub's own `PATH` entry when
/// it has one, and `inherited` only otherwise. `path_is_built_on_the_scrubbed_value` is the test
/// that fails if someone later "simplifies" [`child_path`] to read the process environment.
///
/// A scrub that *removed* `PATH` outright — every entry it had was inside the bundle, so the
/// user had none of their own — still gets the extras, and that is deliberate: the removal said
/// "none of those directories were yours", not "this child must search nowhere".
///
/// # Non-UTF-8
///
/// [`EnvChange`] is `(String, Option<String>)`, so a `PATH` that is not valid UTF-8 cannot be
/// carried through it. The answer is `None` — leave it alone. A lossy conversion would *corrupt*
/// a working `PATH` rather than fail to improve it, which is strictly worse than doing nothing.
/// (`bundle_scrub_from` sidesteps the same question by taking `std::env::vars()`, which skips
/// non-UTF-8 variables entirely.)
pub fn child_path_in(
    scrub: &[EnvChange],
    inherited: Option<&OsStr>,
    extra: &[std::path::PathBuf],
) -> Option<EnvChange> {
    let base = match scrub.iter().find(|(name, _)| name == "PATH") {
        Some((_, Some(value))) => Some(std::ffi::OsString::from(value)),
        Some((_, None)) => None,
        None => inherited.map(std::ffi::OsString::from),
    };
    let joined = crate::toolchain::child_path_from(base.as_deref(), extra)?;
    Some(("PATH".to_string(), Some(joined.into_string().ok()?)))
}

/// Apply [`bundle_scrub`] and [`child_path`] to a [`Command`] that is about to be spawned.
///
/// For the children spawned with `std::process` — the language servers, `cargo metadata`,
/// `go list`, the `claude` one-shots, `git push`, `claude --version`. PTY children take the same
/// two passes as the first half of [`terminal_child_env`], and get them folded into a
/// `SpawnSpec` by `cide_pty::SpawnSpec::apply`, because this crate cannot see `cide-pty`'s
/// types.
///
/// **Named `prepare_command`, not `scrub_command`.** It was the latter until M17, when it grew
/// the `PATH` pass; a function called *scrub* that adds a variable is the kind of drift the
/// comments in this file exist to prevent, and the compiler finding all seven call sites made the
/// rename the cheap half of the change.
///
/// [`arm`] is deliberately **not** folded in here, even though every one of those call sites
/// wants both. Its contract — *the forking thread must outlive the child* — is an obligation on
/// the caller that no function signature can discharge, and hiding the call would make that
/// obligation unstateable at the place it has to be met.
///
/// The `PATH` pass goes last, so that where both produce a `PATH` the appended list wins; it was
/// built *from* the scrubbed value, so nothing the scrub removed comes back.
pub fn prepare_command(command: &mut Command) {
    prepare_command_with(command, &[]);
}

/// [`crate::toolchain::extra_dirs`] with a caller's own directories appended.
///
/// Pure, and separate from [`prepare_command_with`], so the append-and-dedup rule is a thing a
/// test can state: the impure half reads the process environment and mutates a [`Command`], and
/// neither is observable from a unit test on a machine whose `PATH` is whatever CI gave it.
///
/// [`push_unique`]'s rule, restated one layer up: an empty entry means the current directory in
/// a `PATH` and cide has no business putting a child's cwd on its own search path, and a
/// duplicate costs a `stat` on every lookup for ever.
fn dirs_with(base: &[std::path::PathBuf], extra: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let mut dirs = base.to_vec();
    for dir in extra {
        if !dir.as_os_str().is_empty() && !dirs.contains(dir) {
            dirs.push(dir.clone());
        }
    }
    dirs
}

/// [`prepare_command`], with directories appended to the child's `PATH` beyond
/// [`crate::toolchain::extra_dirs`].
///
/// # The rule: the directory a binary was found in is a directory its process must search
///
/// `toolchain`'s header states the invariant this one obeys from the other side — the
/// directories cide searches to *find* a binary and the directories it gives that binary's
/// process are one list, and widening `search_paths` alone turns a refusal that names a remedy
/// into an opaque `ENOENT`. So a caller that resolved a binary through a search of its *own*
/// (M28's `cide-spec`, which probes Node installation directories `extra_dirs` deliberately does
/// not know about) must hand that directory back here, or it has widened one half of the pair.
///
/// The failure this prevents is not hypothetical and does not look like a `PATH` problem.
/// `openspec` is a `#!/usr/bin/env node` script: with its own directory missing from the child's
/// `PATH`, `execve` **succeeds** and the interpreter line fails, so the error is
/// `env: node: No such file or directory` from a process cide never mentions — bit for bit the
/// compounding failure `toolchain`'s header records for a `gopls` that cannot exec `go`.
///
/// Appended, never prepended, and de-duplicated: [`child_path_in`]'s contract is unchanged, so
/// nothing here can shadow a directory the user arranged themselves.
pub fn prepare_command_with(command: &mut Command, extra: &[std::path::PathBuf]) {
    let scrub = bundle_scrub();
    let dirs = dirs_with(crate::toolchain::extra_dirs(), extra);
    let path = child_path_in(&scrub, std::env::var_os("PATH").as_deref(), &dirs);
    for (name, value) in scrub.into_iter().chain(path) {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

/// What running a child as a filter produced. See [`run_filter`].
#[derive(Debug)]
pub struct Filtered {
    /// Did the child exit successfully? `false` and a populated [`Self::stderr`] is the shape
    /// of every refusal a formatter or a `git` subcommand makes.
    pub ok: bool,
    /// Everything the child wrote to stdout, bytes as written. Never decoded here — a
    /// formatter's output is the user's file and this layer must not normalise it.
    pub stdout: Vec<u8>,
    /// Everything it wrote to stderr, lossily decoded, because it is only ever shown.
    pub stderr: String,
}

/// Why a filter produced nothing. **Tagged and not prose**, so each caller phrases its own
/// sentence: `git blame` and a code formatter fail in the same four ways and have nothing
/// useful to say to a user in the same words.
#[derive(Debug)]
pub enum FilterError {
    /// The program could not be started — almost always "not found on `PATH`".
    Spawn(std::io::Error),
    /// It was still running at the deadline, and has been killed.
    Timeout,
    /// It ran, but its stdout could not be read.
    Unreadable,
    /// It ran and could not be reaped.
    Wait(std::io::Error),
}

/// Run `command` as a filter — bytes in on stdin, bytes out on stdout — and give up after
/// `deadline`.
///
/// # Why this is here rather than beside either caller
///
/// Because there are two, and the second one arrived. It began life inside `cide-git`'s
/// `blame::run_with_deadline`, feeding a dirty buffer to `git blame --contents -`; M26's
/// Reformat code needs exactly the same thing to feed a dirty buffer to `prettier`. The three
/// paragraphs below are the entire reason the function is hard to write, and a second copy of
/// them is a second thing to get subtly wrong — the deadlock they describe does not throw, it
/// hangs.
///
/// # Three threads, each load-bearing
///
/// **stdout is read concurrently**, because the output is routinely larger than a pipe buffer
/// and a child blocked writing it while this thread blocks waiting for the child is a deadlock.
/// **stderr gets its own thread** for the same reason applied to the other pipe: reading two
/// pipes in sequence from one thread deadlocks whenever the child fills the one not being read.
/// **stdin is written on a third**, because a multi-megabyte buffer does not fit in a pipe
/// either, so a single-threaded write-then-read deadlocks on the first large file.
///
/// # Both spawn rules, applied here so no caller can forget
///
/// [`prepare_command`] and [`arm`], in that order — the pair CLAUDE.md requires of every
/// `Command::new` in the workspace. Applying them inside is not tidiness: `blame`'s own call
/// site had `prepare_command` and *not* `arm` for two milestones, which is exactly the miss a
/// chokepoint makes unrepresentable.
///
/// [`arm`]'s contract is that the forking thread outlives the child, and this function
/// satisfies it by construction — every path below either waits for the child or kills it
/// before returning. That is also why this must not be moved onto [`on_spawn_thread`], which
/// exists for the opposite case: children nobody waits for.
pub fn run_filter(
    command: Command,
    stdin: Option<&[u8]>,
    deadline: std::time::Duration,
) -> Result<Filtered, FilterError> {
    run_filter_with(command, stdin, deadline, &[])
}

/// [`run_filter`], with directories appended to the child's `PATH`.
///
/// Split out rather than added as a parameter to [`run_filter`] because the two existing callers
/// — `cide-git`'s `blame` and M26's Reformat — resolve their binaries through
/// `toolchain::which`, whose search *is* the list the child already gets, so an empty slice is
/// the honest answer for both and neither call site should have to write it. See
/// [`prepare_command_with`] for the rule and for the shebang failure it prevents.
pub fn run_filter_with(
    mut command: Command,
    stdin: Option<&[u8]>,
    deadline: std::time::Duration,
    extra_path: &[std::path::PathBuf],
) -> Result<Filtered, FilterError> {
    use std::io::{Read, Write};
    use std::process::Stdio;

    prepare_command_with(&mut command, extra_path);
    arm(&mut command);
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(FilterError::Spawn)?;

    let writer = stdin.map(|bytes| {
        let bytes = bytes.to_vec();
        let mut handle = child.stdin.take();
        std::thread::spawn(move || {
            if let Some(handle) = handle.as_mut() {
                // A failed write is not reported: it means the child exited early, and the exit
                // status and stderr say why in terms the user can read.
                let _ = handle.write_all(&bytes);
            }
            // Explicit, because the pipe has to be *closed* for the child to see end of input.
            // A formatter reading stdin to EOF hangs for ever without this.
            drop(handle);
        })
    });

    let mut out = child.stdout.take().expect("stdout was piped");
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let ok = out.read_to_end(&mut buffer).is_ok();
        let _ = tx.send((ok, buffer));
    });
    let mut errors = child.stderr.take().expect("stderr was piped");
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = errors.read_to_string(&mut text);
        text
    });

    let received = rx.recv_timeout(deadline);
    if received.is_err() {
        // Kill first, then join: both reader threads end when their pipes close, and the writer
        // ends with `EPIPE`. Rust's runtime ignores `SIGPIPE`, so that write returns an error
        // rather than killing this process.
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        let _ = stderr.join();
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        return Err(FilterError::Timeout);
    }

    let (read_ok, stdout) = received.expect("checked above");
    let status = child.wait().map_err(FilterError::Wait)?;
    let _ = reader.join();
    let stderr = stderr.join().unwrap_or_default();
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    if !read_ok {
        return Err(FilterError::Unreadable);
    }
    Ok(Filtered {
        ok: status.success(),
        stdout,
        stderr,
    })
}

/// The whole environment a **PTY** child is given, as one ordered [`EnvChange`] list.
///
/// The composition half of what `cmd::session::base_env` used to do inline. It lives here and
/// the fold lives in `cide_pty::SpawnSpec::apply`, for the reason [`claude_env`] already gives:
/// this crate must not link `cide-pty`, and a second spawn site that needs the identical list —
/// a subagent run — must not have to reimplement it from the comments below. `extra` is folded
/// last; see the final section for what a caller is allowed to put there.
///
/// `version` is a parameter rather than this crate's own `CARGO_PKG_VERSION` because the value
/// a terminal reports is the *application's*. They happen to be the same number today (one
/// workspace `version`), and a caller passing its own is what keeps that a coincidence rather
/// than a dependency.
///
/// # The terminal constants
///
/// `TERM=xterm-256color` rather than plain `xterm` is not cosmetic: with `xterm` the
/// Claude Code TUI falls back to 8 colours and ASCII box-drawing, and the alternate screen
/// does not engage. Scrubbing `TMUX` matters for the same reason — its presence triggers
/// an unconditional 256-colour clamp that visibly desaturates the accent colour. A terminal
/// that inherits stale `COLUMNS`/`LINES` lies to the child about its size until the first
/// SIGWINCH.
///
/// Deliberately absent: `ANTHROPIC_API_KEY`. It outranks subscription OAuth in the
/// credential precedence order, so injecting one would silently bill a Console org for a
/// user on Claude Max. The child inherits its auth by inheriting the environment; cide
/// never reads `~/.claude/.credentials.json`.
///
/// The proxy variables are **not** here: they are the user's configuration rather than a
/// constant of the terminal, so they are a second pass — `cmd::session::apply_proxy`, over the
/// rule in [`crate::proxy`] — applied on top of this one. Nothing about proxying changes the
/// rule in the paragraph above.
///
/// # The first pass, and why it is first
///
/// [`bundle_scrub`] undoes what *our own* launcher did to the environment before a pane ever
/// sees it. Running from the AppImage, `AppRun` leaves `PYTHONHOME` pointing inside a bundle
/// that contains no Python, and every stdio MCP server a pane's `claude` spawns dies on
/// `No module named 'encodings'` before it can speak protocol — reported by the CLI as
/// `CONNECTION_CLOSED` against a configuration that is perfectly correct. It runs first so that
/// the explicit constants above are the ones that survive a collision, and it is a no-op for
/// every non-bundled launch.
///
/// [`child_path`] is M17's half, chained onto it rather than folded separately so the ordering
/// is visible in one expression. `bundle_scrub` only ever *removes*, so until it existed a
/// pane's child got cide's own `PATH` verbatim — which for a Finder-launched `.app` is
/// launchd's `/usr/bin:/bin:/usr/sbin:/sbin` and contains neither Homebrew nor `~/.local/bin`.
/// It appends the directories `toolchain::search_paths` already searches, and it is built
/// *from* the scrub's `PATH` so nothing the scrub dropped comes back.
///
/// This is also what closes the exec gap `docs/platforms.md` records under *Finding `claude` from a
/// Finder-launched `.app`*: `claude_cli::resolve` validates a bare `claude` against
/// `search_paths()`, and portable-pty resolves a bare program against the builder's own `PATH`
/// — so the check and the spawn now consult the same list instead of disagreeing about a
/// directory and turning a refusal with a remedy in it into an opaque `ENOENT`.
///
/// # The `CLAUDE_CODE_*` pass, and why it is late
///
/// [`claude_env`] turns the user's [`cide_ipc::ClaudeSettings`] into the same `EnvChange` list,
/// and is folded **after** the constants above so that a switch the user actually set wins over
/// anything this function assumed. It carries `CLAUDE_CODE_SCROLL_SPEED`, which used to be a
/// literal `3` in `base_env`.
///
/// The comment that literal carried was wrong, and the correction is the point of this
/// paragraph. It read *"xterm.js reports one wheel event per notch, unamplified"*. Claude Code's
/// own renderer heuristic concludes the opposite: it classifies a terminal announcing itself as
/// `xterm.js` — which cide's XTVERSION reply deliberately does, see `ui/src/terminal/xterm.ts`
/// — as a wheel **flooder**, and on that branch its unset default is `1` rather than the `3` it
/// gives other renderers. So this variable was never the amplifier the comment described; it
/// was cancelling a penalty cide had asked for two files away, and landing back on the ordinary
/// default. Setting it remains right. The stated reason was not.
///
/// What a notch is actually worth is the product of two numbers, and cide only owns one of
/// them. xterm.js sends **at most one mouse report per DOM wheel event** — `sendEvent` computes
/// a line count and then discards it — and under a high-resolution wheel on Wayland one notch
/// arrives as several small deltas, each of which `CoreMouseService.consumeWheelEvent` scales by
/// `0.3` when `|deltaY| < 50` on the theory that it is a trackpad. Whether this machine's mouse
/// lands in that regime is not knowable from here, and is not knowable without a wheel and a
/// window. That is why the number is now the user's: it is the half of the product cide can
/// move, from a control, without guessing at the other half.
///
/// Applied to every pane rather than only to `claude` ones, which is deliberate and matches
/// what the literal did before. These variables mean nothing to `bash`, and a user who types
/// `claude` at a shell pane's prompt should get the settings they configured rather than the
/// defaults of a program cide did not notice starting.
///
/// # `extra`, and why *it* stops at a shell pane (M16)
///
/// For a pane, `extra` is [`crate::claude_cli::user_env`] — the user's own variables from their
/// launch configuration — and the caller passes it **only when the pane is a Claude one**. The
/// inconsistency with the paragraph above is deliberate and is written down here so it is not
/// "fixed" later: the four `CLAUDE_CODE_*` names are inert to `bash` — a shell that inherits
/// them is a shell that ignores them — while an arbitrary `NODE_OPTIONS`, `GIT_SSH_COMMAND` or
/// `PATH` from that list is not inert to anything. A field labelled *the environment claude
/// panes are spawned with* must not quietly become the environment the user's own shell is
/// spawned with too. This function itself is unconditional and must stay so: it decides
/// nothing about which pane it is composing for, and folds whatever it is handed.
///
/// Folded last of all so that a variable the user set beats a constant this function assumed —
/// which is why `TERM`, `COLUMNS`, `LINES` and `TMUX` are on `claude_cli`'s refusal list rather
/// than left to be shadowed. Everything applied *after* this function — the proxy pass,
/// `CLAUDE_CODE_SSE_PORT`, `CIDE_HOOK_SOCK` — is out of the user's reach by construction, which
/// is the other half of why those names are refused rather than merely discouraged: a value
/// this list carried for one of them would be overwritten with nothing on screen saying so.
pub fn terminal_child_env(
    claude: &cide_ipc::ClaudeSettings,
    version: &str,
    extra: Vec<EnvChange>,
) -> Vec<EnvChange> {
    let scrub = bundle_scrub();
    let path = child_path(&scrub);
    let mut changes: Vec<EnvChange> = scrub.into_iter().chain(path).collect();
    changes.extend([
        ("TERM".to_string(), Some("xterm-256color".to_string())),
        ("COLORTERM".to_string(), Some("truecolor".to_string())),
        ("TERM_PROGRAM".to_string(), Some("cide".to_string())),
        (
            "TERM_PROGRAM_VERSION".to_string(),
            Some(version.to_string()),
        ),
        ("TMUX".to_string(), None),
        ("TMUX_PANE".to_string(), None),
        ("COLUMNS".to_string(), None),
        ("LINES".to_string(), None),
        ("CI".to_string(), None),
    ]);
    changes.extend(claude_env(claude));
    changes.extend(extra);
    changes
}

// ==========================================================================================
// Part two: what must happen to a child when cide dies.
// ==========================================================================================
//
// A clean quit runs `lifecycle::shutdown`, which kills every child and drops every handle. A
// `SIGKILL` — `kill -9`, an OOM kill, a compositor tearing the session down — runs nothing, and
// every PTY child keeps running: `claude` is a session leader in its own process group
// (`portable-pty` calls `setsid` before `exec`), so it does not even get a `SIGHUP` when the
// master fd closes. It sits on a dead terminal holding a subscription slot until the user finds
// it in `ps`. A language server is the same shape and costs more: rust-analyzer on a large
// workspace is 1–4 GB of resident memory with nothing left to talk to.
//
// # `PR_SET_PDEATHSIG`, and the three facts that decide how it must be used
//
// All three are from `prctl(2)` on Linux 6.x, and all three fail silently rather than loudly:
//
//  1. **It is cleared for the child of `fork(2)`** — `copy_process()` does
//     `p->pdeath_signal = 0`. Arming the *parent* and then spawning achieves nothing at all. It
//     must be set in the child, between `fork` and `exec`, which on Linux means
//     `CommandExt::pre_exec`. (The brief for the original work said the setting is inherited
//     across `fork`; it is not, and building on that would have shipped a no-op that tests
//     clean.)
//  2. **It survives `execve(2)`** — except when the image is set-user-ID, set-group-ID or
//     carries file capabilities, where the kernel clears it with the rest of the privileged-exec
//     cleanup. `claude`, `rust-analyzer` and `gopls` are none of those, but a user's `$SHELL`
//     theoretically could be, so this is a degradation and not a guarantee. Surviving `exec` is
//     also what makes the approach work at all.
//  3. **The "parent" is the *thread* that forked, not the process.** The signal is delivered
//     when that thread exits, with the process very much alive. A pane spawned from a
//     short-lived worker thread would be killed seconds later for no reason a user could
//     diagnose. [`on_spawn_thread`] exists solely to remove that possibility.
//
// There is a fourth, smaller trap, handled in [`set_parent_death_signal`]: if the parent dies
// *between* the `fork` and the `prctl`, the signal is armed against a parent that is already
// gone and will never be delivered.
//
// # Why `SIGTERM` and not `SIGKILL`
//
// `SIGTERM` gives `claude` its normal exit path — flushing a transcript, releasing its
// subscription slot — a shell pane the chance to finish a `write(2)` into one of the user's
// files, and `gopls` the chance to write its cache. `SIGKILL` would guarantee the child dies,
// which is only worth having if a child might ignore `SIGTERM`; none of them does. The cost of
// being wrong the other way is a truncated file in the user's repository, so this errs towards
// the polite signal.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

/// The signal a child is sent when cide dies. See above for why not `SIGKILL`.
#[cfg(unix)]
pub const DEATH_SIGNAL: libc::c_int = libc::SIGTERM;

/// Whether this platform can actually enforce what [`arm`] promises.
///
/// `true` only on Linux, where `PR_SET_PDEATHSIG` exists. It is a public constant rather than a
/// private `cfg!` so that the gap is a *value* other code can read, report and test against,
/// instead of a silence — see [`set_parent_death_signal`]'s non-Linux arm for what is and is not
/// still true off Linux, and `docs/platforms.md` for the consequence.
///
/// Nothing branches on this to change behaviour; [`arm`] is called unconditionally at every
/// spawn site and the platform decides how much of it takes effect. It exists so that "cide's
/// children die with it" can be *stated* per platform rather than assumed everywhere.
pub const PARENT_DEATH_IS_ENFORCED: bool = cfg!(target_os = "linux");

/// Said once per process, on a platform that cannot enforce the guarantee.
///
/// Once, not once per spawn: a Claude pane, a shell pane, two language servers and a
/// `cargo metadata` on every project open would make this the loudest line in the log and the
/// least informative. It is a `warn` and not a `debug` because it changes what a crash costs the
/// user — orphaned `claude` processes holding subscription slots — and that belongs in the log
/// a user is asked to send back.
#[cfg(unix)]
static DEATH_SIGNAL_UNSUPPORTED_WARNED: std::sync::Once = std::sync::Once::new();

/// Arrange for `command`'s child to be signalled when this process dies.
///
/// Installs a `pre_exec` hook, so the `prctl` happens in the forked child where it counts and
/// then survives the `exec`. A failure to arm is *not* fatal — the hook returns `Ok(())`
/// regardless — because a child that outlives a crash is strictly better than a pane that
/// refuses to open. The `prctl` cannot fail for a valid signal number in practice; the
/// tolerance is for the seccomp-filtered and non-Linux cases.
///
/// **The calling thread must outlive the child.** That is fact 3 above, and it is the reason
/// [`on_spawn_thread`] exists. A `tokio` task does not satisfy it: a task can be moved between
/// workers, so the thread that forked can retire while the child is healthy, and the kernel
/// then delivers `SIGTERM` to a working language server on a work-stealing schedule nobody can
/// reproduce. Spawn from a thread you own and keep.
///
/// **Off Linux this arms almost nothing**, and it says so in the log rather than pretending.
/// See [`PARENT_DEATH_IS_ENFORCED`] and [`set_parent_death_signal`].
#[cfg(unix)]
pub fn arm(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    if !PARENT_DEATH_IS_ENFORCED {
        DEATH_SIGNAL_UNSUPPORTED_WARNED.call_once(|| {
            tracing::warn!(
                "this platform has no PR_SET_PDEATHSIG, so a SIGKILL or crash of cide will \
                 leave its children — claude sessions, language servers — running. A clean \
                 quit still stops them; see cide_core::child_env::set_parent_death_signal"
            );
        });
    }

    // Read *here*, in the parent, before the fork. Inside `pre_exec` this is exactly what
    // `getppid()` should return, and comparing the two is how the child detects a parent that
    // died in the window between the fork and the prctl.
    let spawner = std::process::id();
    // SAFETY: `pre_exec` runs between `fork` and `exec` in a process that may hold locks
    // belonging to threads that did not come along. Everything the closure calls —
    // `prctl(2)`, `getppid(2)`, `raise(3)` — is a bare syscall or async-signal-safe, and it
    // allocates nothing.
    unsafe {
        command.pre_exec(move || {
            set_parent_death_signal(spawner, DEATH_SIGNAL);
            Ok(())
        });
    }
}

/// Non-unix: nothing to arm.
#[cfg(not(unix))]
pub fn arm(_command: &mut Command) {}

/// Arm the **calling** process to be signalled when `expected_parent` dies.
///
/// Called from inside a `pre_exec` hook, which is why it takes the parent's pid rather than
/// reading it: `getppid()` here is the pid to *compare against*, not the value to trust.
///
/// Public so that a spawner this crate does not own can use the same implementation — a
/// `pre_exec` hook installed anywhere in the workspace should call this rather than write its
/// own `prctl`.
///
/// # PTY panes are not armed, and this is where that is written down
///
/// An earlier version of this comment said the caller that mattered was `cide-pty`. **It is
/// not a caller at all**, and never was: `crates/cide-pty/Cargo.toml` does not depend on
/// `cide-core`, and a grep for this function across the workspace finds the definition, the
/// `cide-claude::orphans` re-export and a type-check test. The armed children are the ones
/// spawned through `std::process::Command` — the language servers (`cide_lsp::server`), the
/// `claude` one-shots (`cide_claude::headless`) and dependency resolution (`cide-deps`).
///
/// Two things stand in the way of arming a PTY pane, and only the first is the one the old
/// comment named:
///
///  1. `portable-pty`'s `CommandBuilder` exposes no `pre_exec` hook. Its own hook already runs
///     `setsid`, and there is no seam to add to it from outside the crate.
///  2. **`PtySession::spawn` is called from a Tauri command worker** (`cmd::session`), and fact
///     3 above says the signal fires when the *forking thread* exits. Arming there would hand
///     the kernel a pid to kill the moment a pooled worker retired — a `claude` dying seconds
///     after the pane opened, with nothing anywhere to say why. Arming PTY panes therefore
///     means routing their spawn through [`on_spawn_thread`] first; it is not a one-line change
///     and it is not free (that thread is process-global and serialises every spawn).
///
/// What covers a PTY child today: a clean quit runs `lifecycle::shutdown`'s
/// SIGHUP→SIGTERM→SIGKILL ladder over the whole process group, a caught signal reaches the same
/// function through `lifecycle`'s signal thread, and `run.sh` reaps `claude` processes orphaned
/// by a previous hard kill before it starts the next one. The uncovered case is a `SIGKILL` or
/// OOM kill of a cide that was **not** launched from `run.sh`.
///
/// Returns nothing. There is no useful recovery in a forked child, and the caller's only
/// alternative to ignoring the error is to fail the spawn, which is worse.
#[cfg(target_os = "linux")]
pub fn set_parent_death_signal(expected_parent: u32, signal: libc::c_int) {
    // SAFETY: `prctl` with PR_SET_PDEATHSIG reads no pointer and touches nothing but this
    // task's `pdeath_signal` field.
    let armed = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, signal as libc::c_ulong, 0, 0, 0) };
    if armed != 0 {
        return;
    }

    signal_self_if_already_orphaned(expected_parent, signal);
}

/// The half of the arrangement that needs no kernel support, shared by both platform arms.
///
/// The race the `prctl(2)` man page does not spell out: if the parent exited between the `fork`
/// and the arming, the death it was armed for has already happened and nothing will ever deliver
/// the signal. Re-parenting is the observable evidence — `getppid()` becomes 1, or the nearest
/// subreaper — so a mismatch here means "already orphaned", and the child does to itself what
/// the kernel now never will.
///
/// **One body, called from both `cfg` arms, on purpose.** The non-Linux arm is the whole of what
/// that platform can do, and an arm written out separately is an arm that can quietly become
/// empty again — which is how a guarantee gets dropped on a platform without anybody saying so.
/// Sharing the body makes "macOS still closes the fork race" true by construction rather than by
/// a second copy nobody runs, and lets the test below cover both platforms with one assertion.
#[cfg(unix)]
fn signal_self_if_already_orphaned(expected_parent: u32, signal: libc::c_int) {
    // SAFETY: both calls are bare syscalls with no arguments to get wrong, and both are
    // async-signal-safe, which is what a `pre_exec` hook requires.
    if unsafe { libc::getppid() } as u32 != expected_parent {
        unsafe { libc::raise(signal) };
    }
}

/// macOS and the BSDs: the fork race is still closed, the standing guarantee is **not**.
///
/// This arm is deliberately not empty, and the difference between what it does and what the
/// Linux arm does is the whole macOS story for ADR 0008. Read it before assuming a Mac build
/// behaves like this one.
///
/// **What still holds.** The half that needs no kernel support is the fourth trap above: if the
/// parent died between the `fork` and this call, the child is already orphaned and nothing will
/// ever come for it, so it does to itself what the kernel would have. `getppid()` and `raise()`
/// are POSIX and async-signal-safe everywhere, so that check is portable and is performed.
///
/// **What does not.** There is no `PR_SET_PDEATHSIG` outside Linux, so the standing arrangement
/// — *signal me whenever my parent goes* — simply does not exist. A `SIGKILL`, an OOM kill or a
/// crash of cide on macOS leaves every child it spawned running: each `claude` holding a
/// subscription slot, each `rust-analyzer` holding 1–4 GB. A clean quit is unaffected;
/// `lifecycle::shutdown`'s ladder is what stops children there and it is platform-independent.
///
/// **Why there is no equivalent, rather than one nobody wrote.** The BSD answer is `kqueue`'s
/// `EVFILT_PROC`/`NOTE_EXIT` on the parent's pid, and it cannot be used from here: this function
/// runs between `fork` and `exec`, and `exec` destroys every thread and every file descriptor
/// that was not marked to survive it. A kqueue registered here is gone microseconds later.
/// Delivering it properly means a *supervising process* — a shim that spawns the real child,
/// waits on both, and kills the group when cide's pid exits — which is a design, not a syscall,
/// and would want the existing second binary (`cide-hook`) to grow a `reap` mode.
///
/// **The cheaper first pass, also not done.** A startup sweep: record the pids cide spawns, and
/// on the next launch kill any whose recording cide is dead. `cide_claude::orphans` already has
/// exactly this shape for hook sockets — same liveness rule, same bias towards leaving things
/// alone — but it sweeps *files*, and nothing in the workspace records a child pid, so this is
/// new state rather than an extension. It also recovers at the next launch instead of
/// immediately, which is strictly weaker than `PDEATHSIG`. Both are written up in `docs/platforms.md`
/// under Platforms; neither is implemented, and [`PARENT_DEATH_IS_ENFORCED`] is `false` here so
/// that no caller can read this arm as equivalent.
#[cfg(all(unix, not(target_os = "linux")))]
pub fn set_parent_death_signal(expected_parent: u32, signal: libc::c_int) {
    signal_self_if_already_orphaned(expected_parent, signal);
}

/// Run `f` on the one thread that spawns children, and return what it returned.
///
/// This exists for exactly one reason, and it is fact 3 above: `PDEATHSIG` is delivered when the
/// **thread** that forked the child exits, not when the process does. A Tauri command handler
/// runs on a pooled worker thread that can be retired at any time, so arming a pane spawned from
/// one would give the user a `claude` that dies a few seconds after it started, with nothing
/// anywhere to say why. The thread this uses is created once, is never joined, and therefore
/// outlives every child it makes.
///
/// A dedicated thread rather than "spawn from the main thread": the main thread is the GTK
/// loop, and a PTY spawn on it stalls every window for the duration of a `fork`.
///
/// Serialising every spawn through one thread is not a bottleneck worth avoiding — a spawn is
/// a `fork`/`exec` measured in single-digit milliseconds and happens once per pane.
///
/// **`f` must not call this function again.** One thread serves every job, so a nested call
/// waits for a reply from the thread it is already running on and hangs both for ever. Not
/// guarded against, because the guard would be a thread-local and a second error path for a
/// mistake the type system already makes awkward — but it is the one shape that deadlocks.
///
/// # A panicking job kills the job, never the thread
///
/// The thread is created once behind a `OnceLock` and can therefore never be replaced: if it
/// unwound, `spawner()` would keep handing out a `Sender` whose receiver is gone and *every
/// later spawn in the process would fail*, with a message naming this module rather than
/// whatever actually went wrong. For the caller that is one broken pane; for the next caller
/// it is an application that can no longer open a pane in any window until it is restarted.
///
/// So the job is run inside [`std::panic::catch_unwind`] and the panic is carried back and
/// resumed on the calling thread, which is where it belongs: the caller sees exactly the
/// panic it would have seen had `f` run inline, and the spawn thread is still there for the
/// next pane. `AssertUnwindSafe` is sound here for that reason — nothing observes any state
/// `f` touched, because the unwind continues in the caller.
pub fn on_spawn_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    type Panic = Box<dyn std::any::Any + Send + 'static>;

    let (reply_tx, reply_rx) = channel::<Result<T, Panic>>();
    let job = Box::new(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        // The receiver is dropped only if the caller was cancelled, which cannot happen: the
        // caller is blocked on `recv` below. Ignored rather than unwrapped so a future caller
        // that does time out cannot panic this thread and take spawning down with it.
        let _ = reply_tx.send(outcome);
    });

    // Both `expect`s are genuinely unreachable rather than merely unlikely: the only way the
    // thread could stop serving was a job that unwound, and jobs no longer unwind.
    spawner()
        .send(job)
        .expect("the cide-spawn thread has gone away");
    match reply_rx
        .recv()
        .expect("the cide-spawn thread dropped a job")
    {
        Ok(value) => value,
        // Not `panic!("the spawn panicked")`: that would replace the original payload,
        // location and backtrace with a sentence about threading, and the thing worth
        // reading is the panic `f` raised.
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

type Job = Box<dyn FnOnce() + Send + 'static>;

fn spawner() -> &'static Sender<Job> {
    static SPAWNER: OnceLock<Sender<Job>> = OnceLock::new();
    SPAWNER.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new()
            .name("cide-spawn".into())
            .spawn(move || {
                // Runs until the process exits. Never joined, never shut down: the moment it
                // ends, every child it forked is signalled, so "tidying it up at shutdown"
                // would be a way of killing panes early.
                while let Ok(job) = rx.recv() {
                    job();
                }
            })
            .expect("spawn the cide-spawn thread");
        tx
    })
}

// ==========================================================================================
// Part three: how a running child is signalled.
// ==========================================================================================
//
// The two parts above are about the moment of the fork. This one is about every moment after
// it, and it is here for the same reason `arm` is: it answers *what does a spawn site here owe
// a child?* — a child cide started is a child cide has to be able to reach — this module
// already links `libc` on unix, and it is exactly where `orphans::arm` moved when it gained a
// second consumer. Same move, same reason.
//
// It lived as a private `deliver` in `cide_app::lifecycle`, which was fine while the shutdown
// ladder was the only caller. M18's pause needs `SIGSTOP`/`SIGCONT` from `cide-agents`, and
// `cide-agents` cannot depend on `cide-app` — nothing may, except the binary.

/// Send `signal` to the child's **process group**, falling back to the process itself.
///
/// The group is what matters. `portable-pty` starts the child in a new session, so it leads
/// a process group holding everything it spawned — a bash tool invocation, an MCP server —
/// and signalling the leader alone leaves those running with the pty closed under them.
/// `kill(-pid)` fails when the child never became a group leader, hence the fallback.
///
/// Takes a raw signal number rather than an enum on purpose: the callers do not agree on a
/// vocabulary. The shutdown ladder has three rungs it escalates through; a pause has two
/// signals that are not rungs of anything and must never be handed to the ladder. Mapping a
/// caller's vocabulary onto a number is one line at each call site, and one enum covering both
/// would be an enum whose variants are only valid for half of its consumers.
#[cfg(unix)]
pub fn signal_group(pid: u32, signal: libc::c_int) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // Negating 0 or 1 turns one signal into a broadcast: `kill(0, …)` hits this process's
    // own group, and `kill(-1, …)` hits every process this user is allowed to signal.
    // Neither is ever a pty child, so arriving here with one is a bug to refuse, not obey.
    if pid <= 1 {
        return;
    }
    // SAFETY: `kill` takes two integers and touches no memory owned by this process.
    unsafe {
        if libc::kill(-pid, signal) == -1 {
            libc::kill(pid, signal);
        }
    }
}

/// No-op off unix, which makes the whole shutdown ladder — and the pause — one off unix. That
/// matches `cide_app::lifecycle::install_signal_handlers`: Linux is the supported target, and a
/// Windows build would need a job object rather than a translation of `kill(2)`.
///
/// `i32` rather than `libc::c_int` because `libc` is a unix-only dependency of this crate; the
/// two are the same type on every target Rust supports.
#[cfg(not(unix))]
pub fn signal_group(_pid: u32, _signal: i32) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mount point of a running AppImage, in the shape the runtime actually produces.
    const APPDIR: &str = "/tmp/.mount_cide_0OOoGFm";

    fn scrub(vars: &[(&str, &str)]) -> Vec<EnvChange> {
        bundle_scrub_from(
            vars.iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
            APPDIR,
        )
    }

    fn change<'a>(changes: &'a [EnvChange], name: &str) -> Option<&'a Option<String>> {
        changes.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    #[test]
    fn the_fatal_one_is_removed() {
        // Verbatim from the session that could not start an MCP server. Absolute, entirely
        // inside the bundle, and nothing survives filtering it — so the variable goes.
        let changes = scrub(&[("PYTHONHOME", "/tmp/.mount_cide_0OOoGFm/usr/")]);
        assert_eq!(change(&changes, "PYTHONHOME"), Some(&None));
    }

    #[test]
    fn a_prepend_onto_nothing_leaves_no_empty_entry() {
        // `PYTHONPATH=$APPDIR/usr/share/pyshared/:$PYTHONPATH` with no inherited `PYTHONPATH`
        // is what produced this trailing colon. Passing on `PYTHONPATH=:` would tell every
        // Python child to import from the pane's current directory.
        let changes = scrub(&[(
            "PYTHONPATH",
            "/tmp/.mount_cide_0OOoGFm/usr/share/pyshared/:",
        )]);
        assert_eq!(change(&changes, "PYTHONPATH"), Some(&None));
    }

    #[test]
    fn a_prepend_onto_something_gives_back_exactly_what_the_user_had() {
        let changes = scrub(&[
            (
                "PATH",
                "/tmp/.mount_cide_0OOoGFm/usr/bin/:/tmp/.mount_cide_0OOoGFm/usr/sbin/:/home/u/bin:/usr/bin",
            ),
            (
                "XDG_DATA_DIRS",
                "/tmp/.mount_cide_0OOoGFm/usr/share:/usr/share:/usr/local/share",
            ),
        ]);
        assert_eq!(
            change(&changes, "PATH"),
            Some(&Some("/home/u/bin:/usr/bin".to_string()))
        );
        assert_eq!(
            change(&changes, "XDG_DATA_DIRS"),
            Some(&Some("/usr/share:/usr/local/share".to_string()))
        );
    }

    #[test]
    fn the_double_slash_the_gtk_hook_writes_is_still_inside_the_bundle() {
        // `linuxdeploy-plugin-gtk.sh` interpolates `$APPDIR//usr/...`. A prefix check that
        // demanded a single separator would keep every one of these.
        let changes = scrub(&[(
            "GDK_PIXBUF_MODULE_FILE",
            "/tmp/.mount_cide_0OOoGFm//usr/lib64/gdk-pixbuf-2.0/2.10.0/loaders.cache",
        )]);
        assert_eq!(change(&changes, "GDK_PIXBUF_MODULE_FILE"), Some(&None));
    }

    #[test]
    fn a_neighbouring_mount_is_not_ours_to_touch() {
        // Two AppImages running at once differ by a random suffix, and one must not scrub the
        // other's paths out of a shell that legitimately has them.
        let changes = scrub(&[("PATH", "/tmp/.mount_cide_0OOoGFmX/usr/bin:/usr/bin")]);
        assert!(
            change(&changes, "PATH").is_none(),
            "a longer mount name is a different mount: {changes:?}"
        );
    }

    #[test]
    fn variables_the_bundle_never_touched_are_left_alone() {
        // Including one whose value contains a colon and one that names a path elsewhere: the
        // rule is about the bundle, not about path-shaped values.
        let changes = scrub(&[
            ("GTK_THEME", "Adwaita:dark"),
            ("HOME", "/home/u"),
            ("PYTHONPATH", "/home/u/lib/python"),
            ("TRACKER_TOKEN", "secret"),
        ]);
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn the_bundle_markers_go_even_though_they_name_no_path_inside_it() {
        let changes = scrub(&[
            ("APPDIR", APPDIR),
            ("APPIMAGE", "/home/u/bin/cide_0.1.0_amd64.AppImage"),
            ("ARGV0", "cide"),
            ("OWD", "/home/u/work/cide"),
        ]);
        for name in BUNDLE_MARKERS {
            assert_eq!(change(&changes, name), Some(&None), "{name}");
        }
        assert_eq!(changes.len(), 4, "each marker exactly once: {changes:?}");
    }

    #[test]
    fn a_development_run_changes_nothing_at_all() {
        // `APPDIR` unset is the path `bundle_scrub` takes for `./run.sh`, the `.deb` and the
        // Flatpak. An empty or relative one cannot be reasoned about and is treated the same.
        let vars = [("PATH".to_string(), "/usr/bin".to_string())];
        assert!(bundle_scrub_from(vars.clone(), "").is_empty());
        assert!(bundle_scrub_from(vars, "usr").is_empty());
    }

    // --- part one and a half: the PATH a child is given ---------------------------------

    fn extra(names: &[&str]) -> Vec<std::path::PathBuf> {
        names.iter().map(std::path::PathBuf::from).collect()
    }

    fn path_of(change: Option<EnvChange>) -> Option<String> {
        let (name, value) = change?;
        assert_eq!(name, "PATH");
        value
    }

    /// The ADR 0007 composition test, and the one that fails if [`child_path`] is ever
    /// "simplified" to read the process environment directly.
    #[test]
    fn the_child_path_is_built_on_the_scrubbed_value_and_never_on_the_inherited_one() {
        // What an AppImage actually hands us: `AppRun` prepended two directories inside the
        // mount, and `bundle_scrub` has already decided they must go. Rebuilding from the
        // process's own PATH here would put them straight back — a child resolving binaries out
        // of a mount point that vanishes the moment cide quits, which is the exact failure ADR
        // 0007 exists to close.
        let scrub = vec![("PATH".to_string(), Some("/usr/bin".to_string()))];
        let inherited = std::ffi::OsString::from(
            "/tmp/.mount_cide_0OOoGFm/usr/bin:/tmp/.mount_cide_0OOoGFm/usr/sbin:/usr/bin",
        );
        let path = path_of(child_path_in(
            &scrub,
            Some(&inherited),
            &extra(["/home/u/go/bin"].as_slice()),
        ))
        .expect("one directory was missing, so a PATH is set");
        assert_eq!(path, "/usr/bin:/home/u/go/bin");
        assert!(
            !path.contains(".mount_"),
            "the bundle's own directories came back through the PATH pass: {path}"
        );
    }

    /// M28's half of `toolchain`'s one-list rule: a binary found through a search of the
    /// caller's own must have that directory on the `PATH` its process is given.
    ///
    /// The failure it guards is invisible as a `PATH` bug. `openspec` is a
    /// `#!/usr/bin/env node` script, so with its own nvm directory missing the `execve`
    /// *succeeds* and the shebang dies — `env: node: No such file or directory`, from a process
    /// cide never names.
    #[test]
    fn the_path_a_child_searches_includes_the_directory_the_binary_came_from() {
        let base = extra(["/home/u/.cargo/bin", "/home/u/go/bin"].as_slice());
        let found = extra(["/home/u/.nvm/versions/node/v22.21.0/bin"].as_slice());

        let dirs = dirs_with(&base, &found);
        assert_eq!(
            dirs,
            extra(
                [
                    "/home/u/.cargo/bin",
                    "/home/u/go/bin",
                    "/home/u/.nvm/versions/node/v22.21.0/bin",
                ]
                .as_slice()
            ),
            "the directory the binary came from is appended, and appended last so it can \
             shadow nothing the user arranged"
        );

        let scrub = vec![("PATH".to_string(), Some("/usr/bin".to_string()))];
        let path = path_of(child_path_in(&scrub, None, &dirs)).expect("directories were added");
        assert!(
            path.ends_with("/home/u/.nvm/versions/node/v22.21.0/bin"),
            "and it reaches the child: {path}"
        );
    }

    #[test]
    fn a_directory_already_searched_is_not_searched_twice_and_an_empty_one_is_dropped() {
        // The empty entry is the interesting half: in a `PATH` it means the current directory,
        // and a child's cwd is the one place cide must never put on a search path.
        let base = extra(["/home/u/.cargo/bin"].as_slice());
        assert_eq!(
            dirs_with(
                &base,
                &[
                    std::path::PathBuf::from("/home/u/.cargo/bin"),
                    std::path::PathBuf::new(),
                ]
            ),
            base
        );
    }

    #[test]
    fn a_scrub_that_removed_path_outright_still_leaves_the_child_the_extras() {
        // Every entry the variable had was inside the bundle, so the user had none of their own.
        // "None of those were yours" is not the same statement as "search nowhere".
        let scrub = vec![("PATH".to_string(), None)];
        let inherited = std::ffi::OsString::from("/tmp/.mount_cide_0OOoGFm/usr/bin");
        assert_eq!(
            path_of(child_path_in(
                &scrub,
                Some(&inherited),
                &extra(["/home/u/.cargo/bin"].as_slice())
            )),
            Some("/home/u/.cargo/bin".to_string())
        );
        // …but with nothing to add there is nothing to say, and the removal stands alone.
        assert_eq!(child_path_in(&scrub, Some(&inherited), &[]), None);
    }

    #[test]
    fn a_terminal_launch_sets_no_path_on_any_child() {
        // The `./run.sh` case and the whole reason Linux is unaffected in practice: every extra
        // is already on PATH, so nothing is emitted and no child's environment differs by a byte.
        let inherited = std::ffi::OsString::from("/home/u/.cargo/bin:/usr/bin");
        assert_eq!(
            child_path_in(
                &[],
                Some(&inherited),
                &extra(["/home/u/.cargo/bin"].as_slice())
            ),
            None
        );
    }

    #[test]
    fn a_path_that_is_not_utf8_is_left_alone_rather_than_mangled() {
        // `EnvChange` is String-shaped. A lossy conversion would corrupt a PATH that works
        // today, which is strictly worse than declining to improve it.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let inherited = std::ffi::OsString::from_vec(b"/usr/bin:/\xff\xfeodd".to_vec());
            assert_eq!(
                child_path_in(&[], Some(&inherited), &extra(["/home/u/go/bin"].as_slice())),
                None
            );
        }
    }

    /// The link in the chain that was read from `std`'s source and never executed.
    ///
    /// Every sentence above assumes that a `PATH` set on a `Command` is the `PATH` the kernel
    /// searches for a **bare** program name. It is true — `std::sys::process` swaps `environ`
    /// before `execvp` — but "true in the standard library I read" is not the same claim as
    /// "true in the standard library this binary links", and the whole fix rests on it. So it is
    /// executed: a directory that is *not* on the minimal `PATH`, a bare name in it, and the
    /// same spawn twice.
    ///
    /// The first half is the more valuable assertion. Without it a test that only checked the
    /// success case would still pass if the child were somehow resolving out of the *parent's*
    /// `PATH` — which is exactly the failure being fixed, and which would be invisible on a
    /// developer machine where the real `PATH` has everything on it.
    #[cfg(unix)]
    #[test]
    fn a_path_set_on_a_command_is_the_path_a_bare_program_name_is_resolved_on() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("cide-child-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let program = dir.join("cide-probe-not-a-real-binary");
        std::fs::write(&program, "#!/bin/sh\necho found\n").expect("write");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        // launchd's four directories, which is what a Finder-launched `.app` inherits.
        let minimal = "/usr/bin:/bin:/usr/sbin:/sbin";
        let run = |path: &str| {
            Command::new("cide-probe-not-a-real-binary")
                .env("PATH", path)
                .output()
        };

        assert!(
            run(minimal).is_err(),
            "the bare name resolved against something other than the PATH set on the Command — \
             every assumption in this module about giving a child a PATH is then wrong"
        );

        let appended = crate::toolchain::child_path_from(
            Some(std::ffi::OsStr::new(minimal)),
            std::slice::from_ref(&dir),
        )
        .expect("the directory was missing, so a PATH is built");
        let output = run(&appended.into_string().expect("utf-8")).expect("spawn");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "found");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- part two: arming --------------------------------------------------------------

    /// A pid that certainly names no process: spawn a child, reap it, reuse its pid. Inventing
    /// a large number risks hitting a real process and turning the assertion into a coin toss.
    fn a_dead_pid() -> u32 {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        child.wait().expect("reap");
        pid
    }

    #[test]
    fn the_spawn_thread_is_one_thread_and_it_is_not_the_caller() {
        // The whole value of `on_spawn_thread` is that the forking thread outlives the child,
        // which is only true if it is *the* long-lived thread rather than whichever pool
        // worker happened to call in.
        let first = on_spawn_thread(|| std::thread::current().id());
        let from_another_thread =
            std::thread::spawn(|| on_spawn_thread(|| std::thread::current().id()))
                .join()
                .expect("join");

        assert_eq!(
            first, from_another_thread,
            "spawns used two different threads"
        );
        assert_ne!(
            first,
            std::thread::current().id(),
            "the spawn ran on the caller's thread, which is exactly what must not happen"
        );
    }

    #[test]
    fn the_spawn_thread_returns_values_and_stays_usable() {
        assert_eq!(on_spawn_thread(|| 6 * 7), 42);
        assert_eq!(on_spawn_thread(|| "still here".to_string()), "still here");
    }

    #[test]
    fn a_job_that_panics_takes_only_itself_down_and_the_panic_reaches_its_caller() {
        // The thread is behind a `OnceLock` and cannot be replaced, so a job that unwound the
        // thread would leave `spawner()` handing out a dead `Sender` for the life of the
        // process: every pane spawned afterwards would fail, in every window, with a message
        // about `cide-spawn` rather than about whatever actually broke. This asserts both
        // halves — the panic is still the caller's to see, and the thread survives it.
        //
        // Ordered before the sibling assertion on purpose: if the fix regresses, the *second*
        // call is what hangs or panics, and a test that only checked the first would pass.
        let panicked = std::panic::catch_unwind(|| on_spawn_thread(|| panic!("a job blew up")));
        let payload = panicked.expect_err("the panic did not reach the caller");
        assert_eq!(
            payload.downcast_ref::<&str>().copied(),
            Some("a job blew up"),
            "the caller got a different panic from the one the job raised"
        );

        assert_eq!(
            on_spawn_thread(|| 6 * 7),
            42,
            "one panicking job ended spawning for the rest of the process"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_already_orphaned_child_signals_itself_rather_than_waiting_for_a_death_that_happened() {
        // The `getppid` race guard, which is the one branch of `set_parent_death_signal` a
        // unit test can reach without killing the test runner. It is also the branch most
        // likely to be dropped as redundant: without it, a child forked in the instant before
        // cide dies is armed against a corpse, the kernel has already run the death
        // notification, and the child runs for ever. Being right in the common case and
        // wrong in the race is exactly how an orphan survives a SIGKILL.
        //
        // `cfg(unix)` and not `cfg(target_os = "linux")`, which is what it used to say. On
        // macOS and the BSDs this branch is not one of two things the function does — it is
        // the *whole* of what the function can do, so it is the platform where dropping it
        // costs the most and the platform where nothing else would notice. Both arms call one
        // shared body, so this assertion covers both wherever it runs.
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        use std::time::Instant;

        // Not our parent, by construction — the pid has been reaped. The guard must see the
        // mismatch and raise `DEATH_SIGNAL` on itself before `exec`.
        let bogus_parent = a_dead_pid();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: as in `arm` — the closure is async-signal-safe and allocates nothing.
        unsafe {
            command.pre_exec(move || {
                set_parent_death_signal(bogus_parent, DEATH_SIGNAL);
                Ok(())
            });
        }

        let started = Instant::now();
        let status = command.spawn().expect("spawn").wait().expect("wait");

        assert!(
            !status.success() && started.elapsed().as_secs() < 10,
            "an armed child whose parent was already gone ran its command anyway \
             ({status:?} after {:?})",
            started.elapsed()
        );
    }
}

/// The settings-to-environment rule.
///
/// Kept apart from the bundle tests above because they share nothing but the return type, and
/// because these are the assertions that would have caught three switches wired to nothing.
#[cfg(test)]
mod claude_env_tests {
    use super::*;
    use cide_ipc::ClaudeSettings;

    fn value(changes: &[EnvChange], name: &str) -> Option<Option<String>> {
        changes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }

    /// Every variable the rule is responsible for, so a field added to `ClaudeSettings` and
    /// forgotten here shows up as a name this list knows and the output does not.
    const VARS: [&str; 4] = [
        "CLAUDE_CODE_SCROLL_SPEED",
        "CLAUDE_CODE_DISABLE_MOUSE",
        "CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT",
        "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN",
    ];

    #[test]
    fn every_toggle_reaches_the_environment_it_documents() {
        // The whole point. Each field is turned on one at a time so that a rule which happened
        // to read the *wrong* field would still be caught — an `alt_screen_full_repaint` that
        // secretly reports `disable_mouse`'s value passes any test that sets both at once.
        let on = ClaudeSettings {
            disable_mouse: true,
            ..Default::default()
        };
        assert_eq!(
            value(&claude_env(&on), "CLAUDE_CODE_DISABLE_MOUSE"),
            Some(Some("1".to_string())),
            "the mouse switch is the one a user presses when a TUI has taken their selection, \
             and it has to arrive in the child's environment to do anything at all"
        );
        assert_eq!(
            value(&claude_env(&on), "CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT"),
            Some(None),
            "and turning one switch on must not turn its neighbours on"
        );

        let on = ClaudeSettings {
            alt_screen_full_repaint: true,
            ..Default::default()
        };
        assert_eq!(
            value(&claude_env(&on), "CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT"),
            Some(Some("1".to_string()))
        );

        let on = ClaudeSettings {
            disable_alternate_screen: true,
            ..Default::default()
        };
        assert_eq!(
            value(&claude_env(&on), "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN"),
            Some(Some("1".to_string())),
            "this is the switch a user chasing a scrollable transcript reaches for, and it \
             spent its whole life so far reaching nothing"
        );
    }

    #[test]
    fn a_switch_that_is_off_removes_the_variable_rather_than_leaving_it_alone() {
        // The inherited-value case: a user whose shell profile exports one of these would
        // otherwise see the switch at `off` and the behaviour at `on`, for ever.
        let changes = claude_env(&ClaudeSettings::default());
        for var in VARS.iter().filter(|v| **v != "CLAUDE_CODE_SCROLL_SPEED") {
            assert_eq!(
                value(&changes, var),
                Some(None),
                "{var} is off by default, and off has to mean removed: an inherited value \
                 would make the Settings screen lie about what the child is doing"
            );
        }
    }

    #[test]
    fn an_off_switch_is_never_spelled_zero() {
        // `V.CLAUDE_CODE_DISABLE_MOUSE !== undefined` gates the read, and the truthiness test
        // after it treats the string "0" as true. Spelling "off" as `=0` would disable the
        // mouse from a switch that is off.
        let changes = claude_env(&ClaudeSettings::default());
        for (name, value) in &changes {
            if name == "CLAUDE_CODE_SCROLL_SPEED" {
                continue;
            }
            assert!(
                value.is_none(),
                "{name} was set to {value:?} to mean `off`; the CLI reads any defined value as \
                 on, so the only safe spelling of off is removal"
            );
        }
    }

    #[test]
    fn the_scroll_speed_is_clamped_into_the_range_the_cli_honours() {
        // Both ends, and the reason they differ: above 20 the CLI clamps to 20 anyway, so
        // sending more is merely useless; at or below 0 it *discards* the value and falls back
        // to 1 for an xterm.js renderer, so sending 0 would be actively worse than sending
        // nothing. The clamp exists for the second case.
        let speed = |n: u8| {
            value(
                &claude_env(&ClaudeSettings {
                    scroll_speed: n,
                    ..Default::default()
                }),
                "CLAUDE_CODE_SCROLL_SPEED",
            )
            .flatten()
        };
        assert_eq!(
            speed(0),
            Some("1".to_string()),
            "0 is a value the CLI throws away, and a thrown-away value scrolls slower than the default"
        );
        assert_eq!(speed(200), Some("20".to_string()));
        assert_eq!(
            speed(7),
            Some("7".to_string()),
            "and a value inside the range is passed through untouched"
        );
    }

    #[test]
    fn the_default_speed_is_the_constant_base_env_used_to_hardcode() {
        // Nobody who never opens Settings may notice this field appearing.
        assert_eq!(
            value(
                &claude_env(&ClaudeSettings::default()),
                "CLAUDE_CODE_SCROLL_SPEED"
            ),
            Some(Some("3".to_string()))
        );
    }

    #[test]
    fn the_rule_answers_for_every_variable_it_claims() {
        let changes = claude_env(&ClaudeSettings::default());
        let mut names: Vec<&str> = changes.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        let mut expected = VARS.to_vec();
        expected.sort_unstable();
        assert_eq!(
            names, expected,
            "a field added to ClaudeSettings whose environment variable never got a line here \
             is exactly the defect this module was written to end"
        );
    }
}

/// The `EDITOR` rule.
///
/// Apart from both modules above for [`claude_env_tests`]'s reason: what these hold is the set
/// of states in which cide must *decline* to name itself as the editor, and each of those was
/// found by asking what a spawn site could ship that is worse than the CLI's own guess.
#[cfg(test)]
mod editor_env_tests {
    use super::*;

    /// The three-line shape [`editor_env_in`] hands a spawn site, as (name, value) pairs.
    fn editor(
        exe: &str,
        socket: Option<&str>,
        inherited: Option<&str>,
    ) -> Vec<(String, Option<String>)> {
        editor_env_in(
            Some(std::path::Path::new(exe)),
            socket.map(std::path::Path::new),
            inherited.map(OsStr::new),
        )
    }

    #[test]
    fn the_command_is_this_binary_plus_the_wait_flag() {
        // The whole feature, spelled the one way the CLI can parse: it splits on a space and
        // execs the first field with the file appended.
        assert_eq!(
            editor(
                "/opt/cide/cide",
                Some("/run/user/1000/cide-edit-42.sock"),
                None
            ),
            vec![
                (
                    "CIDE_EDIT_SOCK".to_string(),
                    Some("/run/user/1000/cide-edit-42.sock".to_string())
                ),
                (
                    "EDITOR".to_string(),
                    Some("/opt/cide/cide --wait".to_string())
                ),
            ]
        );
    }

    #[test]
    fn the_command_and_the_socket_are_never_separable() {
        // A `cide --wait` with nowhere to report is an editor that fails every single time,
        // which is worse than the `code` guess it would have displaced. Neither variable may
        // reach a child without the other, in either direction.
        for changes in [
            editor("/opt/cide/cide", None, None),
            editor_env_in(None, Some(std::path::Path::new("/run/s.sock")), None),
        ] {
            assert!(
                changes.is_empty(),
                "half of the pair went out on its own: {changes:?}"
            );
        }
    }

    #[test]
    fn a_path_with_a_space_in_it_is_refused_rather_than_mangled() {
        // `$EDITOR` has no quoting: this value would name a program called `/Applications/My`.
        // macOS is where such a path is ordinary, which is why this is a refusal and not an
        // assertion that it cannot happen.
        assert_eq!(
            editor(
                "/Applications/My App.app/Contents/MacOS/cide",
                Some("/run/s.sock"),
                None
            ),
            Vec::new()
        );
    }

    #[test]
    fn an_editor_the_user_chose_survives() {
        // Someone with `EDITOR=nvim` has answered this question already, for `git commit` as
        // much as for Ctrl+G.
        assert_eq!(
            editor("/opt/cide/cide", Some("/run/s.sock"), Some("nvim")),
            Vec::new()
        );
    }

    #[test]
    fn an_empty_editor_is_not_a_preference() {
        // The CLI's own test is `if (V.EDITOR)`, on which "" is falsy: deferring to it would be
        // deferring to nothing.
        assert!(!editor("/opt/cide/cide", Some("/run/s.sock"), Some("")).is_empty());
    }
}
