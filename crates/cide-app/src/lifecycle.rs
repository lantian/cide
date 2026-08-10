//! Quitting without losing anything, and coming back.
//!
//! Two halves of one story. On the way out: flush the workspace, then walk the children
//! down a SIGHUP → SIGTERM → SIGKILL ladder. On the way back: decide, pane by pane, whether
//! the conversation can be picked up where it left off.
//!
//! The ladder is not manners. A `claude` asked politely finishes writing its transcript;
//! one killed outright can leave it half-written — and that transcript is exactly what
//! [`plan_restore`] looks for. An impatient shutdown is what makes the next launch unable
//! to resume.
//!
//! cide keeps **no background daemon**. Quitting quits its Claude sessions. What survives a
//! quit is the *workspace* — windows, tabs, splits — and the conversations, which resume
//! because a pane's `SessionId` **is** the value passed to `claude --session-id`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use cide_core::workspace;
use cide_ipc::{
    Pane, PaneId, PaneKind, Project, ProjectId, SessionId, SessionState, TabId, WindowLabel,
    WindowRole, Workspace,
};
use cide_pty::{Exit, PtySession};
use tauri::{AppHandle, Manager};

use crate::state::SessionRegistry;
use crate::workspace_state::WorkspaceState;

/// Set the first time [`shutdown`] runs, because it can be reached twice: the signal thread
/// calls it, then asks the event loop to exit, which raises `RunEvent::ExitRequested`.
/// Running the ladder a second time would signal children that are already reaped.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// Guards against a second signal thread, which would fight the first over the same
/// children.
static SIGNALS_INSTALLED: AtomicBool = AtomicBool::new(false);

/// How long the signal thread gives the event loop to exit on its own before ending the
/// process itself.
///
/// The polite route is worth trying because plugin teardown runs there — window geometry is
/// saved from `RunEvent::Exit`. It is not worth waiting on indefinitely: a wedged GTK loop
/// is one of the reasons a user reaches for `kill` in the first place, and a SIGTERM that
/// does not terminate is a bug in us, not in them.
const EXIT_DEADLINE: Duration = Duration::from_millis(1_500);

// --- shutdown -------------------------------------------------------------------------

/// Flush durable state, then bring the children down.
///
/// Called from the signal thread and from `RunEvent::ExitRequested`; idempotent, so both
/// firing costs nothing. Blocks for as long as the ladder takes, which on the main thread
/// means the event loop stalls until the last child is gone — acceptable only because the
/// process is on its way out.
pub fn shutdown(app: &AppHandle) {
    if SHUTTING_DOWN.swap(true, Ordering::SeqCst) {
        return;
    }

    // Unconditionally, not `flush_if_due`: waiting out a 500 ms debounce on the way to exit
    // is how the user's last change gets lost.
    match app.try_state::<WorkspaceState>() {
        Some(state) => state.write_now(),
        // `try_state` rather than `state`, which panics. Reaching here means shutdown ran
        // before the app was managed, and losing a layout is better than a panic in a
        // signal thread.
        None => tracing::error!("no workspace state to flush during shutdown"),
    }

    // Before the children are signalled. A `claude` blocked on `openDiff` has to receive its
    // rejection over a socket that is still open; killing it first would end the turn with a
    // transport error where a plain "the user did not accept it" was available, and killing
    // it *after* would leave the rejection racing the SIGKILL.
    if let Some(servers) = app.try_state::<crate::ide::IdeServers>() {
        servers.stop_all();
    }

    let Some(registry) = app.try_state::<SessionRegistry>() else {
        tracing::error!("no session registry during shutdown; children may outlive the app");
        return;
    };
    let children: Vec<Arc<PtySession>> = registry
        .ids()
        .into_iter()
        .filter_map(|id| registry.get(id))
        .collect();
    stop_children(&children, Ladder::default());
}

/// Catch SIGTERM, SIGINT and SIGHUP and run the same shutdown path as a quit from the UI.
///
/// `RunEvent::ExitRequested` covers a window close and a quit from the UI and nothing else:
/// a SIGTERM from `pkill`, a session logout or a container stop kills the process without
/// the event loop ever seeing it, and the last debounce window goes with it. Measured, not
/// assumed.
#[cfg(unix)]
pub fn install_signal_handlers(app: &AppHandle) {
    use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;

    if SIGNALS_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }

    let mut signals = match Signals::new([SIGTERM, SIGINT, SIGHUP]) {
        Ok(signals) => signals,
        Err(error) => {
            tracing::error!(%error, "could not install signal handlers; a SIGTERM will lose unsaved layout");
            return;
        }
    };

    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("cide-signals".into())
        .spawn(move || {
            // Nothing below runs *in* the handler. A signal handler is entitled to almost
            // nothing — no allocation, no locks, no logging — and taking the workspace mutex
            // from one deadlocks if the interrupted thread already held it. signal-hook's
            // handler does the single legal thing, writing a byte to a self-pipe; this
            // ordinary thread wakes on the read side, where all of it is safe.
            let Some(signal) = signals.forever().next() else {
                return;
            };
            tracing::info!(signal, "shutting down on a signal");
            shutdown(&app);

            // Ask the event loop to exit so plugins get their teardown, then make sure of
            // it. `exit` is a message to the loop, not a call into it, so this is safe off
            // the main thread.
            app.exit(0);
            thread::sleep(EXIT_DEADLINE);
            // 128 + n is the shell's convention for "died of signal n". An init system that
            // reads the status to tell a clean stop from a crash gets the truth.
            std::process::exit(128 + signal);
        });

    if let Err(error) = spawned {
        SIGNALS_INSTALLED.store(false, Ordering::SeqCst);
        tracing::error!(%error, "could not start the signal thread");
    }
}

/// No-op off unix. Windows signals a console control handler instead, which is not wired
/// because Linux is the supported target — see `docs/adr/0006-linux-window-and-graphics.md`.
#[cfg(not(unix))]
pub fn install_signal_handlers(_app: &AppHandle) {}

/// One rung of the shutdown ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    /// The terminal hung up. What closing a terminal emulator does, and the gentlest thing
    /// that means "stop".
    Hup,
    /// Terminate. Still catchable, so a child that wants to flush state can.
    Term,
    /// Uncatchable. Whatever the child had not written is lost.
    Kill,
}

/// A child that can be asked, then told, then made to stop.
///
/// A trait rather than a concrete `PtySession` so the ladder's ordering can be tested
/// against a fake: the real rungs are process signals, which a test cannot observe without
/// spawning something that outlives its assertion.
pub trait Stoppable {
    fn signal(&self, rung: Rung);
    fn has_exited(&self) -> bool;
}

impl Stoppable for PtySession {
    fn signal(&self, rung: Rung) {
        // Every rung, the last one included, goes to the child's process *group*.
        //
        // Not through `PtySession::kill`, for any rung: portable-pty's `ChildKiller` sends
        // SIGHUP whatever it is asked for, and only to the leader. Routed through it the
        // top rung is a second SIGHUP, so a session that ignored the first two outlives the
        // app — and so does everything it spawned, with the pty closed under it.
        if let Some(pid) = self.child_pid() {
            deliver(pid, rung);
        }
    }

    fn has_exited(&self) -> bool {
        PtySession::has_exited(self)
    }
}

impl<T: Stoppable + ?Sized> Stoppable for Arc<T> {
    fn signal(&self, rung: Rung) {
        (**self).signal(rung);
    }

    fn has_exited(&self) -> bool {
        (**self).has_exited()
    }
}

/// How long each rung waits before escalating.
///
/// A normal quit never pays these: the children are gone within the first poll and
/// [`stop_children`] returns. They are the budget for a session that will not go — long
/// enough for `claude` to finish writing a transcript, short enough that quitting does not
/// feel hung.
#[derive(Debug, Clone, Copy)]
pub struct Ladder {
    pub hup_grace: Duration,
    pub term_grace: Duration,
    /// How long to wait for a killed child to be reaped before giving up on it. SIGKILL
    /// cannot be refused, so this only covers the reaper thread noticing.
    pub kill_grace: Duration,
    pub poll: Duration,
}

impl Default for Ladder {
    fn default() -> Self {
        Self {
            hup_grace: Duration::from_millis(750),
            term_grace: Duration::from_millis(1_500),
            kill_grace: Duration::from_millis(250),
            poll: Duration::from_millis(20),
        }
    }
}

/// Walk `children` down the ladder, stopping as soon as they have all exited.
///
/// Each rung is delivered only to children still running, so a session that goes on SIGHUP
/// is never sent a SIGTERM.
pub fn stop_children<C: Stoppable>(children: &[C], ladder: Ladder) {
    for (rung, grace) in [
        (Rung::Hup, ladder.hup_grace),
        (Rung::Term, ladder.term_grace),
    ] {
        if !signal_survivors(children, rung) {
            return;
        }
        if wait_for_exit(children, grace, ladder.poll) {
            return;
        }
    }

    if signal_survivors(children, Rung::Kill) {
        // Worth a line in the log: this is the case where the next launch may find a
        // transcript that Claude Code will not resume.
        tracing::warn!("a session ignored SIGHUP and SIGTERM and was killed outright");
        wait_for_exit(children, ladder.kill_grace, ladder.poll);
    }
}

/// Deliver `rung` to every child still running. Returns whether there were any.
fn signal_survivors<C: Stoppable>(children: &[C], rung: Rung) -> bool {
    let mut any = false;
    for child in children.iter().filter(|c| !c.has_exited()) {
        child.signal(rung);
        any = true;
    }
    any
}

/// Wait up to `grace` for every child to exit. Returns whether they all did.
fn wait_for_exit<C: Stoppable>(children: &[C], grace: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if children.iter().all(Stoppable::has_exited) {
            return true;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        thread::sleep(poll.min(remaining));
    }
}

/// Send a signal to the child's **process group**, falling back to the process itself.
///
/// The group is what matters. `portable-pty` starts the child in a new session, so it leads
/// a process group holding everything it spawned — a bash tool invocation, an MCP server —
/// and signalling the leader alone leaves those running with the pty closed under them.
/// `kill(-pid)` fails when the child never became a group leader, hence the fallback.
#[cfg(unix)]
fn deliver(pid: u32, rung: Rung) {
    let signal = match rung {
        Rung::Hup => libc::SIGHUP,
        Rung::Term => libc::SIGTERM,
        Rung::Kill => libc::SIGKILL,
    };
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

/// No-op off unix, which makes the whole ladder one off unix. That matches
/// [`install_signal_handlers`]: Linux is the supported target, and a Windows build would
/// need a job object rather than a translation of `kill(2)`.
#[cfg(not(unix))]
fn deliver(_pid: u32, _rung: Rung) {}

// --- exit -------------------------------------------------------------------------------

/// The code reported for a child whose status could not be read at all.
///
/// Re-exported rather than redefined: the only place that can know is `cide-pty`'s reaper,
/// which is where the constant now lives. This alias exists so the name still reads as part
/// of this module's vocabulary — [`report_exit`] is the one thing that publishes it.
pub use cide_pty::UNKNOWN_EXIT_CODE;

/// Report a session's death exactly once, with the status the reaper actually saw.
///
/// Two things happen when a child goes:
///
/// * `SessionState::Exited` is emitted, which is what lets a pane print `— exited —` from an
///   event instead of polling for it; and
/// * the hook server *forgets* the session. Without that, a `claude` that dies mid-turn is
///   remembered as `Busy` for the rest of the process's life, so the close confirm warns
///   about interrupting a session that no longer exists — and warning about nothing is how
///   users learn to dismiss that dialog unread.
///
/// A callback on `cide-pty`'s reaper thread, not a watcher thread of our own. The previous
/// version spawned one thread per session to poll `has_exited()` every 200 ms, and it could
/// not do better than [`UNKNOWN_EXIT_CODE`]: by the time a poll noticed, `wait()` had already
/// consumed the status and `waitpid` on that pid answers `ECHILD`. `PtySession::on_exit`
/// hands the status over at the one moment it exists, and costs no thread and no wakeups.
///
/// Registration cannot lose the race, either: a child that is already reaped calls back
/// immediately rather than never — see [`cide_pty::PtySession::on_exit`].
pub fn watch_for_exit(app: AppHandle, id: SessionId, session: &Arc<PtySession>) {
    watch_exit_with(session, move |exit| report_exit(&app, id, &exit));
}

/// The registration, with the reporting left to the caller.
///
/// Split out purely so a test can drive it: `report_exit` needs an `AppHandle`, and this
/// build cannot make one — `tauri`'s mock app is behind a feature it does not enable, for
/// the same reason `hooks.rs` keeps its logic in free functions. Everything above this line
/// that could be wrong now has a test; what is left untested is the one line of
/// [`watch_for_exit`] that composes the two, and `emit` itself.
fn watch_exit_with(session: &Arc<PtySession>, report: impl FnOnce(Exit) + Send + 'static) {
    session.on_exit(report);
}

/// What the frontend is told about a session that has ended.
///
/// A named function rather than an inline literal at the emit below, because this value *is*
/// the change: it used to be a fabricated [`UNKNOWN_EXIT_CODE`] for every dead pane whatever
/// had happened, and a test that rebuilt `SessionState::Exited { code: exit.code }` by hand
/// to compare against would assert nothing about the code that ships.
fn exit_state(exit: &Exit) -> SessionState {
    SessionState::Exited { code: exit.code }
}

/// Announce that `id` has ended, on `cide-pty`'s reaper thread.
fn report_exit(app: &AppHandle, id: SessionId, exit: &Exit) {
    // Worth a line, and worth it at `warn`: a session that ended badly is the one the user
    // comes asking about ("it just vanished"), and by then the pane has been closed and the
    // log is the only thing left that can answer. A clean exit is the ordinary case and says
    // nothing at info level either, because every quit produces one per pane.
    if is_abnormal(exit) {
        match &exit.signal {
            Some(signal) => {
                tracing::warn!(session = %id, code = exit.code, %signal, "a session was killed")
            }
            None => tracing::warn!(session = %id, code = exit.code, "a session exited non-zero"),
        }
    }

    // The hook server first. The emit is what wakes the frontend, and a window that reacted
    // by asking which sessions are live must not be told this one still is.
    if let Some(hooks) = app.try_state::<crate::hooks::HookServer>() {
        hooks.forget(id);
    }

    // The session stays in the registry. Its screen mirror is the last thing the child
    // printed, and a pane that is re-mounted, re-docked or rehydrated after the exit still
    // has to be able to paint it — removing the entry would turn `session_scrollback` into
    // `NoSuchSession` and leave the pane blank instead of showing what happened.
    crate::emit::session_state(app, &id.to_string(), exit_state(exit));
}

/// Whether an exit is worth telling the log about.
///
/// Anything that is not a clean zero — a non-zero status, a signal, or a status nobody could
/// read. Deliberately not "was it signalled": `claude` exiting 1 because it hit an error is
/// exactly as interesting to someone whose pane vanished as `claude` being OOM-killed, and
/// the two used to be indistinguishable anyway.
fn is_abnormal(exit: &Exit) -> bool {
    !exit.is_success()
}

// --- restore --------------------------------------------------------------------------

// `PaneRestore` and `SessionRestore` live in `cide-ipc` with every other wire type, so
// `cargo xtask codegen` emits them and the frontend consumes the generated definitions
// rather than a hand-written copy that would drift.
pub use cide_ipc::{PaneRestore, SessionRestore};

/// Decide what to do with every pane in the workspace.
pub fn plan_restore(ws: &Workspace) -> Vec<PaneRestore> {
    plan_restore_in(ws, claude_projects_dir().as_deref())
}

fn plan_restore_in(ws: &Workspace, projects_dir: Option<&Path>) -> Vec<PaneRestore> {
    let mut plan = Vec::new();

    for (id, project) in &ws.projects {
        // `validate` forbids a rootless project, but a restore plan is the wrong place to
        // discover that: skipping one project still lets every other one come back.
        let Some(root) = project.roots.first() else {
            tracing::warn!(project = %id, "project has no root; skipping it in the restore plan");
            continue;
        };
        let shell = shell_window(ws, *id);

        for tab in &project.tabs {
            let Some(window) = detached_tab_window(ws, tab.id).or_else(|| shell.clone()) else {
                continue;
            };
            for pane in tab.tree.panes.values() {
                if let Some(entry) = entry_for(
                    pane,
                    window.clone(),
                    project,
                    tab.id,
                    &root.path,
                    projects_dir,
                ) {
                    plan.push(entry);
                }
            }
        }

        // A torn-out pane waits in `detached` because a tab's tree cannot hold a pane with
        // no leaf. Its window names the tab it re-docks into.
        for pane in project.detached.values() {
            let Some((window, tab)) = detached_pane_window(ws, pane.id) else {
                continue;
            };
            if let Some(entry) = entry_for(pane, window, project, tab, &root.path, projects_dir) {
                plan.push(entry);
            }
        }
    }

    plan
}

/// One plan entry, or `None` for a pane that runs no process at all.
fn entry_for(
    pane: &Pane,
    window: WindowLabel,
    project: &Project,
    tab: TabId,
    cwd: &Path,
    projects_dir: Option<&Path>,
) -> Option<PaneRestore> {
    // A diff or an editor pane has nothing to spawn, and listing it would leave the
    // frontend to filter out entries it can only ignore.
    if !matches!(pane.kind, PaneKind::Claude | PaneKind::Shell) {
        return None;
    }
    Some(PaneRestore {
        window,
        project: project.id,
        tab,
        pane: pane.id,
        kind: pane.kind,
        cwd: cwd.to_path_buf(),
        restore: restore_for(pane, cwd, projects_dir),
        eager: pane.session == Some(project.primary_session),
    })
}

fn restore_for(pane: &Pane, cwd: &Path, projects_dir: Option<&Path>) -> SessionRestore {
    // A shell's scrollback died with its process. There is nothing to resume and nothing to
    // replay, and the frontend says so rather than showing a stale screen.
    if pane.kind != PaneKind::Claude {
        return SessionRestore::Fresh;
    }
    let Some(session) = pane.session else {
        return SessionRestore::Fresh;
    };
    match projects_dir {
        Some(dir) if transcript_exists(dir, cwd, session) => SessionRestore::Resumable { session },
        _ => SessionRestore::Fresh,
    }
}

/// The shell window showing `project`, if one does.
fn shell_window(ws: &Workspace, project: ProjectId) -> Option<WindowLabel> {
    workspace::windows_for_project(ws, project)
        .into_iter()
        .find(|label| matches!(ws.windows.get(label), Some(WindowRole::Shell { .. })))
}

fn detached_tab_window(ws: &Workspace, tab: TabId) -> Option<WindowLabel> {
    ws.windows.iter().find_map(|(label, role)| match role {
        WindowRole::DetachedTab { tab: t, .. } if *t == tab => Some(label.clone()),
        _ => None,
    })
}

/// The window a torn-out pane lives in, with the tab it re-docks into.
fn detached_pane_window(ws: &Workspace, pane: PaneId) -> Option<(WindowLabel, TabId)> {
    ws.windows.iter().find_map(|(label, role)| match role {
        WindowRole::DetachedPane { pane: p, tab, .. } if *p == pane => Some((label.clone(), *tab)),
        _ => None,
    })
}

/// Where Claude Code keeps its transcripts.
///
/// `CLAUDE_CONFIG_DIR` relocates `~/.claude` wholesale; honouring it is the difference
/// between a working restore and "nothing is resumable" for anyone who sets it.
///
/// See [`transcript_exists`] for what may and may not be done with the result.
fn claude_projects_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".claude"),
    };
    Some(base.join("projects"))
}

/// Whether Claude Code holds a transcript for `session`, started in `cwd`.
///
/// `<projects>/<encoded cwd>/<session>.jsonl` is an **internal Claude Code implementation
/// detail**, not an interface anyone has promised to keep. This is the only place in cide
/// that depends on it, and it depends on as little of it as it can: it asks the filesystem
/// whether one path exists and never opens, parses, writes or lists anything under there.
///
/// Every way of being wrong — a layout change, an encoding we guessed at, a relocated home
/// — answers `false`, and `false` costs the user a pane that opens fresh instead of resumed.
/// A wrong `true` would cost a spawn that fails on launch, so the bias is deliberate. For
/// the same reason there is no search for the file elsewhere: a transcript filed under a
/// different directory is one `claude --resume` will not find from this `cwd` either, so
/// finding it here would promise a resume that cannot happen.
fn transcript_exists(projects_dir: &Path, cwd: &Path, session: SessionId) -> bool {
    let encoded: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    projects_dir
        .join(encoded)
        .join(format!("{session}.jsonl"))
        .is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use cide_core::layout;
    use cide_ipc::{Axis, PaneRole, Side};

    /// Records the rungs it was sent and exits at the one it was told to.
    struct FakeChild {
        received: Mutex<Vec<Rung>>,
        dies_at: Option<Rung>,
        exited: Mutex<bool>,
    }

    impl FakeChild {
        fn dying_at(rung: Rung) -> Self {
            Self {
                received: Mutex::new(Vec::new()),
                dies_at: Some(rung),
                exited: Mutex::new(false),
            }
        }

        fn immortal() -> Self {
            Self {
                received: Mutex::new(Vec::new()),
                dies_at: None,
                exited: Mutex::new(false),
            }
        }

        fn already_gone() -> Self {
            Self {
                received: Mutex::new(Vec::new()),
                dies_at: None,
                exited: Mutex::new(true),
            }
        }

        fn rungs(&self) -> Vec<Rung> {
            self.received.lock().expect("fake child lock").clone()
        }
    }

    impl Stoppable for FakeChild {
        fn signal(&self, rung: Rung) {
            self.received.lock().expect("fake child lock").push(rung);
            if self.dies_at == Some(rung) {
                *self.exited.lock().expect("fake child lock") = true;
            }
        }

        fn has_exited(&self) -> bool {
            *self.exited.lock().expect("fake child lock")
        }
    }

    /// Graces short enough that a stubborn child costs milliseconds, not seconds.
    fn quick() -> Ladder {
        Ladder {
            hup_grace: Duration::from_millis(5),
            term_grace: Duration::from_millis(5),
            kill_grace: Duration::from_millis(5),
            poll: Duration::from_millis(1),
        }
    }

    #[test]
    fn a_child_that_goes_on_hup_is_never_sent_more() {
        let children = [FakeChild::dying_at(Rung::Hup)];
        stop_children(&children, quick());
        assert_eq!(children[0].rungs(), vec![Rung::Hup]);
    }

    #[test]
    fn the_ladder_escalates_in_order() {
        let children = [FakeChild::immortal()];
        stop_children(&children, quick());
        assert_eq!(children[0].rungs(), vec![Rung::Hup, Rung::Term, Rung::Kill]);
    }

    #[test]
    fn a_child_that_goes_on_term_is_never_killed() {
        let children = [FakeChild::dying_at(Rung::Term)];
        stop_children(&children, quick());
        assert_eq!(children[0].rungs(), vec![Rung::Hup, Rung::Term]);
    }

    #[test]
    fn an_already_exited_child_is_never_signalled() {
        let children = [FakeChild::already_gone()];
        stop_children(&children, quick());
        assert!(children[0].rungs().is_empty());
    }

    #[test]
    fn one_stubborn_child_does_not_re_signal_the_others() {
        // The ladder is per-rung, not per-child: a session that will not go must not drag
        // its well-behaved neighbours into a SIGTERM they never needed.
        let children = [FakeChild::dying_at(Rung::Hup), FakeChild::immortal()];
        stop_children(&children, quick());
        assert_eq!(children[0].rungs(), vec![Rung::Hup]);
        assert_eq!(children[1].rungs(), vec![Rung::Hup, Rung::Term, Rung::Kill]);
    }

    /// The rung the fakes cannot prove: that a real signal reaches a real child.
    ///
    /// `deliver` aims at the child's process *group*, which only works because portable-pty
    /// starts it in a session of its own — get that wrong and this hangs to the SIGKILL
    /// rung instead of finishing in a poll interval.
    #[cfg(unix)]
    #[test]
    fn a_real_child_stops_at_the_first_rung() {
        let spec = cide_pty::SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 30");
        let session = PtySession::spawn(spec).expect("spawn sh");

        stop_children(std::slice::from_ref(&session), Ladder::default());
        assert!(session.has_exited(), "SIGHUP did not reach the child");
    }

    /// The last rung, which the fakes cannot see at all: that it is really SIGKILL, and that
    /// it really reaches the group.
    ///
    /// A session stubborn enough to get here has descendants that were stubborn too — they
    /// ignored the same two signals it did. Anything less than a group-wide SIGKILL leaves
    /// them running with the pty closed under them and nothing left to reach them with.
    #[cfg(unix)]
    #[test]
    fn the_kill_rung_takes_the_whole_group() {
        let dir = temp_dir("kill-group");
        let leader_armed = dir.join("leader-armed");
        let descendant_armed = dir.join("descendant-armed");
        // The subshell is the descendant that matters: it refuses everything catchable, and
        // an ignored disposition survives the exec into `sleep`. The loop keeps the leader
        // alive past both gentle rungs, which is what gets the ladder to the last one.
        let script = format!(
            "trap '' HUP TERM; (trap '' HUP TERM; : > {descendant}; sleep 300) & \
             : > {leader}; while :; do sleep 0.05; done",
            descendant = descendant_armed.display(),
            leader = leader_armed.display(),
        );
        let spec = cide_pty::SpawnSpec::new("/bin/sh", dir.clone())
            .arg("-c")
            .arg(script);
        let session = PtySession::spawn(spec).expect("spawn sh");
        let pid =
            i32::try_from(session.child_pid().expect("the child has a pid")).expect("pid fits");

        // Both traps have to be in place before the first rung. Signalling a shell that is
        // still starting kills it under the default disposition, and the test then passes
        // without ever reaching the rung it exists to check.
        let armed = Instant::now() + Duration::from_secs(5);
        while !(leader_armed.is_file() && descendant_armed.is_file()) && Instant::now() < armed {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            leader_armed.is_file() && descendant_armed.is_file(),
            "the fixture never armed its traps"
        );

        let ladder = Ladder {
            hup_grace: Duration::from_millis(100),
            term_grace: Duration::from_millis(100),
            kill_grace: Duration::from_millis(500),
            poll: Duration::from_millis(10),
        };
        stop_children(std::slice::from_ref(&session), ladder);
        assert!(session.has_exited(), "the leader outlived the ladder");

        // Polled, not asserted once: a killed descendant stays a zombie in the same group
        // until whatever inherited it gets round to reaping, and that is not instant.
        let deadline = Instant::now() + Duration::from_secs(2);
        // SAFETY: `kill` with signal 0 takes two integers, sends nothing and touches no
        // memory owned by this process; it only reports whether the group still has members.
        while unsafe { libc::kill(-pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            unsafe { libc::kill(-pid, 0) } != 0,
            "a descendant outlived the ladder"
        );
    }

    /// The end of the chain this change exists to fix: a real child, a real signal, and the
    /// number that reaches `SessionState::Exited`.
    ///
    /// Driven through `watch_exit_with` and `exit_state` — this module's own two halves of
    /// the chain — rather than through `report_exit`, which needs an `AppHandle` no test in
    /// this build can make. Registering with `session.on_exit` directly and then comparing
    /// a hand-built `SessionState::Exited { code: exit.code }` against `137` is what the
    /// first version of this test did, and it is `assert_eq!(exit.code, 137)` written twice:
    /// green with every line of this module deleted.
    #[cfg(unix)]
    #[test]
    fn a_session_killed_outright_reports_the_signal_it_died_of() {
        let dir = temp_dir("exit-code");
        let armed = dir.join("armed");
        // The traps have to be in place before the first rung, exactly as in
        // `the_kill_rung_takes_the_whole_group`. Signalling a shell that is still starting
        // kills it under the default disposition, and the ladder then stops at SIGHUP — the
        // first draft of this test asserted 137 and got a perfectly correct 129.
        let script = format!(
            "trap '' HUP TERM; : > {armed}; while :; do sleep 0.05; done",
            armed = armed.display(),
        );
        let spec = cide_pty::SpawnSpec::new("/bin/sh", dir.clone())
            .arg("-c")
            .arg(script);
        let session = PtySession::spawn(spec).expect("spawn sh");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !armed.is_file() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(armed.is_file(), "the fixture never armed its traps");

        let (tx, rx) = std::sync::mpsc::channel::<Exit>();
        watch_exit_with(&session, move |exit| {
            let _ = tx.send(exit);
        });

        let ladder = Ladder {
            hup_grace: Duration::from_millis(100),
            term_grace: Duration::from_millis(100),
            kill_grace: Duration::from_millis(500),
            poll: Duration::from_millis(10),
        };
        stop_children(std::slice::from_ref(&session), ladder);

        let exit = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the exit watcher was never called");
        assert_eq!(
            exit.code,
            128 + libc::SIGKILL,
            "a child that ignored both gentle rungs must report the kill, not -1: {exit:?}"
        );
        assert!(is_abnormal(&exit));
        // The literal, not `exit.code` again: this is the value the emit puts on the wire,
        // and 137 is what a user reading the pane has to see for an OOM kill.
        assert_eq!(
            exit_state(&exit),
            SessionState::Exited { code: 137 },
            "the state the frontend receives does not carry the real code"
        );
    }

    #[test]
    fn a_session_that_finished_cleanly_reports_zero_and_says_nothing() {
        let spec = cide_pty::SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("exit 0");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let (tx, rx) = std::sync::mpsc::channel::<Exit>();
        watch_exit_with(&session, move |exit| {
            let _ = tx.send(exit);
        });

        let exit = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the exit watcher was never called");
        assert_eq!(exit.code, 0);
        assert_eq!(exit_state(&exit), SessionState::Exited { code: 0 });
        assert!(
            !is_abnormal(&exit),
            "an ordinary quit must not warn once per pane"
        );
    }

    #[test]
    fn a_status_nobody_could_read_is_published_as_unknown_rather_than_as_success() {
        // `SessionState::is_live` is false for every `Exited` whatever the code, so nothing
        // downstream would notice a 0 here — which is exactly why it has to be pinned. The
        // pane prints this number, and "it finished, and it worked" is the one thing the
        // one case with no answer must not claim.
        let unknown = Exit {
            code: UNKNOWN_EXIT_CODE,
            signal: None,
        };
        assert_eq!(
            exit_state(&unknown),
            SessionState::Exited { code: -1 },
            "an unreadable status was published as something a reader could believe"
        );
        assert_ne!(exit_state(&unknown), SessionState::Exited { code: 0 });
    }

    #[test]
    fn an_unreadable_status_is_still_worth_a_log_line() {
        // The one case with no answer. It has to warn, because "this build could not tell"
        // is exactly the thing a user chasing a vanished pane needs to know.
        assert!(is_abnormal(&Exit {
            code: UNKNOWN_EXIT_CODE,
            signal: None,
        }));
        assert!(is_abnormal(&Exit {
            code: 1,
            signal: None,
        }));
    }

    /// A workspace with one project whose console holds the primary Claude pane plus a
    /// second Claude pane and a shell.
    fn fixture(root: &Path) -> Workspace {
        let mut ws = Workspace::default();
        let project = workspace::open_project(&mut ws, vec![root.to_path_buf()], None)
            .expect("open the fixture project");
        let p = ws.projects.get_mut(&project).expect("fixture project");
        let tab = &mut p.tabs[0];
        let target = tab.tree.focused;

        layout::split(
            &mut tab.tree,
            target,
            Axis::Col,
            Side::After,
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Claude,
                role: PaneRole::Auxiliary,
                session: Some(SessionId::new()),
                title: "secondary : claude".into(),
            },
        )
        .expect("split in a second claude pane");
        layout::split(
            &mut tab.tree,
            target,
            Axis::Row,
            Side::After,
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Shell,
                role: PaneRole::Auxiliary,
                session: Some(SessionId::new()),
                title: "fixture : bash".into(),
            },
        )
        .expect("split in a shell pane");
        ws
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cide-lifecycle-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create the temp dir");
        dir
    }

    fn write_transcript(projects_dir: &Path, cwd: &Path, session: SessionId) {
        let encoded: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let dir = projects_dir.join(encoded);
        std::fs::create_dir_all(&dir).expect("create the transcript dir");
        std::fs::write(dir.join(format!("{session}.jsonl")), b"{}\n").expect("write a transcript");
    }

    #[test]
    fn every_process_bearing_pane_appears_once() {
        let root = temp_dir("panes");
        let ws = fixture(&root);
        let plan = plan_restore_in(&ws, None);

        assert_eq!(plan.len(), 3, "one entry per claude or shell pane");
        let mut ids: Vec<PaneId> = plan.iter().map(|e| e.pane).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3, "a pane must not be planned twice");
    }

    #[test]
    fn only_the_primary_session_is_eager() {
        let root = temp_dir("eager");
        let ws = fixture(&root);
        let plan = plan_restore_in(&ws, None);

        let eager: Vec<&PaneRestore> = plan.iter().filter(|e| e.eager).collect();
        let [entry] = eager.as_slice() else {
            panic!("reopening a project starts one agent, not {}", eager.len());
        };

        let project = ws.projects.values().next().expect("fixture project");
        let pane = project.tabs[0]
            .tree
            .panes
            .get(&entry.pane)
            .expect("the eager pane is in the console tab");
        assert_eq!(pane.session, Some(project.primary_session));
        // No transcript directory was given, so nothing can claim to be resumable.
        assert_eq!(entry.restore, SessionRestore::Fresh);
    }

    #[test]
    fn a_claude_pane_with_a_transcript_is_resumable() {
        let root = temp_dir("resumable");
        let projects_dir = temp_dir("projects");
        let ws = fixture(&root);
        let primary = ws
            .projects
            .values()
            .next()
            .expect("fixture project")
            .primary_session;
        write_transcript(&projects_dir, &root, primary);

        let plan = plan_restore_in(&ws, Some(&projects_dir));
        let primary_entry = plan
            .iter()
            .find(|e| e.eager)
            .expect("the primary pane is planned");
        assert_eq!(
            primary_entry.restore,
            SessionRestore::Resumable { session: primary }
        );

        // The secondary Claude pane has no transcript, so it must not claim one.
        let secondary = plan
            .iter()
            .find(|e| e.kind == PaneKind::Claude && !e.eager)
            .expect("the secondary claude pane is planned");
        assert_eq!(secondary.restore, SessionRestore::Fresh);
    }

    #[test]
    fn a_shell_is_never_resumable() {
        let root = temp_dir("shell");
        let projects_dir = temp_dir("projects");
        let ws = fixture(&root);

        // Give the shell pane a transcript under its own id. Even then it is Fresh: a
        // shell's screen died with its process, and `--resume` means nothing to bash.
        let project = ws.projects.values().next().expect("fixture project");
        let shell_session = project.tabs[0]
            .tree
            .panes
            .values()
            .find(|p| p.kind == PaneKind::Shell)
            .and_then(|p| p.session)
            .expect("the fixture has a shell pane");
        write_transcript(&projects_dir, &root, shell_session);

        let plan = plan_restore_in(&ws, Some(&projects_dir));
        let shell = plan
            .iter()
            .find(|e| e.kind == PaneKind::Shell)
            .expect("the shell pane is planned");
        assert_eq!(shell.restore, SessionRestore::Fresh);
    }

    #[test]
    fn a_detached_pane_keeps_its_place_in_the_plan() {
        let root = temp_dir("detached");
        let projects_dir = temp_dir("projects");
        let mut ws = fixture(&root);
        let (project, tab, pane, session) = {
            let p = ws.projects.values().next().expect("fixture project");
            let t = &p.tabs[0];
            let victim = t
                .tree
                .panes
                .values()
                .find(|pane| {
                    pane.kind == PaneKind::Claude && pane.session != Some(p.primary_session)
                })
                .expect("the secondary claude pane");
            (p.id, t.id, victim.id, victim.session)
        };
        let session = session.expect("the secondary pane has a session");
        write_transcript(&projects_dir, &root, session);

        let label = workspace::detach_pane(&mut ws, project, tab, pane).expect("detach the pane");
        let plan = plan_restore_in(&ws, Some(&projects_dir));

        let entry = plan
            .iter()
            .find(|e| e.pane == pane)
            .expect("a detached pane is still part of the workspace");
        assert_eq!(entry.window, label, "it restores into its own window");
        assert_eq!(entry.tab, tab, "and remembers where it re-docks");
        assert_eq!(entry.restore, SessionRestore::Resumable { session });
        assert!(
            !entry.eager,
            "a torn-out secondary pane still waits to be asked"
        );
    }

    #[test]
    fn a_missing_transcript_reads_as_fresh() {
        let projects_dir = temp_dir("missing");
        let cwd = temp_dir("cwd");
        assert!(!transcript_exists(&projects_dir, &cwd, SessionId::new()));
    }

    #[test]
    fn a_transcript_is_found_under_the_encoded_cwd() {
        let projects_dir = temp_dir("found");
        let cwd = temp_dir("cwd");
        let session = SessionId::new();
        write_transcript(&projects_dir, &cwd, session);
        assert!(transcript_exists(&projects_dir, &cwd, session));
        assert!(
            !transcript_exists(&projects_dir, &cwd, SessionId::new()),
            "another session's id must not match"
        );
    }
}
