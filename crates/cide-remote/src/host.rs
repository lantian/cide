//! The seam between this crate and cide. (M72)
//!
//! Everything a device is served arrives through [`RemoteHost`], and the reason it is a trait and
//! not a set of function calls into the application is the same reason `cide_agents::tools` has
//! `TaskSink`: the whole server becomes testable against a fake. `cargo test -p cide-remote`
//! binds a real loopback socket, speaks the real protocol over a real WebSocket, and needs no
//! workspace, no PTY, no `claude` on PATH and no display.
//!
//! # Every method is synchronous, and that is load-bearing
//!
//! The obvious shape for a trait behind an async server is `async fn`, and it would be wrong
//! here. These methods reach into the workspace mutex, the agent registry's `parking_lot::Mutex`
//! and a `TaskStore`'s, and several of them read or write the disk. Called from a tokio task,
//! each of those parks a runtime worker for as long as it blocks — and the agent lock in
//! particular is held by the hook-apply thread for the length of a state transition. So the
//! server calls every one of these through `spawn_blocking`, and making them synchronous is what
//! makes that impossible to forget: an `async fn` would invite an `.await` in the wrong place and
//! nothing would complain until a phone on a slow read froze an unrelated window.
//!
//! # Why a `String` error
//!
//! `Result<T, String>` rather than a typed error, on `AgentSink`'s precedent. The refusals a host
//! generates are cide's — *no such project*, *that run is awaiting permission*, *the tracker
//! could not be read* — and a vocabulary defined in this crate would be this crate inventing
//! names for failures it cannot produce and cannot enumerate. The sentence is carried to the
//! device in [`cide_ipc::remote::ServerBody::Error`]'s `detail`, where it is read by a person,
//! and the machine-readable half is the `kind` the server chooses from the call site.

use cide_ipc::ProjectId;
use cide_ipc::remote::{
    AwaitingEntry, InstanceInfo, PermissionPrompt, RemoteProject, RemoteSession,
};
use cide_ipc::screen::{ScreenCapture, ScrollbackCapture};

/// What cide answers for a paired device.
///
/// The surface grows one milestone phase at a time; what is here is the read half a device needs
/// to draw its lists. The screen, the input and the writes arrive with the phases that serve
/// them, so there is never a method on this trait that a host has to implement with a lie.
pub trait RemoteHost: Send + Sync + 'static {
    /// Which cide this is. Read once per connection, immediately after the handshake.
    fn instance(&self) -> InstanceInfo;

    /// The open projects, and the workspace revision they were read at.
    ///
    /// The revision travels with them because two readers of one tree can be answered out of
    /// order — `ui/src/store/workspace.ts` drops any snapshot not strictly newer than the one it
    /// holds, and a device needs the same defence for the same reason.
    fn projects(&self) -> (u64, Vec<RemoteProject>);

    /// Every pane holding a session, narrowed to one project when asked.
    fn sessions(&self, only: Option<ProjectId>) -> Vec<RemoteSession>;

    /// The *finished and not yet looked at* set, with the stamp each entry arrived at.
    fn awaiting(&self) -> Vec<AwaitingEntry>;

    /// The agent runs of one project, newest first. (M75)
    ///
    /// Added late, and the gap it closes is worth naming: `run_stop`, `run_pause` and
    /// `run_resume` were on this trait from the first cut while nothing could *list* a run, so a
    /// device could pause a run it had no way to see. A control for a thing that cannot be shown
    /// is not a feature, it is a guess with a button on it.
    fn runs(&self, project: ProjectId) -> Vec<cide_ipc::AgentRun>;

    /// The roles one project can dispatch, with how many of each are live — and whether the
    /// queue is open at all. See `RemoteRoster::dispatching`: a paused project with nothing
    /// running is indistinguishable from an idle one without it, which is exactly the bug the
    /// first cut of this shipped.
    fn roster(&self, project: ProjectId) -> cide_ipc::remote::RemoteRoster;

    /// One project's task board, as rows. (M75)
    fn board(&self, project: ProjectId) -> Vec<cide_ipc::TaskRow>;

    /// One whole task — body, comments, attachments, history. (M75)
    ///
    /// Asked for, never pushed, and that is the board's own rule: `TaskRow` exists because the
    /// board used to carry every word ever written into a tracker, so a device gets rows for a
    /// list and the contents of exactly the one it opened. `None` is "no such task", which is a
    /// fact rather than a failure — a board a device is holding can name a task somebody has
    /// since deleted.
    fn task(&self, project: ProjectId, task: cide_ipc::TaskId) -> Option<cide_ipc::TaskDetail>;

    /// A session's visible grid, or `None` when there is no such live session.
    ///
    /// Polled while a device is watching. **Never a sink**, and the argument is in
    /// [`crate::server`]'s watcher: a sink would make a phone on a train a back-pressure path
    /// into the child it is watching, because `cide-pty`'s backpressure is structural — a full
    /// channel blocks the coalescer, then the reader, then the kernel buffer, then `claude`.
    /// Polling can skip a tick; a sink cannot skip a byte.
    fn screen(&self, session: cide_ipc::SessionId) -> Option<ScreenCapture>;

    /// A page of a session's retained scrollback, counted in lines from the oldest.
    fn scrollback(
        &self,
        session: cide_ipc::SessionId,
        from_top: u32,
        rows: u16,
    ) -> Option<ScrollbackCapture>;

    /// What a session is asking, when it is asking a permission question it can read.
    ///
    /// `None` covers three different things and deliberately does not distinguish them: there is
    /// no such session, the session is not awaiting permission, or the prompt on screen could not
    /// be read with certainty. A caller does the same thing in all three cases — show the screen
    /// and the keyboard — and a device that could tell them apart would be tempted to guess.
    fn prompt(&self, session: cide_ipc::SessionId) -> Option<PermissionPrompt>;

    /// Answer a prompt by picking one of its options.
    ///
    /// `expect_screen` is the digest of the prompt the device was *shown*. The implementation
    /// re-reads the screen, re-parses it and refuses when that has moved, because over a network
    /// the prompt a thumb was travelling towards can already have been replaced by the next one.
    fn answer_prompt(
        &self,
        session: cide_ipc::SessionId,
        option: u8,
        expect_screen: &str,
    ) -> Result<(), String>;

    /// Put bytes into a session, as if they had been typed.
    ///
    /// `writer` and `seq` ride cide's existing at-least-once watermark rather than a second one
    /// invented here: it is keyed by `(session, writer)` and resets on a new `epoch`, which is
    /// exactly the shape a client that counts independently and reconnects needs. A duplicate is
    /// `Ok`, not an error — the whole point of the guard is that re-sending is safe.
    ///
    /// Bytes rather than a keystroke, because deciding *which* bytes needs the child's modes and
    /// that decision belongs in [`crate::keys`], where it can be tested without a terminal.
    fn write(
        &self,
        session: cide_ipc::SessionId,
        bytes: Vec<u8>,
        writer: &str,
        epoch: &str,
        seq: u64,
    ) -> Result<(), String>;

    /// Stop an agent run.
    ///
    /// Routed through cide's own stop, which is the whole point: `stop_route` refuses to *ask* a
    /// run that is `AwaitingPermission` — the wind-down's lone `\r` would approve the very tool
    /// call the stop was meant to prevent — and kills it instead. A remote stop implemented as
    /// "type something at it" would arrive at that bug by a new door.
    fn run_stop(
        &self,
        project: ProjectId,
        run: cide_ipc::RunId,
        reason: Option<String>,
        force: bool,
    ) -> Result<(), String>;

    /// Freeze a run, or the whole project's queue when `run` is `None`.
    ///
    /// One method with a nullable argument rather than two, because that is the shape the command
    /// underneath already has and for its reason: the two scopes share a queue, a signal and a
    /// refusal path.
    fn run_pause(&self, project: ProjectId, run: Option<cide_ipc::RunId>) -> Result<(), String>;

    /// Thaw what [`Self::run_pause`] froze.
    fn run_resume(&self, project: ProjectId, run: Option<cide_ipc::RunId>) -> Result<(), String>;

    /// Put a role onto a task, or onto a bare instruction.
    fn dispatch(&self, request: cide_ipc::DispatchRequest) -> Result<cide_ipc::RunId, String>;

    /// Create a task.
    fn task_new(&self, task: cide_ipc::TaskNew) -> Result<(), String>;

    /// Edit one.
    ///
    /// The board is not returned. A device is subscribed to `tasks-changed` and will be sent the
    /// new board as a matter of course, and answering the gesture with a board as well would be a
    /// second, weaker source of truth about the same rows — one of which is always a little
    /// older than the other.
    fn task_edit(
        &self,
        project: ProjectId,
        task: cide_ipc::TaskId,
        edit: cide_ipc::TaskEdit,
    ) -> Result<(), String>;

    /// This session has been looked at; clear its *finished and not yet seen* mark.
    ///
    /// One session. The set travels **to** a device whole and **from** one entry at a time, which
    /// is `ui/src/panes/awaiting.ts`'s rule and exists because a surface that has observed
    /// nothing would otherwise report its empty set and clear every marker in the application.
    fn acknowledge(&self, session: cide_ipc::SessionId) -> Result<(), String>;

    /// Scroll the desk's own view of a session by `pages` screenfuls — the terminal's
    /// scrollback, not the program. (M91)
    ///
    /// Only for a session on the **normal** screen; the server sends an alternate screen a wheel
    /// instead and never asks this. `Ok` when there is no pane showing the session to scroll:
    /// the device still pages its own copy, and a desk with the tab closed has nothing to keep
    /// in step with.
    ///
    /// Defaulted, like the milestone methods below, to a refusal a person can read: a host that
    /// predates them says so rather than failing to build — `fake_cide` and the test host are
    /// hosts too, and neither is about milestones.
    fn scroll_view(&self, session: cide_ipc::SessionId, pages: i8) -> Result<(), String> {
        let _ = (session, pages);
        Err("this cide cannot scroll its view from a device".to_owned())
    }

    /// A project's milestones, as the desk's Milestones tab draws them; `None` when it has none.
    /// (M91)
    fn milestones(&self, project: ProjectId) -> Option<cide_ipc::MilestonesView> {
        let _ = project;
        None
    }

    /// Start a milestone's gate in the background. The answer is the change event, twice.
    fn gate_run(&self, project: ProjectId, milestone: String) -> Result<(), String> {
        let _ = (project, milestone);
        Err("this cide does not run gates for a device".to_owned())
    }

    /// Accept a milestone whose gate passed.
    fn milestone_accept(
        &self,
        project: ProjectId,
        milestone: String,
    ) -> Result<Option<cide_ipc::MilestonesView>, String> {
        let _ = (project, milestone);
        Err("this cide does not accept milestones from a device".to_owned())
    }

    /// The last MiB of a gate's (`kind: "gate"`) or a verify's (`"verify"`) log.
    fn check_log(
        &self,
        project: ProjectId,
        kind: &str,
        key: &str,
    ) -> Result<Option<String>, String> {
        let _ = (project, kind, key);
        Ok(None)
    }
}
