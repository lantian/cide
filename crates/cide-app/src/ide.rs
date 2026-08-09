//! One IDE MCP server per project, and the bridge from its events into the workspace.
//!
//! # Why the app owns this and `cide-ide-mcp` does not
//!
//! The server crate knows about sockets, tools and a broker. It deliberately knows nothing
//! about tabs, panes or projects — so something has to translate "a `claude` somewhere asked
//! to show a diff" into "open a diff tab in project P", and that translation is the only
//! part that needs both vocabularies. It lives here.
//!
//! # The invariant that shapes every function below
//!
//! `openDiff` blocks an agent turn. The CLI sends it and waits; nothing else happens in that
//! conversation until an answer comes back. [`cide_ide_mcp::DiffBroker`] guarantees that
//! every way the *user* can disappear resolves the request — but it cannot know about the
//! ways the *app* can fail to show it in the first place. A project that closed between the
//! request arriving and the tab opening, a workspace mutation that is rejected by its own
//! validator, a pane id that collides: each of those is a path where the diff tab never
//! appears, and if any of them simply returns, the `claude` that asked waits for ever with
//! nothing on screen to explain it.
//!
//! So every early return in [`pump`] cancels first. The rule is that the request is
//! answered on every path out, and rejection is the safe answer because it leaves the file
//! alone.
//!
//! # Why a dedicated runtime
//!
//! The server is async; the command handlers and the signal-driven shutdown are not. Rather
//! than make the whole app async for one subsystem, this module owns a runtime and blocks on
//! it at the few points that need to. Shutdown blocks on the main or signal thread, which is
//! acceptable for the same reason [`crate::lifecycle::stop_children`] is: the process is on
//! its way out and the alternative is leaving a `claude` mid-turn.

use std::path::PathBuf;

use cide_ide_mcp::{
    CancelReason, DiffBroker, DiffOutcome, DiffRequest, IdeServer, ServerEvent, sweep_stale,
};
use cide_ipc::{DiffOrigin, DiffSpec, Pane, PaneId, PaneKind, PaneRole, ProjectId, TabKind};
use dashmap::DashMap;
use tauri::{AppHandle, Manager};
use tokio::runtime::Runtime;

use crate::workspace_state::WorkspaceState;

/// A running server, and the handles the rest of the app needs from it.
struct Entry {
    server: IdeServer,
    port: u16,
    broker: DiffBroker,
}

/// Every project's IDE server.
pub struct IdeServers {
    rt: Runtime,
    servers: DashMap<ProjectId, Entry>,
}

impl IdeServers {
    /// Build the runtime and clear our own abandoned lockfiles.
    ///
    /// The sweep runs once, at boot, before any server publishes. A lockfile left by a
    /// previous run that was killed points a `claude` at a port nothing is listening on,
    /// which the CLI reports as a broken IDE rather than as no IDE — worse than never having
    /// advertised. [`sweep_stale`] only ever removes files whose `ideName` is ours, so a
    /// user's VS Code or JetBrains lockfile in the same directory is never at risk.
    pub fn new() -> std::io::Result<Self> {
        let swept = sweep_stale();
        if swept > 0 {
            tracing::info!(swept, "removed stale cide lockfiles from a previous run");
        }

        Ok(Self {
            rt: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("cide-ide")
                .build()?,
            servers: DashMap::new(),
        })
    }

    /// Start a server for a project, or return the port of the one already running.
    ///
    /// Failure is logged and returns `None` rather than propagating: not being able to bind a
    /// loopback port costs IDE integration, and the terminal must keep working without it.
    /// That is the same rule the protocol module states — degrade the diff view, never break
    /// the terminal.
    pub fn ensure(&self, app: &AppHandle, project: ProjectId, roots: Vec<PathBuf>) -> Option<u16> {
        if let Some(entry) = self.servers.get(&project) {
            return Some(entry.port);
        }

        let server = match self.rt.block_on(IdeServer::start(roots)) {
            Ok(s) => s,
            Err(error) => {
                tracing::error!(%error, %project, "could not start the IDE server; panes will run without IDE integration");
                return None;
            }
        };

        let port = server.port();
        let broker = server.broker();
        let events = server.events();

        // Taken once, here, because `events()` hands the receiver over: a second caller would
        // silently take the stream from this pump and diffs would stop reaching the UI.
        self.rt
            .spawn(pump(app.clone(), project, events, broker.clone()));

        self.servers.insert(
            project,
            Entry {
                server,
                port,
                broker,
            },
        );
        tracing::info!(%project, port, "IDE server listening");
        Some(port)
    }

    /// The port to put in a child's `CLAUDE_CODE_SSE_PORT`.
    pub fn port(&self, project: ProjectId) -> Option<u16> {
        self.servers.get(&project).map(|e| e.port)
    }

    /// The broker, for answering a diff the user just resolved.
    pub fn broker(&self, project: ProjectId) -> Option<DiffBroker> {
        self.servers.get(&project).map(|e| e.broker.clone())
    }

    /// Tell the server which pane a connected `claude` belongs to.
    pub fn bind_pane(&self, project: ProjectId, pid: u32, pane: PaneId) {
        if let Some(entry) = self.servers.get(&project) {
            entry.server.bind_pane(pid, pane.to_string());
        }
    }

    /// Stop a project's server, resolving anything it still owes.
    pub fn stop(&self, project: ProjectId) {
        let Some((_, entry)) = self.servers.remove(&project) else {
            return;
        };
        // `shutdown` rejects pending diffs *before* closing sockets, so a blocked `claude`
        // receives a plain refusal over a live connection instead of a transport error.
        self.rt.block_on(entry.server.shutdown());
        tracing::info!(%project, "IDE server stopped");
    }

    /// Stop every server. The quit path.
    pub fn stop_all(&self) {
        let projects: Vec<ProjectId> = self.servers.iter().map(|e| *e.key()).collect();
        for project in projects {
            self.stop(project);
        }
    }
}

/// Translate one project's server events into workspace mutations.
///
/// Every arm that cannot complete its mutation cancels the request it was handling. See the
/// module docs: an unanswered `openDiff` is a hung agent turn with no visible cause.
async fn pump(
    app: AppHandle,
    project: ProjectId,
    mut events: tokio::sync::mpsc::Receiver<ServerEvent>,
    broker: DiffBroker,
) {
    while let Some(event) = events.recv().await {
        match event {
            ServerEvent::DiffRequested(request) => {
                open_diff_tab(&app, project, &broker, request);
            }
            ServerEvent::DiffWithdrawn { tab_name } => {
                // The CLI closed the diff itself — its `beforeExit` does this. The broker has
                // already resolved the request; this only takes the tab off the screen.
                close_diff_tab_by_name(&app, project, &tab_name);
            }
            ServerEvent::Connected { pid, .. } => {
                tracing::debug!(%project, pid, "a claude connected to the IDE server");
            }
            ServerEvent::Disconnected { .. } => {}
            ServerEvent::OpenFile {
                path,
                start_line,
                end_line,
            } => {
                // Line numbers arrive already converted to 1-based by `tools::open_file`.
                tracing::debug!(%project, ?path, start_line, end_line, "openFile requested");
            }
        }
    }
}

/// Open the diff tab for a blocked `openDiff`, or reject it.
fn open_diff_tab(app: &AppHandle, project: ProjectId, broker: &DiffBroker, request: DiffRequest) {
    let Some(state) = app.try_state::<WorkspaceState>() else {
        // The app is being torn down. Rejecting is the only honest answer available.
        broker.cancel(&request.id, CancelReason::Shutdown);
        return;
    };

    let new_path = PathBuf::from(&request.params.new_file_path);
    let title = new_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.params.tab_name.clone());

    let spec = DiffSpec {
        title: title.clone(),
        old_path: PathBuf::from(&request.params.old_file_path),
        new_path,
        origin: DiffOrigin::ClaudeMcp {
            request_id: request.id.clone(),
        },
    };

    let pane = Pane {
        id: PaneId::new(),
        kind: PaneKind::Diff,
        role: PaneRole::Auxiliary,
        session: None,
        title,
    };

    let opened = state.update(|ws| {
        cide_core::workspace::open_tab(ws, project, TabKind::Diff { spec }, pane)?;
        Ok(())
    });

    if let Err(error) = opened {
        // The project closed while the request was in flight, or the mutation failed its own
        // validator. Either way no tab will appear, so the turn must not be left waiting.
        tracing::warn!(%error, %project, "could not open a diff tab; rejecting the request");
        broker.cancel(&request.id, CancelReason::ProjectClosed);
    }
}

/// Take a diff tab off screen after the CLI withdrew it.
fn close_diff_tab_by_name(app: &AppHandle, project: ProjectId, tab_name: &str) {
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return;
    };

    // The tab is found by the request id the CLI is closing, not by title: two diffs can
    // share a filename, and closing the wrong one would strand the other's agent turn.
    let target: Option<cide_ipc::TabId> = state.with(|ws| {
        let p = cide_core::workspace::project(ws, project).ok()?;
        p.tabs.iter().find_map(|tab| match &tab.kind {
            TabKind::Diff { spec } => match &spec.origin {
                DiffOrigin::ClaudeMcp { request_id } if request_id == tab_name => Some(tab.id),
                _ => None,
            },
            _ => None,
        })
    });

    if let Some(tab) = target
        && let Err(error) = state.update(|ws| {
            cide_core::workspace::close_tab(ws, project, tab)?;
            Ok(())
        })
    {
        tracing::warn!(%error, "could not close a withdrawn diff tab");
    }
}

/// Answer a diff the user resolved, and cancel any that a closing tab abandons.
///
/// Called from the tab-close path as well as from the explicit answer command, because
/// closing a diff tab *is* an answer: the protocol's `TAB_CLOSED` means accepted-as-proposed,
/// but a tab going away for any other reason must reject rather than accept, or a window
/// closing would write a change nobody approved.
pub fn resolve_or_cancel(
    servers: &IdeServers,
    project: ProjectId,
    request_id: &str,
    outcome: Option<DiffOutcome>,
) {
    let Some(broker) = servers.broker(project) else {
        return;
    };
    match outcome {
        Some(outcome) => {
            broker.resolve(request_id, outcome);
        }
        None => {
            broker.cancel(request_id, CancelReason::PaneClosed);
        }
    }
}

/// Cancel every `openDiff` that a set of tabs was showing.
///
/// Used by the close paths, which know which tabs are about to disappear but not which of
/// them are blocking an agent.
pub fn cancel_for_tabs(
    servers: &IdeServers,
    project: ProjectId,
    request_ids: impl IntoIterator<Item = String>,
    reason: CancelReason,
) {
    let Some(broker) = servers.broker(project) else {
        return;
    };
    for id in request_ids {
        broker.cancel(&id, reason);
    }
}

/// The `ClaudeMcp` request id of one tab, if it is a diff blocking an agent.
pub fn request_id_for_tab(
    state: &WorkspaceState,
    project: ProjectId,
    tab: cide_ipc::TabId,
) -> Option<String> {
    state.with(|ws| {
        let p = cide_core::workspace::project(ws, project).ok()?;
        p.tabs
            .iter()
            .find(|t| t.id == tab)
            .and_then(|t| match &t.kind {
                TabKind::Diff { spec } => match &spec.origin {
                    DiffOrigin::ClaudeMcp { request_id } => Some(request_id.clone()),
                    _ => None,
                },
                _ => None,
            })
    })
}

/// The `ClaudeMcp` request ids of every diff tab in a project.
///
/// Read before a destructive mutation so the ids survive the tabs.
pub fn pending_request_ids(state: &WorkspaceState, project: ProjectId) -> Vec<String> {
    state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .map(|p| {
                p.tabs
                    .iter()
                    .filter_map(|tab| match &tab.kind {
                        TabKind::Diff { spec } => match &spec.origin {
                            DiffOrigin::ClaudeMcp { request_id } => Some(request_id.clone()),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}
