//! An OS shutdown or reboot takes the same road out as a quit — and is told in time to.
//!
//! # Why a signal handler is not enough
//!
//! [`crate::lifecycle::install_signal_handlers`] catches the SIGTERM a shutdown ends in, and
//! runs the ordinary teardown from it. But on a systemd machine that SIGTERM is not addressed
//! to cide: the desktop starts cide as a transient `app-*.service` with
//! `KillMode=control-group`, and stopping that unit signals **every process in the cgroup at
//! once** — each `claude`, `opencode` and `codex` a run is driving, each language server, each
//! shell. The children die in parallel with the teardown rather than on its ladder, a
//! transcript can be cut mid-write, and five seconds later (`TimeoutStopUSec`, measured on the
//! machine this was written on) the whole cgroup is SIGKILLed whatever state it is in.
//! `AgentRegistry::external_stop` makes the *record* of that honest; it cannot make it gentle.
//!
//! logind can. A **delay inhibitor** asks logind to announce a shutdown before it starts one:
//! it emits `PrepareForShutdown(true)` and waits until every delay lock is released, or
//! `InhibitDelayMaxSec` (5 s by default) has passed, before it lets systemd stop anything. In
//! that window cide runs its *normal* quit — `app.exit`, `ExitRequested`, the deferred
//! teardown, the SIGHUP → SIGTERM → SIGKILL ladder cide itself drives — so a reboot leaves the
//! runs exactly as a quit does. The lock is handed back straight after the ladder
//! ([`release`]), not at process exit, so the language servers' own slow ladders never hold up
//! the machine.
//!
//! # What this does not cover
//!
//! A **logout** that is not a shutdown: logind has no `PrepareForLogout`, and the session's
//! units are simply stopped. That road stays on the signal thread and on `external_stop`, with
//! the teardown ordered so everything durable lands first inside the same five seconds.
//!
//! A shutdown **cancelled** after `PrepareForShutdown(true)` (the `false` edge) still quits
//! cide: the teardown has begun by then, and there is no un-quitting. Re-taking the lock for a
//! process that is on its way out would buy nothing.
//!
//! # Failure is quiet
//!
//! No system bus (a container, a non-systemd distribution), a logind that refuses the lock, a
//! policy that forbids it: each is one `warn` and no lock. Nothing here is required for a quit
//! to be correct — it is what makes an OS shutdown *as good as* a quit.

use std::sync::Mutex;

use tauri::AppHandle;

/// The delay lock, held for the life of the process and dropped by [`release`]. Closing the
/// descriptor is the whole of releasing it — logind watches the fd, not a method call — which
/// is also why process exit releases it with no help.
static INHIBITOR: Mutex<Option<zbus::zvariant::OwnedFd>> = Mutex::new(None);

/// The sentence `systemd-inhibit --list` and a desktop's "these apps are delaying shutdown"
/// dialog show beside cide's name.
const WHY: &str = "Saving Claude sessions and subagent runs";

/// Take the delay lock and watch for the shutdown it announces, on a thread of its own.
///
/// Called once from setup, right after the signal handlers. Returns at once; every failure is
/// logged and swallowed — see the module header.
pub fn watch(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("cide-logind".into())
        .spawn(move || {
            if let Err(error) = run(&app) {
                tracing::warn!(
                    %error,
                    "no logind shutdown inhibitor; an OS shutdown will SIGTERM the runs directly"
                );
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the logind thread");
    }
}

/// Hand the delay lock back: the children are down, and nothing the rest of the teardown does
/// is worth making the machine wait for. Idempotent.
pub fn release() {
    if let Ok(mut held) = INHIBITOR.lock()
        && held.take().is_some()
    {
        tracing::info!("released the logind shutdown inhibitor");
    }
}

fn run(app: &AppHandle) -> zbus::Result<()> {
    let bus = zbus::blocking::Connection::system()?;
    let manager = zbus::blocking::Proxy::new(
        &bus,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )?;
    // Subscribed **before** the lock is taken, so there is no instant in which cide holds a
    // lock whose announcement it could miss — logind would then wait out the whole delay on
    // a process that never answers.
    let announcements = manager.receive_signal("PrepareForShutdown")?;
    let fd: zbus::zvariant::OwnedFd =
        manager.call("Inhibit", &("shutdown", "cide", WHY, "delay"))?;
    if let Ok(mut held) = INHIBITOR.lock() {
        *held = Some(fd);
    }
    tracing::debug!("holding a logind shutdown delay inhibitor");

    for message in announcements {
        let starting: bool = match message.body().deserialize() {
            Ok(starting) => starting,
            Err(error) => {
                tracing::debug!(%error, "an unreadable PrepareForShutdown; ignored");
                continue;
            }
        };
        if !starting {
            continue;
        }
        tracing::info!("logind announced a shutdown; quitting the way a quit does");
        // First, for the reason `lifecycle::begin_going_down` gives: from here on a child's
        // death is an interruption and nothing new is forked.
        crate::lifecycle::begin_going_down(app);
        // A message to the event loop, safe off the main thread (see the signal thread, which
        // does the same). It raises `ExitRequested`, and from there the path is the quit's own.
        app.exit(0);
        break;
    }
    Ok(())
}
