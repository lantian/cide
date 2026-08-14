//! What every child of cide is given on its way out of `fork`: an environment with the bundle
//! filtered out of it, and a death signal tied to ours.
//!
//! # Two rules, one module
//!
//! Both halves answer the same question — *what does a spawn site here owe a child?* — and both
//! are wrong to skip, silently, in ways that surface three processes away. `CLAUDE.md` already
//! names this module as the one every `Command::new`/`SpawnSpec` in the workspace passes
//! through, so this is where the second rule belongs too.
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

/// Apply [`bundle_scrub`] to a [`Command`] that is about to be spawned.
///
/// For the children spawned with `std::process` — the `claude` one-shots, `git push`,
/// `claude --version`. PTY children take the same changes through `SpawnSpec`, in
/// `cide_app::cmd::session::base_env`, because this crate cannot see `cide-pty`'s types.
pub fn scrub_command(command: &mut Command) {
    for (name, value) in bundle_scrub() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
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
#[cfg(unix)]
pub fn arm(command: &mut Command) {
    use std::os::unix::process::CommandExt;

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
/// Public so that a spawner this crate does not own can use the same implementation. The one
/// that matters is `cide-pty`: `portable-pty`'s `CommandBuilder` exposes no `pre_exec` hook of
/// its own, so PTY panes cannot be armed from outside that crate.
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

    // The race the man page does not spell out: if the parent exited between the `fork` and
    // the line above, the death it was armed for has already happened and nothing will ever
    // deliver the signal. Re-parenting is the observable evidence — `getppid()` becomes 1, or
    // the nearest subreaper — so a mismatch here means "already orphaned", and the child does
    // to itself what the kernel now never will.
    //
    // SAFETY: both calls are bare syscalls with no arguments to get wrong.
    if unsafe { libc::getppid() } as u32 != expected_parent {
        unsafe { libc::raise(signal) };
    }
}

/// Other unixes have no `PR_SET_PDEATHSIG`. macOS and the BSDs would need `kqueue`'s
/// `NOTE_EXIT` and a supervising thread, which is a different design; cide targets Linux.
#[cfg(all(unix, not(target_os = "linux")))]
pub fn set_parent_death_signal(_expected_parent: u32, _signal: libc::c_int) {}

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

    #[cfg(target_os = "linux")]
    #[test]
    fn an_already_orphaned_child_signals_itself_rather_than_waiting_for_a_death_that_happened() {
        // The `getppid` race guard, which is the one branch of `set_parent_death_signal` a
        // unit test can reach without killing the test runner. It is also the branch most
        // likely to be dropped as redundant: without it, a child forked in the instant before
        // cide dies is armed against a corpse, the kernel has already run the death
        // notification, and the child runs for ever. Being right in the common case and
        // wrong in the race is exactly how an orphan survives a SIGKILL.
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
