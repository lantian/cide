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

use std::collections::HashSet;
use std::path::PathBuf;

use cide_ide_mcp::{
    CancelReason, DiffBroker, DiffOutcome, DiffRequest, IdeServer, ServerEvent, sweep_stale,
};
use cide_ipc::{
    DiffOrigin, DiffSpec, Pane, PaneId, PaneKind, PaneRole, ProjectId, TabKind, Workspace,
};
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

    /// Start a server for every project a restored workspace already holds.
    ///
    /// [`ensure`](Self::ensure) is otherwise reached only from `project_open`, and a launch
    /// that *restores* never calls it: the projects come straight out of `workspace.json`.
    /// Without this, every project on every launch after the first had no lockfile and no
    /// `CLAUDE_CODE_SSE_PORT` for its panes — no `openDiff`, no selection, no `@`-mentions.
    ///
    /// Failures are already swallowed by `ensure`, one project at a time, which is what we
    /// want here too: one project that cannot bind a port must not cost the others theirs.
    pub fn ensure_all(&self, app: &AppHandle, ws: &Workspace) {
        for (project, roots) in servable_projects(ws) {
            self.ensure(app, project, roots);
        }
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

    /// Tell every connected `claude` in a project where the editor selection is.
    pub fn selection_changed(&self, project: ProjectId, payload: cide_ide_mcp::SelectionChanged) {
        if let Some(entry) = self.servers.get(&project) {
            entry.server.selection_changed_all(payload);
        }
    }

    /// Put a file reference into one pane's `claude` prompt.
    ///
    /// Addressed to a single pane, unlike `selection_changed`. A mention is something the
    /// user aimed at a conversation — it lands in that prompt as text they are about to send
    /// — so broadcasting it would type into every Claude in the project at once.
    pub fn at_mentioned(
        &self,
        project: ProjectId,
        pane: PaneId,
        payload: cide_ide_mcp::AtMentioned,
    ) {
        if let Some(entry) = self.servers.get(&project) {
            entry.server.at_mentioned(&pane.to_string(), payload);
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

/// Every project that wants a server, with the roots its lockfile should advertise.
///
/// A separate function from [`IdeServers::ensure_all`] because this is the half that can be
/// tested: `ensure` binds a loopback port and writes into the user's real `~/.claude/ide`,
/// which is shared with whatever editors they are actually running and is no place for a
/// test suite to leave files — the same reason `cide-ide-mcp` tests against `attach` rather
/// than `start`.
///
/// Every root, not just `roots[0]`: the CLI matches its cwd against the lockfile's
/// `workspaceFolders`, so a `claude` started in a project's second root would otherwise not
/// see this server as its own.
fn servable_projects(ws: &Workspace) -> Vec<(ProjectId, Vec<PathBuf>)> {
    // What each restored window is *showing* comes first, and header order fills in behind
    // it. This ordering exists only for the cap below, and it is what makes the cap safe: in
    // header order a workspace whose active project sits past index 32 would launch with 32
    // servers for projects that are not on screen and none for the one that is — silently,
    // since a missing server looks exactly like Claude having no IDE. Ordering by what is
    // visible means the cap can only ever cost a project the user cannot currently see.
    //
    // Only `active` and the detached windows' own project, not every id in a stacked
    // window's `projects` list: in `Stacked` mode that list is every project in the
    // workspace, so honouring it would restore header order and hand the cap back its bug.
    let mut order: Vec<ProjectId> = Vec::with_capacity(ws.projects.len());
    let mut seen: HashSet<ProjectId> = HashSet::with_capacity(ws.projects.len());
    let mut want = |id: ProjectId, order: &mut Vec<ProjectId>| {
        // A window naming a project the workspace does not hold fails validation, so this is
        // belt-and-braces — but `servable_projects` runs on a file we have just read from
        // disk, and a `roots` lookup that misses would otherwise publish an empty lockfile.
        if ws.projects.contains_key(&id) && seen.insert(id) {
            order.push(id);
        }
    };
    for role in ws.windows.values() {
        match role {
            cide_ipc::WindowRole::Shell { active, .. } => {
                if let Some(id) = active {
                    want(*id, &mut order);
                }
            }
            cide_ipc::WindowRole::DetachedPane { project, .. }
            | cide_ipc::WindowRole::DetachedTab { project, .. } => want(*project, &mut order),
        }
    }
    for id in ws.projects.keys() {
        want(*id, &mut order);
    }

    let mut targets: Vec<(ProjectId, Vec<PathBuf>)> = order
        .into_iter()
        .map(|id| {
            let roots = ws.projects[&id]
                .roots
                .iter()
                .map(|r| r.path.clone())
                .collect();
            (id, roots)
        })
        .collect();

    // The same reasoning as `restore_windows`' window cap, and the same incident behind it: a
    // workspace here once accumulated 242 projects, and a launch that faithfully obeys such a
    // file binds 242 loopback ports and drops 242 lockfiles into `~/.claude/ide` — a
    // directory shared with whatever editors the user actually runs, which the CLI reads in
    // full every time it looks for an IDE. Serving the first few is a degraded launch; that
    // is a broken `claude` for everything else on the machine.
    //
    // The projects past the cap are not stranded: `project_open` on an already-open path
    // activates it and calls `ensure`, so re-opening the folder gives it a server.
    const MAX_RESTORED_SERVERS: usize = 32;
    if targets.len() > MAX_RESTORED_SERVERS {
        tracing::error!(
            projects = targets.len(),
            cap = MAX_RESTORED_SERVERS,
            "too many restored projects to serve; the rest get an IDE server when re-opened"
        );
        targets.truncate(MAX_RESTORED_SERVERS);
    }
    targets
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
            // `force`: the tab being withdrawn is a `Diff`, which has no dirty flag and
            // therefore no unsaved edits to lose — and the CLI has already stopped waiting
            // on it, so refusing would leave a tab on screen that answers nothing.
            cide_core::workspace::close_tab(ws, project, tab, true)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every restored project appears in the list `ensure_all` walks.
    ///
    /// **What this does not cover, deliberately and unhappily:** that `setup` calls
    /// `ensure_all` at all. Deleting that call leaves this file's four tests green — the
    /// wiring is one line in `crate::run`'s `setup` closure, `ensure` binds a port and
    /// writes into the user's real `~/.claude/ide`, and `tauri`'s mock app is behind a
    /// feature this build does not enable, so there is no seam between the two that a test
    /// can hold. This is the same limitation `hooks::live_in` documents, handled the same
    /// way: everything that can be *wrong* was pushed down into `servable_projects`, which
    /// is what these tests hold, and the call site above it is a single unconditional line
    /// with nothing to get subtly wrong.
    ///
    /// Servers used to start only from `project_open`, so a launch that loaded
    /// `workspace.json` gave every project panes with no `CLAUDE_CODE_SSE_PORT`. The
    /// assertion is on the *set* of projects rather than on a count: serving only the active
    /// one is the shape this bug would most plausibly grow back in.
    #[test]
    fn every_restored_project_is_served() {
        let mut ws = Workspace::default();
        let atlas =
            cide_core::workspace::open_project(&mut ws, vec!["/home/dev/atlas".into()], None)
                .expect("a rooted project opens");
        let beacon =
            cide_core::workspace::open_project(&mut ws, vec!["/home/dev/beacon".into()], None)
                .expect("a rooted project opens");

        let served: Vec<ProjectId> = servable_projects(&ws)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(served, vec![atlas, beacon]);
    }

    /// The lockfile advertises the folders the CLI matches its cwd against, so a project's
    /// second root has to be among them — a `claude` started in it would otherwise read this
    /// server's lockfile as belonging to some other workspace.
    #[test]
    fn a_multi_root_project_advertises_all_of_its_roots() {
        let mut ws = Workspace::default();
        let id = cide_core::workspace::open_project(&mut ws, vec!["/home/dev/atlas".into()], None)
            .expect("a rooted project opens");
        cide_core::workspace::add_root(&mut ws, id, "/home/dev/atlas-docs".into())
            .expect("a second root is accepted");

        let roots = servable_projects(&ws)
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, roots)| roots)
            .expect("the project is served");
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/home/dev/atlas"),
                PathBuf::from("/home/dev/atlas-docs"),
            ]
        );
    }

    /// A first launch restores no projects, and asking for zero servers is not a failure.
    #[test]
    fn an_empty_workspace_asks_for_nothing() {
        assert!(servable_projects(&Workspace::default()).is_empty());
    }

    /// A pathological file is capped rather than obeyed — one lockfile per project lands in
    /// a directory every `claude` on the machine reads.
    #[test]
    fn a_workspace_full_of_projects_is_capped() {
        let mut ws = Workspace::default();
        for i in 0..40 {
            cide_core::workspace::open_project(
                &mut ws,
                vec![format!("/home/dev/p{i}").into()],
                None,
            )
            .expect("a rooted project opens");
        }

        let served = servable_projects(&ws);
        assert_eq!(served.len(), 32);
        // The ones that are served are the header's leftmost, not an arbitrary subset: the
        // map's insertion order is the tab order the user sees.
        assert_eq!(served[0].1, vec![PathBuf::from("/home/dev/p0")]);
    }

    /// The cap must never take the server away from the project that is on screen.
    ///
    /// Straight header order does exactly that: a workspace of 40 whose active project is
    /// the last one would launch having bound 32 ports for projects behind the tab strip and
    /// none for the one in front of the user — and a project with no server is
    /// indistinguishable, from the pane, from Claude simply having no IDE.
    #[test]
    fn the_cap_never_drops_the_project_a_window_is_showing() {
        let mut ws = Workspace::default();
        let ids: Vec<ProjectId> = (0..40)
            .map(|i| {
                cide_core::workspace::open_project(
                    &mut ws,
                    vec![format!("/home/dev/p{i}").into()],
                    None,
                )
                .expect("a rooted project opens")
            })
            .collect();
        // Through the real gesture: `rebuild_windows` leaves `active` on the first project
        // opened, and clicking the fortieth header tab is what moves it.
        let on_screen = ids[39];
        cide_core::workspace::activate_project(&mut ws, on_screen);

        let served = servable_projects(&ws);
        assert_eq!(served.len(), 32);
        assert_eq!(
            served[0].0, on_screen,
            "the visible project is served first"
        );
        // And the cap still spends the rest of its budget on the header's leftmost, rather
        // than on whatever the window list happened to mention.
        assert_eq!(served[1].1, vec![PathBuf::from("/home/dev/p0")]);
    }

    /// A detached window has no tab strip and no `active` — its project is what it shows.
    #[test]
    fn a_detached_window_counts_as_showing_its_project() {
        let mut ws = Workspace::default();
        let ids: Vec<ProjectId> = (0..40)
            .map(|i| {
                cide_core::workspace::open_project(
                    &mut ws,
                    vec![format!("/home/dev/p{i}").into()],
                    None,
                )
                .expect("a rooted project opens")
            })
            .collect();
        ws.windows.insert(
            cide_ipc::WindowLabel("pane:test".into()),
            cide_ipc::WindowRole::DetachedPane {
                project: ids[38],
                tab: cide_ipc::TabId::new(),
                pane: PaneId::new(),
            },
        );

        let served: Vec<ProjectId> = servable_projects(&ws).into_iter().map(|(p, _)| p).collect();
        assert!(served.contains(&ids[38]));
    }
}
