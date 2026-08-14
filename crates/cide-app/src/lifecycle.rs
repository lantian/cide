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
use crate::windows;
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

    // Likewise unconditional, and it matters more here than for the workspace: the file the
    // user was looking at when they quit is the one they are most likely to come back to, and
    // its position is by construction the newest thing in the store — so a debounce waited out
    // on the way to exit loses exactly the entry that was worth keeping.
    if let Some(positions) =
        app.try_state::<std::sync::Arc<crate::positions_state::PositionsState>>()
    {
        positions.write_now();
    }

    // Before the children are signalled. A `claude` blocked on `openDiff` has to receive its
    // rejection over a socket that is still open; killing it first would end the turn with a
    // transport error where a plain "the user did not accept it" was available, and killing
    // it *after* would leave the rejection racing the SIGKILL.
    if let Some(servers) = app.try_state::<crate::ide::IdeServers>() {
        servers.stop_all();
    }

    // Then the language servers, and in this order: a blocked `openDiff` needs its socket first
    // (above), where a `gopls` needs only enough time to write its cache. Each handle's `Drop`
    // runs `shutdown` → `exit` → SIGTERM → SIGKILL, which is why this can be a single call and
    // still be a ladder.
    //
    // Before the PTY ladder below, because a rust-analyzer holding 1–4 GB is the largest thing
    // in the process and giving the kernel that memory back early makes the rest of the shutdown
    // cheaper on a machine that is already under pressure.
    if let Some(diagnostics) = app.try_state::<crate::lsp::DiagnosticsRegistry>() {
        diagnostics.close_all();
    }

    // A store per project, and the only thing on this path that reaches a content search: the
    // ladder below blocks the main thread for as long as the slowest child takes, and a walker
    // thread per core reading a repository nobody will see the results of is disk the dying
    // children are competing for. `cmd::fs::close_all` also cancels, but nothing calls it —
    // this is the quit path.
    //
    // Deliberately not `cmd::fs::close_all`, which would drop every project's index here:
    // that is the teardown `fs_close` hands to a blocking worker precisely because it is too
    // slow for a shared thread, and the memory is about to go back to the kernel anyway.
    if let Some(searches) = app.try_state::<crate::cmd::search::SearchRegistry>() {
        searches.cancel_all();
    }

    let Some(registry) = app.try_state::<SessionRegistry>() else {
        tracing::error!("no session registry during shutdown; children may outlive the app");
        return;
    };

    // Before the ladder, and it has to be: `screen_state()` reads the mirror of a *live*
    // session, and a child that has been through SIGKILL has usually cleared the screen or
    // dumped a shell's exit message over it on the way out. This is the last moment the
    // screen is the one the user was looking at.
    //
    // The pane list is copied out from under the workspace lock before any of the reading or
    // writing happens. `WorkspaceState::with` is not reentrant and this path serialises a
    // screen per pane and writes a file; holding the lock across that would stall every other
    // thread that wants the workspace, on the way out, for no reason.
    if let Some(state) = app.try_state::<WorkspaceState>() {
        let shells = state.with(shell_sessions);
        save_screens(&shells, &registry);
    }

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
    // Captured here rather than looked up in the callback, because the callback runs on the
    // reaper thread *after* the child is gone and the registry entry deliberately survives
    // that (see the note at the end of `report_exit`). `child_pid` is a plain field taken at
    // spawn, so this is the same value either way — reading it now just means the unbind
    // below cannot depend on the order two teardown steps happen in.
    let pid = session.child_pid();
    watch_exit_with(session, move |exit| report_exit(&app, id, pid, &exit));
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
fn report_exit(app: &AppHandle, id: SessionId, pid: Option<u32>, exit: &Exit) {
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

    // And the IDE server's pid → pane table, for the same "do not answer for a corpse"
    // reason one rung sideways.
    //
    // `IdeServer::unbind_pane` had **no caller anywhere in the workspace** until this line:
    // it was written, documented with the hazard it prevents, and wired to nothing — so
    // `pane_of_pid` only ever grew, and every binding the process ever made outlived the
    // child it described. That is this project's recurring defect, and it is worth naming
    // where it was found rather than only where it is fixed.
    //
    // It is *here* and not in `pane_close`, which is where the doc comment guessed it would
    // go, and the difference is a real case: closing a pane leaves its child running (the
    // session is owned by the registry, not by the pane), and `claude.mirror` puts two panes
    // on one child and therefore one pid. Unbinding when a pane closes would make the
    // surviving mirror unaddressable and take a working `@`-mention away. A reaped child, by
    // contrast, can never speak again under that pid — and the instant it is reaped is the
    // earliest the OS may hand the number to somebody else.
    if let Some(pid) = pid
        && let Some(servers) = app.try_state::<crate::ide::IdeServers>()
    {
        servers.unbind_pid(pid);
    }

    // And the waiting set, for the same reason one rung up: a `claude` killed while it was
    // waiting for the user would otherwise keep an `Awaiting: 1` in a task-bar entry for the
    // life of the process, with the pane gone and therefore no gesture left that could
    // acknowledge it. The frontend cannot clear this one either — its own rule drops the
    // session on `Exited`, but only in a window that was listening at the time.
    //
    // Retitling needs the workspace, so it is done here rather than in `windows.rs`: this
    // runs on `cide-pty`'s reaper thread, and `WorkspaceState` is behind the same lock every
    // command takes, which is why the snapshot is read and released before the titles are set.
    if windows::forget_awaiting(id)
        && let Some(state) = app.try_state::<WorkspaceState>()
    {
        let ws = state.snapshot();
        crate::cmd::window::retitle(app, &ws);
        crate::emit::session_awaiting(app, windows::awaiting_sessions());
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
    // Read once for the whole plan: it is one bool for the launch, not a per-pane decision.
    let resume_all = ws.settings.claude.resume_all_on_launch;

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
                    resume_all,
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
            if let Some(entry) = entry_for(
                pane,
                window,
                project,
                tab,
                &root.path,
                projects_dir,
                resume_all,
            ) {
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
    resume_all: bool,
) -> Option<PaneRestore> {
    // A diff or an editor pane has nothing to spawn, and listing it would leave the
    // frontend to filter out entries it can only ignore.
    if !matches!(pane.kind, PaneKind::Claude | PaneKind::Shell) {
        return None;
    }
    let restore = restore_for(pane, cwd, projects_dir);
    Some(PaneRestore {
        window,
        project: project.id,
        tab,
        pane: pane.id,
        kind: pane.kind,
        cwd: cwd.to_path_buf(),
        restore,
        // Eager when it is the project's console, and — with `resume_all_on_launch` — when
        // there is a conversation to pick up.
        //
        // Gated on `Resumable` rather than on the setting alone, and that is the distinction
        // the splash was really protecting. Resuming a pane whose transcript still exists
        // returns the user to what they left; spawning one whose transcript is gone starts a
        // *new* conversation they did not ask for, in a pane that looks identical. So an
        // unresumable pane keeps its splash however this is set.
        eager: pane.session == Some(project.primary_session)
            || (resume_all && matches!(restore, SessionRestore::Resumable { .. })),
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

/// Whether a pane could resume `session` right now, asked one pane at a time.
///
/// [`plan_restore`] answers the same question for the whole workspace, once, at launch — which
/// is the only moment it used to be asked. A pane whose child dies *during* the run needs it
/// again and cannot wait for the next launch: it is what decides whether the bar over a dead
/// terminal offers **Resume this conversation** beside **Start a new session**, and an offer
/// made without asking would be a button that fails after it is pressed.
///
/// `cwd` is the directory the session was spawned in — the project's primary root, which is
/// what the pane passes to `session_spawn`. Nothing is read or written under the transcript
/// directory; see [`transcript_exists`], which is the whole of the filesystem contact.
pub fn resumable(cwd: &Path, session: SessionId) -> bool {
    claude_projects_dir().is_some_and(|dir| transcript_exists(&dir, cwd, session))
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

// --- the previous run's shell screens --------------------------------------------------

/// Where the last run's shell screens are kept.
///
/// **A sidecar, not `workspace.json`, and the size argument decides it.** `workspace.json` is
/// the layout: it is rewritten on a 500 ms debounce every time the user drags a splitter or
/// focuses a pane, it is loaded before any window exists, and a corrupt one costs the user
/// their whole arrangement (`persist::load` moves it aside and starts over). A screenful of
/// escape codes per pane is 10–40 KiB of churn on every one of those writes, for bytes
/// nothing but a respawning shell will ever read. Keeping it beside `workspace.json` means it
/// is written exactly once, at quit, and a corrupt or missing one costs a banner line.
///
/// In the state directory rather than the config one, on the argument [`cide_core::persist`]
/// already makes: the app writes it and nobody would want it in a dotfiles repository.
fn screens_path() -> PathBuf {
    cide_core::persist::state_dir().join("screens.json")
}

/// The visible screen, and only the visible screen.
///
/// `cide_pty::SCROLLBACK` is 5,000 lines, and a 5,000-line scrollback per pane is not
/// something to write to disk at quit — it is megabytes per pane, it takes a visible moment
/// to serialise on the way out (on the main thread, with the shutdown ladder still to run),
/// and it is not what the complaint was about. What the user missed was the screen they were
/// looking at. [`cide_pty::PtySession::screen_state`] is exactly that: contents, cursor,
/// modes, for the visible rows only.
///
/// Anything bigger than this is dropped rather than truncated. A screen is a stream of escape
/// sequences, and cutting one in half leaves a dangling `CSI` that swallows whatever follows
/// it — which is the very class of defect the banner bug was. 128 KiB is far above a
/// full-colour 400×100 screen and far below anything worth worrying about on disk.
const MAX_SCREEN_BYTES: usize = 128 * 1024;

/// How many screens are kept at all.
///
/// The pane host registry evicts at twelve; a workspace with more shells than this has more
/// than the user is looking at. Bounded so the file cannot grow without limit across a long
/// life of the app — it is rewritten wholesale at each quit, so a session that is gone is
/// simply not written again.
const MAX_SCREENS: usize = 24;

/// One shell's parting screen.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SavedScreen {
    /// The bytes `screen_state()` produced, as text.
    ///
    /// A `String` rather than base64: the payload is escape sequences plus whatever the user
    /// had on screen, which is UTF-8 by the time it has been through `vt100`. JSON escaping
    /// turns each escape byte into a six-character `\u001b`, which is larger than base64
    /// would be — and a file a human can open and read is worth more here than the bytes.
    screen: String,
    /// Epoch milliseconds, so a future version can expire these. Nothing reads it yet, and
    /// it is written now because adding it later means every existing file lacks it.
    at: u64,
}

/// Save the visible screen of every shell in the workspace, for the next launch to replay.
///
/// Claude panes are deliberately **not** saved. The CLI redraws its whole UI from its own
/// transcript on `--resume`, so a replayed screen there is a stale frame that the real one
/// paints over a moment later — cost with no benefit, and by far the largest screens in the
/// app.
///
/// Failure is logged and swallowed. This runs on the quit path, between flushing the
/// workspace and signalling the children; a disk error here must not stop either.
///
/// A torn-out pane counts: `project.detached` is where a pane waits while its own window
/// holds it, and a shell the user tore into a window of its own is the one they are most
/// likely to want back.
fn shell_sessions(ws: &Workspace) -> Vec<SessionId> {
    let mut wanted: Vec<SessionId> = Vec::new();
    for project in ws.projects.values() {
        let panes = project
            .tabs
            .iter()
            .flat_map(|t| t.tree.panes.values())
            .chain(project.detached.values());
        for pane in panes {
            if pane.kind == PaneKind::Shell
                && let Some(session) = pane.session
                && !wanted.contains(&session)
            {
                wanted.push(session);
            }
        }
    }
    wanted
}

/// Serialise those sessions' screens and write the sidecar. See [`shell_sessions`].
fn save_screens(wanted: &[SessionId], registry: &SessionRegistry) {
    let at = cide_core::persist::now_ms();
    let mut saved: std::collections::BTreeMap<String, SavedScreen> = Default::default();
    for &session in wanted.iter().take(MAX_SCREENS) {
        let Some(pty) = registry.get(session) else {
            continue;
        };
        let bytes = pty.screen_state();
        if bytes.is_empty() || bytes.len() > MAX_SCREEN_BYTES {
            continue;
        }
        let screen = String::from_utf8_lossy(&bytes).into_owned();
        saved.insert(session.to_string(), SavedScreen { screen, at });
    }
    write_screens(&screens_path(), &saved);
}

/// Publish the sidecar, or clear it when there is nothing to keep.
///
/// Split from [`save_screens`] so a test can name its own path: the real one is the
/// developer's own `~/.local/state/cide/screens.json`, and a test that wrote there would
/// destroy the screens of their last real session.
fn write_screens(path: &Path, saved: &std::collections::BTreeMap<String, SavedScreen>) {
    if saved.is_empty() {
        // Removed rather than left behind. A stale file would replay the screens of a run
        // two launches ago into panes that had nothing on them last time.
        if path.exists()
            && let Err(error) = std::fs::remove_file(path)
        {
            tracing::warn!(%error, "could not clear the saved shell screens");
        }
        return;
    }
    match serde_json::to_vec(saved) {
        // Through `persist::write_atomic`, and the reason is the file mode rather than the
        // atomicity. This was a plain `fs::write`, argued for on the grounds that a torn
        // sidecar costs only a banner line — which is true, and is not the whole trade.
        // `fs::write` creates at 0666 & umask, so on an ordinary machine this file was 0644:
        // world-readable, holding a screenful of the user's shell for every open pane. That
        // is `export AWS_SECRET_ACCESS_KEY=…`, a `.env` someone `cat`ted, a psql URL with a
        // password in it. `persist` already owns the one routine in this app that creates a
        // state file 0600 — the same routine, and the same argument, as the `workspace.json`
        // that holds a proxy password — so this uses it rather than re-deciding the mode.
        // The atomicity and the directory fsync come along for free.
        Ok(bytes) => {
            if let Err(error) = cide_core::persist::write_atomic(path, &bytes) {
                tracing::warn!(%error, "could not save the shell screens");
            }
        }
        Err(error) => tracing::warn!(%error, "could not serialise the shell screens"),
    }
}

/// The screens the previous run left behind, read once.
///
/// Latched for the life of the process. The file is written only at quit, so re-reading it
/// would return the same bytes; and a shell respawned twice in one run (killed, then the pane
/// re-mounted) should see the same previous screen both times rather than the first read
/// consuming it.
fn saved_screens() -> &'static std::collections::BTreeMap<String, SavedScreen> {
    use std::sync::OnceLock;
    static LOADED: OnceLock<std::collections::BTreeMap<String, SavedScreen>> = OnceLock::new();
    LOADED.get_or_init(|| {
        let path = screens_path();
        let Ok(bytes) = std::fs::read(&path) else {
            return Default::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(%error, "the saved shell screens are unreadable; panes come back empty");
            Default::default()
        })
    })
}

/// The bytes to seed a respawned shell's mirror with, or `None` if nothing was saved.
///
/// **Blocking**: reads a file on the first call. Callers are Tauri commands and must be on
/// the blocking pool — `session_spawn` asks for it inside the same closure that forks.
pub fn prior_screen(session: SessionId) -> Option<Vec<u8>> {
    saved_screens()
        .get(&session.to_string())
        .map(|s| s.screen.as_bytes().to_vec())
}

/// The line a respawned shell prints to say what the user is looking at.
///
/// # Why the wording changed
///
/// The old line was `— session restored (previous output not retained) —`, and it was honest:
/// nothing was retained. Now the visible screen is, so the line has to change *meaning*
/// rather than disappear. Replayed output is **dead text** — the cwd may have moved, the
/// prompt above the line is a picture of a prompt, and scrolling up finds nothing because
/// only the visible screen was kept. A user who believes otherwise types into a shell they
/// think has state it does not have. So the banner stays, and says which of the two happened.
///
/// # Why it is written into the terminal
///
/// It used to be a React `<p>` rendered above the pane's terminal, which is what broke the
/// pane: `.body` gives the terminal `height: 100%`, so a sibling with height pushed the
/// terminal past the bottom of the frame and its background painted over the next row's title
/// bar. As a line in the transcript it cannot do that, it scrolls away with the text it is
/// about, and it survives a re-dock the way every other byte in the pane does — the same
/// argument `ui/src/panes/exitMarker.ts` already makes for `— exited —`.
///
/// # The escape sequences
///
/// **`\x1b[0m` comes first, and that is the whole bug fix.** `state_formatted()` ends by
/// emitting the screen's *live* attributes, faithfully — a shell that was sitting inside a
/// coloured prompt leaves a background colour set. Opening with `\x1b[2m` on top of that
/// paints this line, and every line after it, on the previous run's background. Reset, then
/// dim, then reset again.
pub fn restore_notice(replayed: bool) -> Vec<u8> {
    let text = if replayed {
        "— session restored · the output above is from the previous run and is not live —"
    } else {
        "— session restored (previous output not retained) —"
    };
    // The leading newline only when there is something above to separate from; on an empty
    // screen it would be a blank first line for no reason.
    let lead = if replayed { "\r\n" } else { "" };
    format!("\x1b[0m{lead}\x1b[2m{text}\x1b[0m\r\n").into_bytes()
}

/// Everything a respawned shell's mirror should start with: the old screen, then the notice.
///
/// **Blocking on the first call**, through [`prior_screen`]. `session_spawn` asks for it inside
/// the closure that forks, which is where this app puts every filesystem read a command needs.
pub fn shell_preload(session: SessionId) -> Vec<u8> {
    preload_from(prior_screen(session))
}

/// Hand the terminal back to a child that set none of the previous one's modes.
///
/// # The same bug as the SGR, one layer up
///
/// `restore_notice` opens with `\x1b[0m` because `state_formatted()` ends by restoring the
/// screen's live *attributes*. It also ends by restoring the screen's live **input modes** —
/// `write_input_mode_formatted` emits application keypad, application cursor keys, bracketed
/// paste and the mouse protocol — and those are not attributes, so no SGR closes them.
///
/// A shell is not what sets them. A TUI is: quit cide with `htop`, `vim`, `lazygit` or `btop`
/// on screen in a shell pane and the saved screen carries `\x1b[?1000h`/`\x1b[?1006h` and
/// `\x1b[?25l` in it. Replay that into a fresh `bash`, which never asked for mouse reporting
/// and therefore never turns it off, and the pane is stuck for its whole life: the cursor is
/// invisible, a drag selects nothing because the terminal is forwarding the drag to the child,
/// and every click types `\x1b[<0;40;12M` at the prompt. Nothing the user can do clears it —
/// only closing the pane does — and it happens on the launch *after* the one that caused it,
/// which is the hardest possible thing to connect to a cause.
///
/// So the replayed screen is followed by the modes turned back off, before the notice. Each
/// one is the DECRST for a mode `write_input_mode_formatted` can emit, and nothing else: a
/// blanket DECSTR (`\x1b[!p`) was the alternative and it loses twice — it does not clear mouse
/// tracking in xterm.js, and `vt100` does not implement it at all, so the Rust mirror and the
/// user's terminal would disagree about the state of the pane from its first byte.
const NEUTRAL_MODES: &str = concat!(
    "\x1b[?25h",   // DECTCEM — the previous child may have hidden the cursor and died there.
    "\x1b>",       // DECKPNM — normal keypad.
    "\x1b[?1l",    // DECCKM — cursor keys send CSI, not SS3.
    "\x1b[?1000l", // and the four mouse-tracking modes vt100 tracks, plus its encoding.
    "\x1b[?1002l",
    "\x1b[?1003l",
    "\x1b[?1006l",
    "\x1b[?1005l",
    "\x1b[?9l",
    "\x1b[?2004l", // bracketed paste: the new shell's readline turns it back on if it wants it.
);

/// The composition, split out from the lookup so a test can drive both answers.
///
/// One function so the screen and the notice can never disagree about whether a replay
/// happened — a "previous output not retained" line printed under a screenful of retained
/// output is worse than either alone.
///
/// The order is load-bearing: the screen, then [`NEUTRAL_MODES`] to undo what the screen just
/// set, then the notice. Neutralising first would be undone by the replay itself.
fn preload_from(prior: Option<Vec<u8>>) -> Vec<u8> {
    let replayed = prior.is_some();
    let mut bytes = strip_old_notices(prior.unwrap_or_default());
    if replayed {
        bytes.extend_from_slice(NEUTRAL_MODES.as_bytes());
    }
    bytes.extend_from_slice(&restore_notice(replayed));
    bytes
}

/// The notice texts, so [`strip_old_notices`] recognises what [`restore_notice`] wrote.
///
/// Both variants, because a pane can have been restored with and without a replay across
/// different runs and the screen keeps whichever it got each time.
const NOTICE_TEXTS: [&str; 2] = [
    "— session restored · the output above is from the previous run and is not live —",
    "— session restored (previous output not retained) —",
];

/// Drop the notices a previous restore left on the screen.
///
/// **Without this the banner multiplies, once per launch.** The replay is the *screen*, and
/// the screen contains whatever was on it — including the notice the last restore printed. So
/// the composition was `[screen containing yesterday's notice] + [today's notice]`, and after
/// six restarts a shell pane opened with six identical lines claiming its output was not live.
/// Observed at six; it grows without bound.
///
/// Line-oriented rather than a byte replace: the notice is written as its own line, and taking
/// the line with it is what stops a bare `\r\n` accumulating in its place — which would push the
/// real output down the pane one row per launch and be the same bug wearing a blank.
///
/// Matched on the *text*, not on the full escape framing. The framing is this module's and can
/// change; the sentence is what identifies the line, and a screen that legitimately contains
/// that sentence is a shell echoing our own notice back at us, which is not a case worth
/// protecting.
fn strip_old_notices(screen: Vec<u8>) -> Vec<u8> {
    let Ok(text) = String::from_utf8(screen) else {
        // Not UTF-8, so not something to reason about line by line. A screen holding a binary
        // splat is replayed as-is rather than mangled — it was already unreadable.
        return Vec::new();
    };
    if !NOTICE_TEXTS.iter().any(|n| text.contains(n)) {
        return text.into_bytes();
    }
    let kept: Vec<&str> = text
        .split_inclusive("\r\n")
        .filter(|line| {
            // The notice's own line, and the `NEUTRAL_MODES` line that precedes it. Both are
            // written by `preload_from` and both were accumulating — the first test of this
            // caught only the sentence, and the mode run kept multiplying on the line above
            // it, which is the identical unbounded growth with nothing legible to show for it.
            //
            // `NEUTRAL_MODES` is a ten-escape run this module concatenates; a shell emitting
            // that exact sequence is emitting our preamble back at us.
            !NOTICE_TEXTS.iter().any(|n| line.contains(n)) && !line.contains(NEUTRAL_MODES)
        })
        .collect();
    kept.concat().into_bytes()
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
    fn only_the_primary_is_eager_when_nothing_can_be_resumed() {
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
        // No transcript directory was given, so nothing can claim to be resumable — which is
        // also why `resume_all_on_launch` cannot widen this, and why the name of this test had
        // to narrow when that setting arrived. It passed unchanged under the new default, for
        // the wrong reason: the clause it should have been exercising was unreachable here.
        assert_eq!(entry.restore, SessionRestore::Fresh);
    }

    /// With `resume_all_on_launch`, every pane that *has* a conversation comes back live.
    ///
    /// The user's report: four Claude panes all showing `Resume "cide : claude"` after a
    /// restart, a screen asking a question whose answer is always yes.
    #[test]
    fn resume_all_makes_every_resumable_claude_pane_eager() {
        let root = temp_dir("resume-all");
        let projects_dir = temp_dir("resume-all-projects");
        let mut ws = fixture(&root);
        assert!(
            ws.settings.claude.resume_all_on_launch,
            "the default is on; this test would otherwise pass by describing the old behaviour"
        );

        // Both Claude panes get a transcript, so both are genuinely resumable.
        let project = ws
            .projects
            .values()
            .next()
            .expect("fixture project")
            .clone();
        for pane in project.tabs[0].tree.panes.values() {
            if pane.kind == PaneKind::Claude
                && let Some(session) = pane.session
            {
                write_transcript(&projects_dir, &root, session);
            }
        }

        let plan = plan_restore_in(&ws, Some(&projects_dir));
        let claude: Vec<&PaneRestore> =
            plan.iter().filter(|e| e.kind == PaneKind::Claude).collect();
        assert!(claude.len() >= 2, "fixture has two Claude panes");
        assert!(
            claude.iter().all(|e| e.eager),
            "every resumable Claude pane should come back live: {claude:?}"
        );

        // And turning it off restores the cautious behaviour rather than merely renaming it.
        ws.settings.claude.resume_all_on_launch = false;
        let cautious = plan_restore_in(&ws, Some(&projects_dir));
        let eager: Vec<&PaneRestore> = cautious.iter().filter(|e| e.eager).collect();
        assert_eq!(eager.len(), 1, "only the console: {eager:?}");
    }

    /// A pane whose transcript is gone keeps its splash, whatever the setting says.
    ///
    /// This is the distinction the splash was really protecting. Resuming returns the user to
    /// what they left; spawning a pane with no transcript starts a *new* conversation they did
    /// not ask for, in a pane that looks identical to one that was restored.
    #[test]
    fn resume_all_does_not_start_conversations_that_do_not_exist() {
        let root = temp_dir("resume-all-fresh");
        let projects_dir = temp_dir("resume-all-fresh-projects");
        let ws = fixture(&root);
        // No transcripts written at all.
        let plan = plan_restore_in(&ws, Some(&projects_dir));
        let unresumable: Vec<&PaneRestore> = plan
            .iter()
            .filter(|e| e.restore == SessionRestore::Fresh && e.kind == PaneKind::Claude)
            .collect();
        assert!(!unresumable.is_empty(), "fixture has an unresumable pane");
        assert!(
            unresumable.iter().all(|e| !e.eager
                || ws
                    .projects
                    .values()
                    .any(|p| Some(p.primary_session) == plan_session(e, &ws))),
            "a Fresh pane must not spawn on its own: {unresumable:?}"
        );
    }

    /// The pane's session id, for a plan entry. Test-only.
    fn plan_session(entry: &PaneRestore, ws: &Workspace) -> Option<SessionId> {
        ws.projects.values().find_map(|p| {
            p.tabs
                .iter()
                .find_map(|t| t.tree.panes.get(&entry.pane))
                .and_then(|pane| pane.session)
        })
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
        // It resumes with everything else, because it has a transcript and the default is to
        // pick conversations up. This assertion used to read `!entry.eager` — "a torn-out
        // secondary pane still waits to be asked" — which was true when exactly one pane was
        // eager and is now false by intent rather than by accident. Both directions are pinned
        // so the setting cannot be quietly dropped, and so this does not have to be revisited
        // as a mystery the next time the default moves.
        assert!(
            entry.eager,
            "a detached pane with a transcript comes back live like any other"
        );
        let mut cautious_ws = ws.clone();
        cautious_ws.settings.claude.resume_all_on_launch = false;
        let cautious = plan_restore_in(&cautious_ws, Some(&projects_dir));
        let entry = cautious
            .iter()
            .find(|e| e.pane == pane)
            .expect("still planned with the setting off");
        assert!(
            !entry.eager,
            "with the setting off, a torn-out secondary pane waits to be asked"
        );
    }

    // --- the restore notice, and the screens it is about ---------------------------------

    /// Every byte of the notice, so the SGR framing is asserted rather than eyeballed.
    ///
    /// This is the pane-breaking bug in test form. The line used to be a React `<p>` above the
    /// terminal, and `PaneTitleBar.module.css`'s `.body` gives the terminal `height: 100%` —
    /// so a sibling with a height of its own pushed the terminal past the bottom of the frame
    /// and its background painted over the next row's title bar. As a line in the transcript
    /// the only way it can bleed is an SGR it opens and never closes, which is what this pins.
    #[test]
    fn the_notice_opens_with_a_reset_and_closes_every_attribute_it_sets() {
        for replayed in [true, false] {
            let bytes = restore_notice(replayed);
            let text = String::from_utf8(bytes).expect("the notice is text");

            assert!(
                text.starts_with("\u{1b}[0m"),
                "the notice must reset first: `state_formatted()` ends by restoring the \
                 screen's live attributes, so a shell that quit inside a coloured prompt \
                 leaves a background set and this line would inherit it — {text:?}"
            );
            assert!(
                text.ends_with("\u{1b}[0m\r\n"),
                "the notice must close its own dim attribute, or the shell's first prompt is \
                 drawn dim — {text:?}"
            );
            assert_eq!(
                text.matches("\u{1b}[").count(),
                3,
                "reset, dim, reset — nothing else, and nothing left open: {text:?}"
            );
        }
    }

    /// The banner changed meaning rather than disappearing, which is the point of keeping it:
    /// replayed output is a picture of a shell, not a shell.
    #[test]
    fn the_notice_says_which_of_the_two_things_happened() {
        let replayed = String::from_utf8(restore_notice(true)).expect("text");
        let empty = String::from_utf8(restore_notice(false)).expect("text");

        assert!(replayed.contains("is not live"));
        assert!(empty.contains("previous output not retained"));
        assert_ne!(
            replayed, empty,
            "a user who cannot tell the two apart will type into a dead prompt"
        );
    }

    /// Nothing above to separate from means no leading blank line.
    #[test]
    fn the_notice_only_spaces_itself_off_when_there_is_output_above_it() {
        let replayed = String::from_utf8(restore_notice(true)).expect("text");
        let empty = String::from_utf8(restore_notice(false)).expect("text");
        assert!(replayed.starts_with("\u{1b}[0m\r\n"));
        assert!(!empty.starts_with("\u{1b}[0m\r\n"));
    }

    /// The two preloads, and the rule that the notice must describe the bytes above it.
    ///
    /// Driven through [`preload_from`] rather than `shell_preload` on purpose: the latter
    /// reads the real `~/.local/state/cide/screens.json`, so a developer who has run the app
    /// would be testing against their own last session.
    #[test]
    fn the_notice_always_describes_the_bytes_it_was_put_under() {
        let empty = String::from_utf8(preload_from(None)).expect("text");
        assert!(empty.contains("previous output not retained"), "{empty:?}");
        assert!(
            !empty.contains("is not live"),
            "an empty pane must not claim to be showing anything: {empty:?}"
        );

        let replayed = String::from_utf8(preload_from(Some(b"$ ls\r\nCargo.toml\r\n".to_vec())))
            .expect("text");
        assert!(replayed.starts_with("$ ls"), "the screen comes first");
        assert!(replayed.contains("is not live"), "{replayed:?}");
        assert!(
            !replayed.contains("not retained"),
            "a notice that says nothing was kept, under output that was kept, is the worst of \
             the three: {replayed:?}"
        );
    }

    /// A replay hands the new child a terminal, not the old child's terminal *modes*.
    ///
    /// The SGR half of this was already pinned above. This is the other half, and it is the
    /// one with no attribute reset behind it: `state_formatted()` ends with
    /// `write_input_mode_formatted`, so a screen saved while `htop` was running carries mouse
    /// tracking and a hidden cursor. `bash` never set those and never clears them, so without
    /// [`NEUTRAL_MODES`] the restored pane spends its whole life unable to select text, typing
    /// `\x1b[<0;40;12M` on every click, with no cursor. See `cide-pty`'s
    /// `a_preload_carries_the_old_childs_input_modes_into_the_mirror`, which is the proof that
    /// those bytes really do survive into the new session.
    #[test]
    fn a_replay_turns_off_the_modes_the_old_screen_turned_on() {
        // Exactly what `vt100`'s `write_input_mode_formatted` can emit, in front of it.
        let hostile = b"\x1b[?25l\x1b=\x1b[?1h\x1b[?1002h\x1b[?1006h\x1b[?2004h".to_vec();
        let out = String::from_utf8(preload_from(Some(hostile.clone()))).expect("text");

        for (set, cleared) in [
            ("\u{1b}[?25l", "\u{1b}[?25h"),
            ("\u{1b}=", "\u{1b}>"),
            ("\u{1b}[?1h", "\u{1b}[?1l"),
            ("\u{1b}[?1002h", "\u{1b}[?1002l"),
            ("\u{1b}[?1006h", "\u{1b}[?1006l"),
            ("\u{1b}[?2004h", "\u{1b}[?2004l"),
        ] {
            let at_set = out
                .find(set)
                .unwrap_or_else(|| panic!("{set:?} is the input"));
            let at_clear = out.rfind(cleared).unwrap_or_else(|| {
                panic!("nothing turns {set:?} back off; the pane is stuck in it: {out:?}")
            });
            assert!(
                at_clear > at_set,
                "{cleared:?} has to come *after* the screen that sets {set:?}, or the replay \
                 undoes it: {out:?}"
            );
        }

        // And the notice is last, so the user reads it under a terminal already handed back.
        assert!(
            out.ends_with("is not live —\u{1b}[0m\r\n"),
            "the notice is the last thing in the preload: {out:?}"
        );

        // Nothing is neutralised when nothing was replayed: the modes are only ever set by
        // the bytes above them, and a fresh pane emitting nine DECRSTs before its first
        // prompt is noise in every transcript for a case that cannot happen.
        let empty = String::from_utf8(preload_from(None)).expect("text");
        assert!(!empty.contains("\u{1b}[?1002l"), "{empty:?}");
    }

    /// The sidecar is the user's shell output. It is not world-readable.
    ///
    /// `fs::write` creates at 0666 & umask — 0644 on an ordinary machine — and this file holds
    /// a screenful per open pane: an `export …_TOKEN=`, a `.env` someone `cat`ted, a psql URL
    /// with a password in it. `persist` already owns the one routine that writes a private
    /// state file, and `workspace.json` has a test exactly like this one for exactly the same
    /// reason.
    #[test]
    #[cfg(unix)]
    fn the_saved_screens_are_not_readable_by_every_user_on_the_machine() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("screens-mode");
        let path = dir.join("screens.json");
        // Stand in for a file an older build left behind at 0644.
        std::fs::write(&path, b"{}").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("loosen");

        let mut saved = std::collections::BTreeMap::new();
        saved.insert(
            SessionId::new().to_string(),
            SavedScreen {
                screen: "$ echo $AWS_SECRET_ACCESS_KEY\r\n".into(),
                at: 1,
            },
        );
        write_screens(&path, &saved);

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "screens.json holds the user's shell output");
    }

    /// Nothing to keep clears the file rather than leaving the run-before-last's screens.
    #[test]
    fn an_empty_save_removes_a_sidecar_from_a_previous_run() {
        let dir = temp_dir("screens-clear");
        let path = dir.join("screens.json");
        std::fs::write(&path, b"{\"x\":1}").expect("seed");

        write_screens(&path, &Default::default());

        assert!(
            !path.exists(),
            "a stale sidecar replays the screens of a run two launches ago"
        );
    }

    /// Only shells, and each one once.
    ///
    /// Claude panes are deliberately absent: the CLI repaints its whole UI from the transcript
    /// on `--resume`, so a saved screen there is a stale frame something paints over a moment
    /// later — and they are by far the largest screens in the app.
    #[test]
    fn only_shell_panes_have_a_screen_worth_saving() {
        let root = temp_dir("screens");
        let ws = fixture(&root);

        let found = shell_sessions(&ws);
        let project = ws.projects.values().next().expect("fixture project");
        let shell = project.tabs[0]
            .tree
            .panes
            .values()
            .find(|p| p.kind == PaneKind::Shell)
            .and_then(|p| p.session)
            .expect("the fixture has a shell");

        assert_eq!(found, vec![shell], "only the shell, and only once");
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

#[cfg(test)]
mod notice_accumulation {
    use super::*;

    /// The banner does not multiply across restarts.
    ///
    /// Reported from a screenshot after six launches: a shell pane opened with six identical
    /// `— session restored —` lines. The replay is the *screen*, and the screen holds whatever
    /// the last restore printed on it, so each launch appended one more.
    #[test]
    fn a_restored_screen_carries_one_notice_however_often_it_is_restored() {
        let mut screen =
            b"lantian@powerhall:~/work/cide> pwd\r\n/home/lantian/work/cide\r\n".to_vec();
        for _ in 0..6 {
            // Each round is the next launch: yesterday's screen in, today's screen out.
            screen = preload_from(Some(screen));
        }
        let text = String::from_utf8(screen).expect("utf8");
        let notices = NOTICE_TEXTS
            .iter()
            .map(|n| text.matches(n).count())
            .sum::<usize>();
        assert_eq!(notices, 1, "six launches left {notices} notices:\n{text}");
        // And the shell's own output is still there — stripping must not take the transcript.
        assert!(text.contains("/home/lantian/work/cide"), "{text}");
    }

    /// A screen with no notice is passed through untouched.
    ///
    /// The strip runs on every restored shell, so the ordinary case has to be a no-op rather
    /// than a re-encode: `split_inclusive`/`concat` on a screen full of escape sequences must
    /// return exactly what it was given.
    #[test]
    fn a_screen_without_a_notice_is_unchanged() {
        let screen = b"\x1b[32muser@host\x1b[0m:~$ ls\r\nCargo.toml  src\r\n".to_vec();
        assert_eq!(strip_old_notices(screen.clone()), screen);
    }

    /// The line goes with the notice, not just the words.
    ///
    /// Replacing the text and leaving its line behind would push the real output down one row
    /// per launch — the same unbounded growth wearing a blank instead of a sentence.
    #[test]
    fn the_whole_line_goes_not_only_the_sentence() {
        let once = preload_from(Some(b"output\r\n".to_vec()));
        let twice = preload_from(Some(once.clone()));
        let a = String::from_utf8(once).expect("utf8");
        let b = String::from_utf8(twice).expect("utf8");
        assert_eq!(
            a.matches("\r\n").count(),
            b.matches("\r\n").count(),
            "a restore added a line:\n{a}\n---\n{b}"
        );
    }
}
