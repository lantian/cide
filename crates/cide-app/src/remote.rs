//! The projection a paired device is served. (M72)
//!
//! This module is the *only* place a remote client's view of cide is built, and the discipline
//! it enforces is one sentence long: **nothing here may reach into [`cide_ipc::Workspace`] on a
//! client's behalf and hand back what it finds.** `Workspace` carries `Settings`, `Settings`
//! carries `LlmSettings` and `ProxySettings`, and those carry provider API keys and proxy
//! passwords in plaintext — which is why `workspace.json` is `0600` and why
//! [`cide_ipc::LlmProvider`] has a hand-written `Debug`. The tree is broadcast whole to every
//! *window* because a window is this process's own webview; a phone is not, and the difference
//! is the entire security argument of the feature.
//!
//! So the functions below return [`cide_ipc::remote`]'s projections and nothing else, and
//! `a_projection_carries_no_credential` plants a key in a fixture workspace and greps everything
//! they can produce. "The types cannot name `Workspace`" is true today and stops being true the
//! first time somebody adds a convenient field, which is what the test is for.
//!
//! There is no socket here yet. This is the read half, standing on its own so it can be tested
//! without one.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use cide_ipc::remote::{AwaitingEntry, InstanceInfo, RemoteProject, RemoteSession};
use cide_ipc::{ProjectId, SessionId, SessionState, Workspace};

use cide_core::remote::BindVerdict;
use cide_ipc::remote::{PermissionPrompt, RemoteDevice, RemoteStatus};
use cide_ipc::screen::{ScreenCapture, ScrollbackCapture};
use cide_ipc::{RemoteBind, RemoteSettings};
use cide_remote::seal::StaticKey;
use cide_remote::{DeviceStore, RemoteHost, RemoteServer};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use crate::agents::AgentRegistry;
use crate::hooks::HookServer;
use crate::state::SessionRegistry;
use crate::workspace_state::WorkspaceState;

/// Which cide this is. See [`cide_core::remote::instance_id`] for why the id is its own file.
pub fn instance_info() -> InstanceInfo {
    InstanceInfo {
        id: cide_core::remote::instance_id(),
        name: cide_core::remote::display_name(),
        profile: cide_core::profile::active().map(str::to_owned),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// The open projects, in the order the header draws them.
pub fn projects(ws: &Workspace) -> Vec<RemoteProject> {
    ws.projects
        .iter()
        .map(|(id, project)| RemoteProject {
            id: *id,
            name: project.name.clone(),
            display_path: project.display_path.clone(),
            dot: project.dot.clone(),
        })
        .collect()
}

/// What a session's state and its run are, for panes this workspace holds.
///
/// Split out as a struct rather than three arguments because the two callers that will exist —
/// the snapshot and the per-project push — must assemble it identically, and a positional triple
/// of maps is the shape that gets swapped.
pub struct SessionFacts {
    states: HashMap<SessionId, SessionState>,
    awaiting: HashSet<SessionId>,
    runs: HashMap<SessionId, cide_ipc::AgentRun>,
}

impl SessionFacts {
    /// Gather everything that is not in the tree.
    ///
    /// The awaiting set is read from [`crate::windows`] rather than derived from the states,
    /// and that is not an optimisation — it is the one fact this process cannot compute.
    /// "Finished a turn and has not been looked at" needs a bit of history `SessionState` does
    /// not carry and an acknowledgement only a surface can report, so the frontend decides and
    /// `windows.rs` aggregates (`ui/src/panes/awaiting.ts` argues it at length). A second
    /// derivation here would be a fourth surface disagreeing with the other three.
    pub fn gather(
        states: HashMap<SessionId, SessionState>,
        agents: Option<&AgentRegistry>,
        ws: &Workspace,
        only: Option<ProjectId>,
    ) -> Self {
        let mut runs = HashMap::new();
        if let Some(agents) = agents {
            for (id, _) in &ws.projects {
                if only.is_some_and(|wanted| wanted != *id) {
                    continue;
                }
                for run in agents.runs_for(*id) {
                    if let Some(session) = run.session {
                        runs.insert(session, run);
                    }
                }
            }
        }
        Self {
            states,
            awaiting: crate::windows::awaiting_sessions().into_iter().collect(),
            runs,
        }
    }

    /// A session's state, or `Spawning` — which is [`HookServer::state`]'s own default and is
    /// the honest answer for a pane whose child has said nothing yet.
    fn state(&self, session: SessionId) -> SessionState {
        self.states
            .get(&session)
            .copied()
            .unwrap_or(SessionState::Spawning)
    }
}

/// Every pane holding a session, projected.
///
/// The enumeration is [`cide_core::workspace::session_panes`], which walks tabs **and**
/// `project.detached`. A walk over tabs alone silently omits a torn-out pane — see that
/// function's own header for the bug it was extracted from.
///
/// Mirrored panes are *not* de-duplicated here. Two panes showing one child are two rows with
/// one `session`, because that is what is on screen, and only the consumer knows whether it is
/// listing panes or conversations.
pub fn sessions(
    ws: &Workspace,
    facts: &SessionFacts,
    only: Option<ProjectId>,
) -> Vec<RemoteSession> {
    cide_core::workspace::session_panes(ws, only)
        .into_iter()
        .map(|found| {
            let run = facts.runs.get(&found.session);
            RemoteSession {
                session: found.session,
                project: found.project,
                pane: found.pane.id,
                tab: found.tab,
                tab_title: found.tab_title.clone(),
                title: found.pane.title.clone(),
                kind: found.pane.kind,
                role: found.pane.role,
                state: facts.state(found.session),
                awaiting: facts.awaiting.contains(&found.session),
                run: run.map(|r| r.run),
                agent: run.map(|r| r.agent.clone()),
                task: run.and_then(|r| r.task.clone()),
            }
        })
        .collect()
}

/// The *finished and not yet looked at* set, with the stamps a device needs to tell news from
/// what it has already announced.
pub fn awaiting() -> Vec<AwaitingEntry> {
    crate::windows::awaiting_since()
}

/// Run one of cide's `async` command bodies from a synchronous [`RemoteHost`] method.
///
/// Three of the gestures a device can make — dispatch, and the two task writes — are `async`
/// commands whose bodies already hop to a blocking thread for the disk work. Re-implementing
/// their synchronous cores here would be three second spellings of code that fires mention
/// triggers, auto-dispatch and an event apiece, so the bridge is the smaller risk: one function,
/// one argument, written down once.
///
/// **Safe because of where it is called from, and nowhere else.** Every `RemoteHost` method
/// reaches this process through `cide-remote`'s `spawn_blocking`, so the caller is a thread from
/// tokio's *blocking* pool: the runtime context is set, which is what makes `Handle::current`
/// answer, and the thread is not a runtime worker, which is what makes blocking on it legal
/// rather than a panic. Nothing else may call this. A `Handle` that is not there is reported as
/// a sentence rather than as a panic, because the one situation that could produce it — a call
/// from outside the remote runtime — is a programming error that must not take the application
/// down with it.
fn on_the_runtime<F>(future: F) -> Result<F::Output, String>
where
    F: std::future::Future,
{
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return Err("this gesture reached cide from outside its remote runtime".to_owned());
    };
    Ok(handle.block_on(future))
}

/// [`cide_remote::RemoteHost`], over this application.
///
/// Every method is one of the projections above, read under the workspace lock and nothing more.
/// It holds an [`AppHandle`] rather than the three registries because a registry is `manage`d
/// after the handle exists and a host built from them at startup would be built from whichever
/// ones happened to be ready — `try_state` asks at the moment of the question, and answers
/// honestly when the answer is *not yet*.
pub struct AppRemoteHost {
    app: AppHandle,
}

impl AppRemoteHost {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    /// The agent registry, or the sentence to send back when there is not one yet.
    ///
    /// A device can be connected before every registry is `manage`d — the listener comes up with
    /// the application — so *not yet* is an ordinary answer and deserves a sentence rather than a
    /// panic or a silence.
    fn registry(&self) -> Result<std::sync::Arc<AgentRegistry>, String> {
        self.app
            .try_state::<std::sync::Arc<AgentRegistry>>()
            .map(|state| std::sync::Arc::clone(&state))
            .ok_or_else(|| "this cide is still starting".to_owned())
    }

    /// The facts a session projection needs, gathered from whichever registries exist.
    ///
    /// A missing registry is not an error. cide serves a device from the moment the listener is
    /// up, and a hook server that has not started yet means *no session has said anything yet*,
    /// which is exactly what `SessionState::Spawning` says.
    fn facts(&self, ws: &Workspace, only: Option<ProjectId>) -> SessionFacts {
        let agents = self.app.try_state::<std::sync::Arc<AgentRegistry>>();
        let agents = agents.as_deref().map(std::sync::Arc::as_ref);
        // Every state a window was told, not the hook server's map: that one holds only what
        // hooks said, and was read here through `live_sessions` — `Busy`/`AwaitingPermission`
        // alone — so every finished turn reached a phone as `Spawning`. See
        // `emit::last_session_states`.
        SessionFacts::gather(crate::emit::last_session_states(), agents, ws, only)
    }
}

impl RemoteHost for AppRemoteHost {
    fn instance(&self) -> InstanceInfo {
        instance_info()
    }

    fn projects(&self) -> (u64, Vec<RemoteProject>) {
        let Some(state) = self.app.try_state::<WorkspaceState>() else {
            return (0, Vec::new());
        };
        state.with(|ws| (ws.rev, projects(ws)))
    }

    fn sessions(&self, only: Option<ProjectId>) -> Vec<RemoteSession> {
        let Some(state) = self.app.try_state::<WorkspaceState>() else {
            return Vec::new();
        };
        // The facts are gathered *inside* the same `with`, so the tree and the states a row is
        // built from are read at one instant. Two reads would let a session appear in the list
        // with the state it held before the pane that holds it moved.
        state.with(|ws| {
            let facts = self.facts(ws, only);
            sessions(ws, &facts, only)
        })
    }

    /// This project's runs, from the registry that owns them. (M75)
    ///
    /// The same `runs_for` the panel draws and the same one `SessionFacts` joins against, so a
    /// device and a window cannot disagree about what is running.
    fn runs(&self, project: ProjectId) -> Vec<cide_ipc::AgentRun> {
        self.app
            .try_state::<Arc<AgentRegistry>>()
            .map(|registry| registry.runs_for(project))
            .unwrap_or_default()
    }

    /// The roles, projected, with a live count apiece. (M75)
    ///
    /// Built from `project_roster` — the one function that reads the files *and* the registry —
    /// rather than from either alone, so the roster a device sees is the roster the panel sees.
    /// `AgentRoster::Disabled` and `::Empty` are both an empty list here: a device's roster
    /// screen says "no roles" for either, and the two sentences that tell them apart name a
    /// config path on a machine the person holding the phone is not sitting at.
    fn roster(&self, project: ProjectId) -> cide_ipc::remote::RemoteRoster {
        let Some(cide_ipc::AgentRoster::Ready {
            agents,
            runs,
            dispatching,
        }) = crate::cmd::agents::project_roster(&self.app, project)
        else {
            // `Disabled` and `Empty` both mean nothing can be dispatched, so `dispatching` is
            // false rather than true — a device drawing "queue open, no roles" about a project
            // with subagents switched off would be offering a Resume for a queue that does not
            // exist.
            return cide_ipc::remote::RemoteRoster {
                agents: Vec::new(),
                dispatching: false,
            };
        };
        let agents = agents
            .into_iter()
            .map(|def| {
                // Counted here, once, for the reason `RemoteAgent`'s own doc gives: a device
                // re-deriving these from the runs list would be a second producer of a number
                // that is only ever visibly wrong beside the list it counts.
                let mine = runs.iter().filter(|run| run.agent == def.id);
                let running = mine
                    .clone()
                    .filter(|run| {
                        matches!(
                            run.state,
                            cide_ipc::RunState::Starting
                                | cide_ipc::RunState::Running
                                | cide_ipc::RunState::Idle
                                | cide_ipc::RunState::AwaitingPermission
                                | cide_ipc::RunState::Paused { .. }
                        )
                    })
                    .count() as u32;
                let queued = mine
                    .filter(|run| matches!(run.state, cide_ipc::RunState::Queued))
                    .count() as u32;
                cide_ipc::remote::RemoteAgent {
                    id: def.id,
                    label: def.label,
                    scope: def.scope,
                    harness: def.harness,
                    description: def.description,
                    model: def.model,
                    color: def.color,
                    unavailable: def.unavailable,
                    max_concurrent: def.max_concurrent,
                    worktree: def.worktree,
                    running,
                    queued,
                }
            })
            .collect();
        cide_ipc::remote::RemoteRoster {
            agents,
            dispatching,
        }
    }

    /// One whole task, from the same store the board came from. (M75)
    fn task(&self, project: ProjectId, task: cide_ipc::TaskId) -> Option<cide_ipc::TaskDetail> {
        let state = self.app.try_state::<WorkspaceState>()?;
        let root = crate::tasks_state::project_root(&state, project).ok()?;
        let stores = self
            .app
            .try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>()?;
        // The same two steps `task_get` takes, so a device and the card read one task the
        // same way.
        stores
            .ensure(project, &root)
            .get(&task)
            .as_ref()
            .map(cide_tasks::detail_of)
    }

    /// This project's board, as rows. (M75)
    fn board(&self, project: ProjectId) -> Vec<cide_ipc::TaskRow> {
        let Some(state) = self.app.try_state::<WorkspaceState>() else {
            return Vec::new();
        };
        let Ok(root) = crate::tasks_state::project_root(&state, project) else {
            return Vec::new();
        };
        let Some(stores) = self
            .app
            .try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>()
        else {
            return Vec::new();
        };
        // `ensure` rather than `get`, for `cmd::tasks::tracker`'s stated reason: the restore
        // loop warms a store per project but is capped at 32, and a project past the cap would
        // otherwise answer "no tasks" for ever with nothing saying why.
        match stores.ensure(project, &root).board() {
            cide_ipc::TaskBoard::Ready { tasks, .. } => tasks,
            // `Absent` and `Unreadable` both flatten to an empty list, and that is a real
            // simplification rather than an oversight: both of their sentences name a path on a
            // machine the person holding the phone is not sitting at, and neither offers an
            // action a device could take. A screen saying "no tasks" is the honest reading of
            // both from here. `AgentRoster`'s two empty arms flatten the same way, above.
            _ => Vec::new(),
        }
    }

    fn awaiting(&self) -> Vec<AwaitingEntry> {
        awaiting()
    }

    fn screen(&self, session: cide_ipc::SessionId) -> Option<ScreenCapture> {
        // `None` for a session this process does not hold, which is an ordinary answer: a device
        // may be watching a pane that has since closed, and the server turns this into a sentence
        // rather than into silence.
        Some(
            self.app
                .try_state::<SessionRegistry>()?
                .get(session)?
                .capture_screen(),
        )
    }

    fn scrollback(
        &self,
        session: cide_ipc::SessionId,
        from_top: u32,
        rows: u16,
    ) -> Option<ScrollbackCapture> {
        Some(
            self.app
                .try_state::<SessionRegistry>()?
                .get(session)?
                .capture_scrollback(from_top, rows),
        )
    }

    fn prompt(&self, session: cide_ipc::SessionId) -> Option<PermissionPrompt> {
        // The state comes from the hook and the screen comes from the mirror, and both are read
        // here rather than passed in: `parse` refuses anything whose session is not awaiting
        // permission, and that gate is only worth having if the state it reads is the state now.
        let hooks = self.app.try_state::<HookServer>()?;
        let capture = self
            .app
            .try_state::<SessionRegistry>()?
            .get(session)?
            .capture_screen();
        cide_claude::permission::parse(&capture, hooks.state(session))
    }

    fn answer_prompt(
        &self,
        session: cide_ipc::SessionId,
        option: u8,
        expect_screen: &str,
    ) -> Result<(), String> {
        // Re-read, re-parse, compare — in that order and at this moment. Over a network the
        // prompt a thumb was travelling towards can already have been replaced by the next one,
        // and without this a tap on "No, tell Claude what to do differently" lands as "1. Yes"
        // on a question the user never saw. This is the single most important line in the
        // feature.
        let Some(prompt) = self.prompt(session) else {
            return Err("that session is not asking a permission question".to_owned());
        };
        if prompt.digest != expect_screen {
            return Err("that prompt has changed since you were shown it".to_owned());
        }
        if !prompt.options.iter().any(|o| o.number == option) {
            return Err("that prompt does not offer that option".to_owned());
        }
        let Some(bytes) = cide_claude::permission::answer_bytes(option) else {
            return Err("that is not an option a keystroke can pick".to_owned());
        };

        let Some(pty) = self
            .app
            .try_state::<SessionRegistry>()
            .and_then(|registry| registry.get(session))
        else {
            return Err("that session is not running here".to_owned());
        };
        // No watermark, and no sequence number. An answer is not a keystroke a device might
        // re-send after a dropped socket — it is guarded by the digest instead, which is
        // strictly stronger: a repeat that arrives after the prompt has moved is refused, and one
        // that arrives while it has not is the same answer to the same question.
        pty.write(bytes);
        Ok(())
    }

    fn write(
        &self,
        session: cide_ipc::SessionId,
        bytes: Vec<u8>,
        writer: &str,
        epoch: &str,
        seq: u64,
    ) -> Result<(), String> {
        let Some(registry) = self.app.try_state::<SessionRegistry>() else {
            return Err("this cide is still starting".to_owned());
        };
        let Some(pty) = registry.get(session) else {
            return Err("that session is not running here".to_owned());
        };
        // cide's own at-least-once watermark, not a second one. A duplicate is `Ok` and not an
        // error: the guard exists so that re-sending after a dropped socket is safe, and
        // reporting a refusal would make a device try to "fix" a keystroke that already landed.
        if registry.accept_write(session, writer, Some(epoch), Some(seq)) {
            pty.write(bytes);
        }
        Ok(())
    }

    fn run_stop(
        &self,
        project: ProjectId,
        run: cide_ipc::RunId,
        reason: Option<String>,
        force: bool,
    ) -> Result<(), String> {
        // `StopBy::User` and not a third variant. That enum distinguishes *whose decision* a stop
        // was — an orchestrating session's or a person's — and a person pressing Stop on their
        // phone is a person. Inventing `StopBy::Device` would put a distinction into the run's
        // epitaph that the record does not need, and `agents.rs` is explicit that the existing
        // sentences do not move.
        crate::cmd::agents::agents_stop_blocking(
            &self.app,
            project,
            run,
            crate::agents::StopBy::User,
            reason,
            force,
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn run_pause(&self, project: ProjectId, run: Option<cide_ipc::RunId>) -> Result<(), String> {
        self.registry()?
            .pause(&self.app, project, run)
            .map_err(|error| error.to_string())
    }

    fn run_resume(&self, project: ProjectId, run: Option<cide_ipc::RunId>) -> Result<(), String> {
        self.registry()?
            .resume(&self.app, project, run)
            .map_err(|error| error.to_string())
    }

    fn dispatch(&self, request: cide_ipc::DispatchRequest) -> Result<cide_ipc::RunId, String> {
        let app = self.app.clone();
        on_the_runtime(async move {
            let state = app.state::<WorkspaceState>();
            let agents = app.state::<std::sync::Arc<AgentRegistry>>();
            let tasks = app.state::<std::sync::Arc<crate::tasks_state::TasksStores>>();
            crate::cmd::agents::agents_dispatch(app.clone(), state, agents, tasks, request).await
        })?
        .map_err(|error| error.to_string())
    }

    fn task_new(&self, task: cide_ipc::TaskNew) -> Result<(), String> {
        let app = self.app.clone();
        on_the_runtime(async move {
            let state = app.state::<WorkspaceState>();
            let stores = app.state::<std::sync::Arc<crate::tasks_state::TasksStores>>();
            crate::cmd::tasks::task_new(app.clone(), state, stores, task).await
        })?
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn task_edit(
        &self,
        project: ProjectId,
        task: cide_ipc::TaskId,
        edit: cide_ipc::TaskEdit,
    ) -> Result<(), String> {
        let app = self.app.clone();
        on_the_runtime(async move {
            let state = app.state::<WorkspaceState>();
            let stores = app.state::<std::sync::Arc<crate::tasks_state::TasksStores>>();
            crate::cmd::tasks::task_edit(app.clone(), state, stores, project, task, edit).await
        })?
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn acknowledge(&self, session: cide_ipc::SessionId) -> Result<(), String> {
        let Some(state) = self.app.try_state::<WorkspaceState>() else {
            return Err("this cide is still starting".to_owned());
        };
        // The same road a pointerdown in a pane title bar takes — see `report_awaiting`. Every
        // window retitles and every webview converges, which is what makes "opening it on the
        // phone clears the mark on the desktop" true rather than merely intended.
        crate::cmd::window::report_awaiting(
            &self.app,
            &state,
            session,
            false,
            crate::cmd::window::AwaitingReporter::Device,
        );
        Ok(())
    }

    /// PgUp/PgDn from a device, on a normal screen: the pane showing the session scrolls its own
    /// scrollback, as Shift+PgUp at the desk would. (M91)
    ///
    /// An event rather than a write, because the scroll position is **xterm's** — it lives in
    /// the webview and never touches the vt100 mirror or the child. Every window is told; the
    /// one whose pane host holds the session acts, and a desk with the tab closed does nothing,
    /// which is `Ok` — the device still pages its own copy.
    fn scroll_view(&self, session: cide_ipc::SessionId, pages: i8) -> Result<(), String> {
        crate::emit::remote_scroll(&self.app, session, pages);
        Ok(())
    }

    /// The same one producer the desk's Milestones tab reads. (M91)
    fn milestones(&self, project: ProjectId) -> Option<cide_ipc::MilestonesView> {
        crate::milestones::view(&self.app, project)
    }

    /// Only the **active** milestone's gate runs, as on the desk — and a device naming another
    /// one is refused rather than silently running the active one: its screen is out of date,
    /// and the gate it thinks it started is not the one that would run.
    fn gate_run(&self, project: ProjectId, milestone: String) -> Result<(), String> {
        self.is_active(project, &milestone)?;
        crate::milestones::run_gate(&self.app, project);
        Ok(())
    }

    fn milestone_accept(
        &self,
        project: ProjectId,
        milestone: String,
    ) -> Result<Option<cide_ipc::MilestonesView>, String> {
        self.is_active(project, &milestone)?;
        crate::milestones::accept(&self.app, project)?;
        Ok(crate::milestones::view(&self.app, project))
    }

    fn check_log(
        &self,
        project: ProjectId,
        kind: &str,
        key: &str,
    ) -> Result<Option<String>, String> {
        let kind = match kind {
            "gate" => crate::milestones::LogKind::Gate,
            "verify" => crate::milestones::LogKind::Verify,
            other => return Err(format!("no check log of kind `{other}`")),
        };
        let Some(state) = self.app.try_state::<WorkspaceState>() else {
            return Err("this cide is still starting".to_owned());
        };
        let root = crate::tasks_state::project_root(&state, project)
            .map_err(|_| "that project is not open here".to_owned())?;
        Ok(crate::milestones::read_log(&root, kind, key))
    }
}

impl AppRemoteHost {
    /// Refuse an act aimed at a milestone that is not the active one. See `gate_run`.
    fn is_active(&self, project: ProjectId, milestone: &str) -> Result<(), String> {
        let view =
            crate::milestones::view(&self.app, project).ok_or("that project is not open here")?;
        match view.plan.current() {
            Some(current) if current.id == milestone => Ok(()),
            Some(current) => Err(format!(
                "`{milestone}` is not the active milestone — `{}` is",
                current.id
            )),
            None => Err("this project has no milestones".to_owned()),
        }
    }
}

// --- the listener's life ------------------------------------------------------------------

/// The remote subsystem: a runtime, the device list, and the server when one is running.
///
/// Modelled on [`crate::ide::IdeServers`], and for its reasons — a tokio runtime owned by the
/// application rather than borrowed from Tauri, so it can be started and stopped without
/// touching anything else. Its **own** runtime and not `ide.rs`'s: the two features are
/// independently switchable, and a phone's socket must not be able to starve the IDE server that
/// every `claude` in the app depends on.
pub struct RemoteState {
    rt: tokio::runtime::Runtime,
    devices: Arc<DeviceStore>,
    /// This instance's long-lived key. Its public half goes in every pairing payload; a device
    /// that cannot complete an exchange against it is not talking to this cide.
    statik: Arc<StaticKey>,
    /// `None` when the listener is off, which is the default.
    server: Mutex<Option<Running>>,
    /// The last thing that happened, so the panel can be told without asking the socket.
    status: Mutex<RemoteStatus>,
}

/// A listener, and what it was bound for.
///
/// The two travel in one `Option` rather than in two fields because [`reconcile`] compares them
/// to decide whether there is anything to do, and a pair of fields that could disagree about
/// which socket is open is exactly the drift that comparison exists to detect.
struct Running {
    server: Arc<RemoteServer>,
    addr: IpAddr,
    port: u16,
}

/// Built before the application exists, installed once inside `setup`.
///
/// `#[must_use]` for [`crate::ide::PendingIdeServers`]' stated reason: a subsystem that is never
/// installed is one every restoring launch silently comes up without, and nothing about the
/// absence is a type error until the binding it leaves behind is unused.
#[must_use = "a remote subsystem that is never installed serves no device"]
pub struct PendingRemote(RemoteState);

impl PendingRemote {
    /// Build the runtime and read the device list.
    ///
    /// A device list that will not parse is **not** fatal to the application and **is** fatal to
    /// the feature: it becomes a `Refused` status with the sentence in it. Starting empty would
    /// unpair every device with nothing anywhere saying why, and unlike a workspace there is no
    /// gesture that rebuilds it.
    pub fn new() -> std::io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("cide-remote")
            .build()?;
        let state_dir = cide_core::persist::state_dir();
        let (devices, mut status) = match DeviceStore::load(state_dir.join(DEVICES_FILE)) {
            Ok(devices) => (Arc::new(devices), RemoteStatus::Off),
            Err(error) => (
                Arc::new(DeviceStore::ephemeral()),
                RemoteStatus::Refused {
                    why: error.to_string(),
                },
            ),
        };
        // Same trade as the device list: a key file that is not a key is refused rather than
        // replaced, because minting over it would unpair every device with no message anywhere.
        // An ephemeral key lets the rest of the application start; nothing can pair against it,
        // which the status says.
        let statik = match StaticKey::load_or_mint(&state_dir.join(KEY_FILE)) {
            Ok(key) => Arc::new(key),
            Err(error) => {
                if matches!(status, RemoteStatus::Off) {
                    status = RemoteStatus::Refused {
                        why: error.to_string(),
                    };
                }
                Arc::new(StaticKey::ephemeral())
            }
        };
        Ok(Self(RemoteState {
            rt,
            devices,
            statik,
            server: Mutex::new(None),
            status: Mutex::new(status),
        }))
    }

    /// Manage the state, and start the listener if the restored settings ask for it.
    pub fn install(self, app: &AppHandle, ws: &Workspace) {
        let wanted = ws.settings.remote.clone();
        app.manage(self.0);
        if wanted.enabled {
            reconcile(app, &wanted);
        }
    }
}

/// Where the device list lives, relative to the profile's state directory.
const DEVICES_FILE: &str = "remote-devices.json";

/// And the instance's long-lived key, in its own file beside it.
///
/// Its own file rather than a field of the device list, for [`cide_core::remote::instance_id`]'s
/// reason one step further: revoking every device must not change which cide this *is*, or every
/// previously paired device would refuse the next handshake having been given no reason to.
const KEY_FILE: &str = "remote-key";

/// Start, stop or restart the listener so that it matches the settings.
///
/// One function for all three because they are one question — *is what is running what was
/// asked for* — and three entry points would be three places to get the answer half right. It is
/// called from `settings_set` when the group moves and from `install` at boot.
///
/// **A listener that already matches is left alone, and that is load-bearing rather than an
/// optimisation.** Two of the four callers do not move the socket at all: opening a pairing
/// window and closing one change *who may pair*, which the running server reads out of the
/// device store on every handshake, and never *what is bound*. Restarting for them cost two
/// things, both bad. Every connected phone was dropped mid-session by a gesture that had nothing
/// to do with it — cancelling a pairing window disconnected the device that had just used it.
/// And the stop is **synchronous**: `emit_going_away` waits for the aborted tasks and for the
/// connections to drain, while a `#[tauri::command]` that is not `async` runs its body on the
/// GTK main thread, which is also the thread every `app.emit` from a host call is waiting on.
/// So a Cancel with a phone connected froze the whole window for the drain timeout, having
/// disconnected the phone to do it. Since M74 made `shutdown` actually wait — it used to reach
/// the server through `Arc::try_unwrap` and silently do nothing — that stall is new, which is
/// why it appeared with the fix rather than before it.
pub fn reconcile(app: &AppHandle, wanted: &RemoteSettings) {
    let Some(state) = app.try_state::<RemoteState>() else {
        return;
    };

    // The socket exists for paired devices and for an open pairing window, and a cide with the
    // feature on and neither is a listener on a café network answering questions for no one.
    let serve =
        wanted.enabled && (!state.devices.is_empty() || state.devices.pending_code().is_some());

    if serve && already_serving(&state, wanted) {
        // Nothing about the socket moved. The readout still goes out, because the live half of
        // it — how many devices are connected — is read from the server and a device arriving
        // is one of the things that brings us here.
        crate::emit::remote_changed(app);
        return;
    }

    // Stop first, always. A restart that bound before releasing would refuse itself with
    // `AddrInUse` and report the refusal as though the port belonged to somebody else. Bound to
    // a name rather than taken inside the `if let`, because a `MutexGuard` in an `if let`
    // scrutinee lives until the end of the block — which would hold the lock across the drain.
    let running = state.server.lock().take();
    if let Some(running) = running {
        emit_going_away(&state.rt, running.server);
    }
    crate::emit::clear_remote_tee();

    if !serve {
        *state.status.lock() = RemoteStatus::Off;
        crate::emit::remote_changed(app);
        return;
    }

    let status = match start(app, &state, wanted) {
        Ok(status) => status,
        Err(why) => RemoteStatus::Refused { why },
    };
    *state.status.lock() = status;
    crate::emit::remote_changed(app);
}

/// Is the socket that is open the socket that was asked for?
///
/// Only the two facts that decide the socket: which address it is bound to, and — when the user
/// named a port — which port. A derived port (`0`) is satisfied by whatever the band gave us,
/// because nothing promised a particular number to anybody; that asymmetry is `bind_somewhere`'s
/// and is restated here rather than re-derived, since the two must agree about what “the port
/// that was asked for” means.
///
/// Settings that no longer name an address cide may bind answer **false**: that is a refusal to
/// report rather than a listener to keep, so the ordinary road takes the socket down and says so.
///
/// Pure, and taking the running pair rather than the state, because it is the whole of the new
/// rule and the thing worth pinning: answering `true` where it should answer `false` leaves a
/// device talking to a socket the settings have disowned, and answering `false` where it should
/// answer `true` is the freeze this was written to remove.
fn same_socket(running: Option<(IpAddr, u16)>, wanted: &RemoteSettings) -> bool {
    let (Some((addr, port)), Ok(asked)) = (running, bind_address(wanted)) else {
        return false;
    };
    asked == addr && (wanted.port == 0 || wanted.port == port)
}

fn already_serving(state: &RemoteState, wanted: &RemoteSettings) -> bool {
    let running = state.server.lock().as_ref().map(|r| (r.addr, r.port));
    same_socket(running, wanted)
}

fn start(
    app: &AppHandle,
    state: &RemoteState,
    wanted: &RemoteSettings,
) -> Result<RemoteStatus, String> {
    let host: Arc<dyn RemoteHost> = Arc::new(AppRemoteHost::new(app.clone()));
    let bind_ip = bind_address(wanted)?;
    let (server, port) = bind_somewhere(state, bind_ip, wanted, host)?;

    let server = Arc::new(server);
    // Remembered so a collision is self-healing: the second instance walked the band once and
    // must keep the number every device it paired now knows.
    if let Err(error) = state.devices.remember_port(port) {
        tracing::warn!(%error, "remote: the bound port could not be remembered");
    }

    // The tee, and the pump that drains it. Installed only while a listener exists, so an
    // installation with the feature off pays one uncontended read lock per emit.
    let (tx, mut rx) = tokio::sync::mpsc::channel(TEE_CAPACITY);
    crate::emit::install_remote_tee(tx);
    let pumping = Arc::clone(&server);
    state.rt.spawn(async move {
        while let Some(event) = rx.recv().await {
            pumping.notify(event);
        }
    });

    // The panel wants to know when a device arrives or goes. Drained rather than left to fill:
    // the channel is bounded and a full one would make the server drop events it could have
    // reported.
    let mut events = server.events();
    let reporting = app.clone();
    state.rt.spawn(async move {
        while let Some(event) = events.recv().await {
            tracing::info!(?event, "remote");
            crate::emit::remote_changed(&reporting);
        }
    });

    *state.server.lock() = Some(Running {
        server,
        addr: bind_ip,
        port,
    });
    Ok(RemoteStatus::Listening {
        addresses: offered_addresses(bind_ip),
        port,
        connected: 0,
    })
}

/// How deep the tee's queue is. See `emit.rs`'s own header for why it is bounded at all.
const TEE_CAPACITY: usize = 256;

/// Which address to bind, refusing the one decision that must be made deliberately.
fn bind_address(wanted: &RemoteSettings) -> Result<IpAddr, String> {
    let addr = match &wanted.bind {
        RemoteBind::Loopback => IpAddr::V4(Ipv4Addr::LOCALHOST),
        RemoteBind::Network => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        RemoteBind::Address { address } => address
            .parse::<IpAddr>()
            .map_err(|_| format!("{address} is not an address cide can bind"))?,
    };
    match cide_core::remote::bind_policy(addr) {
        BindVerdict::Private | BindVerdict::Unspecified => Ok(addr),
        // Named, and refused rather than quietly bound. Reaching the internet is a different
        // decision from reaching the house, and this is the switch that says so.
        BindVerdict::Public if !wanted.allow_non_private => Err(format!(
            "{addr} is reachable from the internet — turn on \"allow a public address\" if that is what you meant"
        )),
        BindVerdict::Public => Ok(addr),
    }
}

/// Bind the port that was asked for, or walk the band when nobody asked for one.
///
/// The asymmetry is the whole rule. A number the user typed is a promise to a device that saved
/// it, so a taken port **refuses and names it**; sliding to another would point that device at
/// whatever else is listening, and typing into another cide's session is the failure at the end
/// of that road. A *derived* port was promised to nobody yet, so it may move — and the readout
/// says where it landed.
fn bind_somewhere(
    state: &RemoteState,
    addr: IpAddr,
    wanted: &RemoteSettings,
    host: Arc<dyn RemoteHost>,
) -> Result<(RemoteServer, u16), String> {
    if wanted.port != 0 {
        let server = bind_one(state, addr, wanted.port, Arc::clone(&host)).map_err(|error| {
            format!(
                "port {} is in use — something else on this machine is listening on it: {error}",
                wanted.port
            )
        })?;
        return Ok((server, wanted.port));
    }

    let preferred = state
        .devices
        .remembered_port()
        .unwrap_or_else(|| cide_core::remote::port_for_profile(cide_core::profile::active()));
    let base = cide_core::remote::PORT_BASE;
    let band = base..base + cide_core::remote::PORT_BAND;
    let mut last = String::new();
    for port in std::iter::once(preferred).chain(band) {
        match bind_one(state, addr, port, Arc::clone(&host)) {
            Ok(server) => return Ok((server, port)),
            Err(error) => last = error.to_string(),
        }
    }
    Err(format!(
        "no port between {base} and {} was free: {last}",
        base + cide_core::remote::PORT_BAND
    ))
}

fn bind_one(
    state: &RemoteState,
    addr: IpAddr,
    port: u16,
    host: Arc<dyn RemoteHost>,
) -> Result<RemoteServer, cide_remote::RemoteError> {
    state.rt.block_on(RemoteServer::bind(
        SocketAddr::new(addr, port),
        host,
        Arc::clone(&state.devices),
        Arc::clone(&state.statik),
    ))
}

/// What to print beside the port.
///
/// A wildcard bind is every interface, so the useful answer is the list of addresses a phone
/// could actually type — which `cide_core::remote::local_addresses` already excludes loopback and
/// link-local from. A specific bind is its own address and nothing else.
fn offered_addresses(addr: IpAddr) -> Vec<String> {
    if addr.is_unspecified() {
        let found = cide_core::remote::local_addresses();
        if !found.is_empty() {
            return found.iter().map(ToString::to_string).collect();
        }
    }
    vec![addr.to_string()]
}

/// Tell every connected device the machine is going away, then let the sockets close.
fn emit_going_away(rt: &tokio::runtime::Runtime, server: Arc<RemoteServer>) {
    // The tee first: dropping the sender is what lets the event pump's `recv` finish, so its
    // clone of the `Arc` goes away. Nothing below depends on that having happened yet — which
    // is the fix, and used to be the bug.
    crate::emit::clear_remote_tee();

    // Unconditionally, through the `Arc`. This was `Arc::try_unwrap` with a comment claiming
    // "the sockets close either way", and that was simply false: the pump holds a clone for as
    // long as its channel is open, so `try_unwrap` returned `Err` on every stop that mattered
    // and the losing arm dropped a refcount. The accept loop kept the `TcpListener` for the
    // life of the process, so the next `reconcile` — a cancelled pairing window reopening, or
    // the feature being switched off and on — answered *port already in use* against cide's
    // own orphan, for ever. With a **configured** port that is terminal, because a port the
    // user typed is a promise to a device and `bind_somewhere` refuses rather than sliding to
    // another one; with a derived port it merely walked the band and looked like it worked.
    //
    // `shutdown` now takes `&self` and joins its tasks, so when this returns the port is free
    // and `reconcile` may bind it on the very next line.
    rt.block_on(server.shutdown());
}

/// Stop the listener. Called from the shutdown ladder, before any signal reaches a child.
pub fn stop(app: &AppHandle) {
    let Some(state) = app.try_state::<RemoteState>() else {
        return;
    };
    let running = state.server.lock().take();
    if let Some(running) = running {
        emit_going_away(&state.rt, running.server);
    }
    crate::emit::clear_remote_tee();
    *state.status.lock() = RemoteStatus::Off;
}

/// What the panel draws.
pub fn status(app: &AppHandle) -> RemoteStatus {
    let Some(state) = app.try_state::<RemoteState>() else {
        return RemoteStatus::Off;
    };
    let mut status = state.status.lock().clone();
    if let (RemoteStatus::Listening { connected, .. }, Some(running)) =
        (&mut status, state.server.lock().as_ref())
    {
        *connected = running.server.connected() as u32;
    }
    status
}

/// Whether the pairing window is still open, and the device standing at it.
///
/// Both read on demand rather than remembered in the app, because each is only ever correct in
/// the present tense — see `RemoteServer::pairing_attempt` for the digits, and `DeviceStore`
/// for the code, which is taken by a redemption and forgotten at the end of its two minutes with
/// nothing emitted either time.
pub fn pairing_progress(app: &AppHandle) -> cide_ipc::remote::PairingProgress {
    let Some(state) = app.try_state::<RemoteState>() else {
        return cide_ipc::remote::PairingProgress {
            open: false,
            attempt: None,
        };
    };
    let attempt = state
        .server
        .lock()
        .as_ref()
        .and_then(|running| running.server.pairing_attempt());
    cide_ipc::remote::PairingProgress {
        open: state.devices.pending_code().is_some(),
        attempt,
    }
}

/// The paired devices, for the panel's list.
pub fn devices(app: &AppHandle) -> Vec<RemoteDevice> {
    let Some(state) = app.try_state::<RemoteState>() else {
        return Vec::new();
    };
    state
        .devices
        .devices()
        .into_iter()
        .map(|device| RemoteDevice {
            id: device.id,
            name: device.name,
            platform: device.platform,
            created_unix_ms: device.created_unix_ms,
            last_seen_unix_ms: device.last_seen_unix_ms,
            last_addr: device.last_addr,
        })
        .collect()
}

/// The device store, for the commands that pair and revoke.
pub fn device_store(app: &AppHandle) -> Option<Arc<DeviceStore>> {
    app.try_state::<RemoteState>()
        .map(|state| Arc::clone(&state.devices))
}

/// This instance's public key, for the pairing payload.
pub fn public_key(app: &AppHandle) -> Option<String> {
    app.try_state::<RemoteState>()
        .map(|state| state.statik.public_base64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{LlmProvider, LlmSettings};
    use std::path::PathBuf;

    const PLANTED_KEY: &str = "sk-ant-planted-000000000000000000";

    fn workspace_holding_a_key() -> Workspace {
        let mut ws = Workspace::default();
        let project =
            cide_core::workspace::open_project(&mut ws, vec![PathBuf::from("/tmp/p")], None)
                .expect("opens");
        let console = cide_core::workspace::console_tab(&ws, project).expect("exists");
        let pane = cide_core::workspace::tab(&ws, project, console)
            .expect("exists")
            .tree
            .focused;
        cide_core::workspace::bind_session(&mut ws, project, console, pane, SessionId::new())
            .expect("binds");

        ws.settings.llm = LlmSettings {
            providers: vec![LlmProvider::Catalog {
                id: "anthropic".into(),
                label: String::new(),
                enabled: true,
                api_key: PLANTED_KEY.into(),
            }],
            pools: vec![],
        };
        ws.settings.proxy.http = format!("http://user:{PLANTED_KEY}@proxy.corp:3128");
        ws
    }

    /// The rule this module exists for. If a projection ever grows a field that reaches into
    /// `Settings`, this is what says so.
    #[test]
    fn a_projection_carries_no_credential() {
        let ws = workspace_holding_a_key();
        let facts = SessionFacts::gather(HashMap::new(), None, &ws, None);

        let mut everything = serde_json::to_string(&projects(&ws)).expect("serialises");
        everything.push_str(&serde_json::to_string(&sessions(&ws, &facts, None)).expect("ser"));
        everything.push_str(&serde_json::to_string(&awaiting()).expect("serialises"));
        everything.push_str(&serde_json::to_string(&instance_info()).expect("serialises"));

        assert!(
            !everything.contains(PLANTED_KEY),
            "a credential reached the remote projection: {everything}"
        );
        // And the planted key really was in the workspace, so a test that stopped exercising
        // the interesting path would fail rather than pass quietly.
        assert!(
            serde_json::to_string(&ws)
                .expect("ser")
                .contains(PLANTED_KEY)
        );
    }

    #[test]
    fn a_session_with_no_hook_server_reads_as_spawning() {
        let ws = workspace_holding_a_key();
        let facts = SessionFacts::gather(HashMap::new(), None, &ws, None);
        let found = sessions(&ws, &facts, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].state, SessionState::Spawning);
        assert!(!found[0].awaiting);
        assert_eq!(found[0].run, None);
    }

    fn network(port: u16) -> RemoteSettings {
        RemoteSettings {
            enabled: true,
            bind: RemoteBind::Network,
            port,
            allow_non_private: false,
        }
    }

    const ANY: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

    /// The gesture this exists for: opening or closing a pairing window moves nothing about the
    /// socket, so the listener — and every phone on it — must be left where it is.
    #[test]
    fn a_listener_that_already_matches_is_left_alone() {
        assert!(same_socket(Some((ANY, 17643)), &network(0)));
        assert!(same_socket(Some((ANY, 17643)), &network(17643)));
    }

    #[test]
    fn a_listener_bound_somewhere_else_is_not_the_one_that_was_asked_for() {
        // Nothing running.
        assert!(!same_socket(None, &network(0)));
        // A port the user typed, and a listener on a different one. This is the case that must
        // not be short-circuited: the promise in a configured port is to the device that saved
        // it, so the socket really does have to move.
        assert!(!same_socket(Some((ANY, 17644)), &network(17643)));
        // A different address entirely.
        assert!(!same_socket(
            Some((IpAddr::V4(Ipv4Addr::LOCALHOST), 17643)),
            &network(0)
        ));
        // And settings that name an address cide refuses to bind are a refusal to report, never
        // a listener to keep: `allow_non_private` is off and the address is routable.
        let public = RemoteSettings {
            enabled: true,
            bind: RemoteBind::Address {
                address: "8.8.8.8".to_owned(),
            },
            port: 0,
            allow_non_private: false,
        };
        assert!(!same_socket(Some((ANY, 17643)), &public));
    }
}
