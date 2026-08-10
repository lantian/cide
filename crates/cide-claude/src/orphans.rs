//! What survives a `SIGKILL` of cide, and what must not.
//!
//! A clean quit runs `lifecycle::shutdown`, which kills every child and drops every handle.
//! A `SIGKILL` — `kill -9`, an OOM kill, a compositor tearing the session down — runs
//! nothing. What is left behind, measured rather than guessed:
//!
//! * every PTY child keeps running. `claude` is a session leader in its own process group
//!   (`portable-pty` calls `setsid` before `exec`), so it does not even get a `SIGHUP` when
//!   the master fd closes; it sits on a dead terminal holding a subscription slot until the
//!   user finds it in `ps`.
//! * `$XDG_RUNTIME_DIR/cide-hooks-<pid>.sock` stays on disk. `HookServer`'s `Drop` is the
//!   only thing that removes it and `Drop` does not run.
//!
//! This module is the answer to both: [`arm`] hands a child to the kernel to kill, and
//! [`sweep_hook_sockets`] clears what a previous hard kill left behind.
//!
//! # `PR_SET_PDEATHSIG`, and the three facts that decide how it must be used
//!
//! All three are from `prctl(2)` on Linux 6.x, and all three have a failure mode that is
//! silent rather than loud:
//!
//! 1. **It is cleared for the child of `fork(2)`** — `copy_process()` does
//!    `p->pdeath_signal = 0`. So arming the *parent* and then spawning achieves nothing at
//!    all. It must be set in the child, between `fork` and `exec`, which on Linux means
//!    `CommandExt::pre_exec`. (The brief for this work said the setting is inherited across
//!    `fork`; it is not, and building on that would have shipped a no-op that tests clean.)
//! 2. **It survives `execve(2)`** — except when the image being executed is set-user-ID,
//!    set-group-ID, or carries file capabilities, in which case the kernel clears it along
//!    with the rest of the privileged-exec cleanup. `claude` is none of those, but a user's
//!    `$SHELL` is theoretically capable of being one, so this is a degradation and not a
//!    guarantee. Surviving `exec` is also what makes the whole approach work: we set it in a
//!    forked child that is about to become `claude`.
//! 3. **The "parent" is the *thread* that forked, not the process.** The signal is delivered
//!    when that thread exits, even with the process very much alive. A pane spawned from a
//!    short-lived worker thread would therefore be killed seconds later, for no reason a user
//!    could ever diagnose. [`on_spawn_thread`] exists solely to remove that possibility: it
//!    runs the spawn on one thread that lives as long as the process does.
//!
//! There is a fourth, smaller trap, handled in [`set_parent_death_signal`]: if the parent
//! dies *between* the `fork` and the `prctl`, the signal is armed against a parent that is
//! already gone and will never be delivered. The child re-checks its own `getppid()` against
//! the pid it was told to expect and raises the signal on itself if they differ.
//!
//! # Why `SIGTERM` and not `SIGKILL`
//!
//! `SIGTERM` gives `claude` its normal exit path — flushing a transcript, releasing its
//! subscription slot — and a shell pane the chance to finish a `write(2)` into one of the
//! user's files. `SIGKILL` would guarantee the child dies, which is only worth having if a
//! child might ignore `SIGTERM`; neither program does. The cost of being wrong the other way
//! is a truncated file in the user's repository, so this errs towards the polite signal.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

/// The signal a child is sent when cide dies. See the module docs for why not `SIGKILL`.
#[cfg(unix)]
pub const DEATH_SIGNAL: libc::c_int = libc::SIGTERM;

// --- arming a child ----------------------------------------------------------------------

/// Arrange for `command`'s child to be signalled when this process dies.
///
/// Installs a `pre_exec` hook, so the `prctl` happens in the forked child where it counts and
/// then survives the `exec` into `claude`. A failure to arm is *not* fatal — the hook returns
/// `Ok(())` regardless — because a child that outlives a crash is strictly better than a pane
/// that refuses to open. The `prctl` cannot fail for a valid signal number in practice; the
/// tolerance is for the seccomp-filtered and non-Linux cases.
///
/// **The calling thread must outlive the child.** That is fact 3 in the module docs, and it
/// is the reason [`on_spawn_thread`] exists.
#[cfg(unix)]
pub fn arm(command: &mut std::process::Command) {
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

/// Non-Linux, non-unix: nothing to arm.
#[cfg(not(unix))]
pub fn arm(_command: &mut std::process::Command) {}

/// Arm the **calling** process to be signalled when `expected_parent` dies.
///
/// Called from inside a `pre_exec` hook, which is why it takes the parent's pid rather than
/// reading it: `getppid()` here is the pid to *compare against*, not the value to trust.
///
/// Public so that a spawner this crate does not own can use the same implementation. The one
/// that matters is `cide-pty`: `portable-pty`'s `CommandBuilder` exposes no `pre_exec` hook of
/// its own, so PTY panes cannot be armed from outside that crate — see the report accompanying
/// this change.
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

// --- the spawning thread -----------------------------------------------------------------

/// Run `f` on the one thread that spawns children, and return what it returned.
///
/// This exists for exactly one reason, and it is fact 3 from the module docs: `PDEATHSIG` is
/// delivered when the **thread** that forked the child exits, not when the process does. A
/// Tauri command handler runs on a pooled worker thread that can be retired at any time, so
/// arming a pane spawned from one would give the user a `claude` that dies a few seconds after
/// it started, with nothing anywhere to say why. The thread this uses is created once, is
/// never joined, and therefore outlives every child it makes.
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
/// mistake the type system already makes awkward — but whoever threads this into `cide-pty`
/// should know it is the one shape that deadlocks.
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

    // Both `expect`s are now genuinely unreachable rather than merely unlikely: the only way
    // the thread could stop serving was a job that unwound, and jobs no longer unwind.
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

// --- sweeping abandoned hook sockets ------------------------------------------------------

/// The filename prefix `cide-app::hooks` publishes under. The pid follows it.
pub const HOOK_SOCKET_PREFIX: &str = "cide-hooks-";

/// Where hook sockets live: `$XDG_RUNTIME_DIR`, or the temp directory when it is unset.
///
/// Mirrors `cide_app::hooks::socket_path` exactly, and the duplication is deliberate rather
/// than an oversight: this crate must not depend on the Tauri shell, and the alternative —
/// having the shell pass its own directory in — would leave the sweep unable to run before
/// the shell exists, which is the only moment it is useful.
pub fn hook_socket_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// The socket path for a given pid.
pub fn hook_socket_path(pid: u32) -> PathBuf {
    hook_socket_dir().join(format!("{HOOK_SOCKET_PREFIX}{pid}.sock"))
}

/// Remove `cide-hooks-*.sock` files whose owning process is gone. Returns how many went.
///
/// Called at startup, before the new listener binds. Mirrors
/// [`cide_ide_mcp::lockfile::sweep_stale`] in shape and in caution, and reuses its liveness
/// rule rather than restating it — see [`cide_ide_mcp::lockfile::pid_is_alive`].
///
/// The rules, and why each one is not merely tidiness:
///
/// * the name must be exactly `cide-hooks-<digits>.sock`. The fallback directory is the shared
///   temp directory, so "starts with `cide-`" is not specific enough to delete on.
/// * the pid must be *established* dead, ESRCH and nothing else. `EPERM` is another user's
///   live cide in a shared `/tmp`, and unlinking its socket takes hooks away from a running
///   application that has no idea we exist.
/// * our own socket is protected by the same rule for free: this process is alive.
///
/// A missed sweep costs one stale file; a wrong sweep silently deletes a live control channel.
/// The bias is towards leaving files behind.
pub fn sweep_hook_sockets() -> usize {
    sweep_hook_sockets_in(&hook_socket_dir())
}

/// [`sweep_hook_sockets`] against an explicit directory, so tests never point at the real one.
pub fn sweep_hook_sockets_in(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let Some(pid) = socket_pid(name) else {
            continue;
        };
        if pid_is_alive(pid) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            // Another user's file in a shared `/tmp`, or a race with a second sweep. Neither
            // is worth more than a line.
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "could not remove a stale hook socket");
            }
        }
    }
    removed
}

/// The pid in a hook socket's filename, if this is one of ours.
///
/// A named function with tests rather than a `strip_prefix` chain inline, because every way of
/// getting it slightly wrong widens what the sweep will delete.
fn socket_pid(name: &str) -> Option<u32> {
    let digits = name
        .strip_prefix(HOOK_SOCKET_PREFIX)?
        .strip_suffix(".sock")?;
    // `parse::<u32>` accepts a leading `+`, which would make `cide-hooks-+7.sock` a candidate
    // for deleting pid 7's socket under a name that process never wrote.
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The single liveness rule, borrowed rather than reimplemented.
///
/// Re-exported through a private alias only so the two call sites above read cleanly; the
/// implementation, and the ESRCH-versus-EPERM reasoning behind it, lives in
/// [`cide_ide_mcp::lockfile::pid_is_alive`] and must stay there.
fn pid_is_alive(pid: u32) -> bool {
    cide_ide_mcp::lockfile::pid_is_alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pid that certainly names no process: spawn a child, reap it, reuse its pid. Inventing
    /// a large number risks hitting a real process and turning the assertion into a coin toss.
    fn a_dead_pid() -> u32 {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        child.wait().expect("reap");
        pid
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-orphans-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_socket_from_a_dead_process_is_swept_and_a_live_one_is_kept() {
        let dir = temp_dir("sweep");
        let dead = dir.join(format!("cide-hooks-{}.sock", a_dead_pid()));
        let live = dir.join(format!("cide-hooks-{}.sock", std::process::id()));
        fs::write(&dead, "").expect("write");
        fs::write(&live, "").expect("write");

        assert_eq!(sweep_hook_sockets_in(&dir), 1);
        assert!(!dead.exists(), "a socket with no process was left behind");
        assert!(
            live.exists(),
            "the sweep deleted the socket of a running cide — this one"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_else_in_the_directory_is_a_candidate() {
        // The fallback directory is the shared temp directory. Every name below is one a
        // loose prefix match would have deleted.
        let dir = temp_dir("shape");
        let dead = a_dead_pid();
        let keep = [
            format!("cide-hooks-{dead}.sock.bak"),
            format!("cide-hooks-{dead}"),
            format!("cide-hooks-+{dead}.sock"),
            "cide-hooks-.sock".to_string(),
            "cide-hooks-notapid.sock".to_string(),
            format!("cide-ide-{dead}.sock"),
            format!("{dead}.sock"),
        ];
        for name in &keep {
            fs::write(dir.join(name), "").expect("write");
        }

        assert_eq!(sweep_hook_sockets_in(&dir), 0);
        for name in &keep {
            assert!(dir.join(name).exists(), "{name} was treated as ours");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_name_this_crate_parses_is_the_name_the_app_writes() {
        // `cide_app::hooks::socket_path` formats `cide-hooks-{pid}.sock`. If the two ever
        // drift the sweep silently stops matching anything, which looks exactly like a sweep
        // that has nothing to do.
        let path = hook_socket_path(4242);
        let name = path.file_name().expect("named").to_str().expect("utf-8");
        assert_eq!(name, "cide-hooks-4242.sock");
        assert_eq!(socket_pid(name), Some(4242));
    }

    #[test]
    fn sweeping_a_directory_that_does_not_exist_is_not_an_error() {
        // `XDG_RUNTIME_DIR` can name a directory that has not been created yet on a fresh
        // login, and startup must not care.
        assert_eq!(
            sweep_hook_sockets_in(Path::new("/nonexistent/cide-orphans")),
            0
        );
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
        use std::process::{Command, Stdio};
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
