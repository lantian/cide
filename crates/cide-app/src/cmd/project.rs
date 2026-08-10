//! Project and tab commands.
//!
//! Thin by policy: each handler unwraps its arguments, calls one `cide-core` operation
//! through [`WorkspaceState::update`] — which validates and marks the file dirty — and
//! returns the new revision. The rules about what may be closed or reordered live in the
//! domain, so a second frontend, or a future headless mode, gets them for free.

use std::path::PathBuf;

use cide_core::CoreError;
use cide_core::workspace;
use cide_ipc::{Pane, PaneId, PaneKind, PaneRole, ProjectId, TabId, TabKind};
use tauri::{Manager, State};

use crate::workspace_state::WorkspaceState;

/// Returned by every mutation so the caller can order or discard a later snapshot.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutated {
    pub rev: u64,
}

#[tauri::command(rename_all = "camelCase")]
pub fn project_open(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    paths: Vec<String>,
    name: Option<String>,
) -> Result<ProjectId, CoreError> {
    let roots: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let id = state.update(|ws| workspace::open_project(ws, roots, name))?;

    // Started here rather than lazily at first spawn, because the lockfile has to exist
    // before any `claude` in this project looks for one. `open_project` also *activates* an
    // already-open path instead of opening it twice, so `ensure` is by design idempotent.
    if let Some(servers) = app.try_state::<crate::ide::IdeServers>() {
        let roots = state.with(|ws| {
            workspace::project(ws, id)
                .map(|p| p.roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        });
        servers.ensure(&app, id, roots);
    }
    Ok(id)
}

/// Close a project, its tabs and its windows.
///
/// `force` is the caller's statement that the user has been shown, and accepted, whatever
/// unsaved edits the project holds. Without it a dirty file tab anywhere in the project is
/// [`CoreError::UnsavedChanges`] — see `cide_core::workspace::close_project`.
#[tauri::command(rename_all = "camelCase")]
pub fn project_close(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    force: bool,
) -> Result<Mutated, CoreError> {
    // Read before the mutation: once the project is gone the diff tabs are gone with it, and
    // with them the only record of which agent turns are still blocked waiting on them.
    let blocked = app
        .try_state::<crate::ide::IdeServers>()
        .map(|_| crate::ide::pending_request_ids(&state, project))
        .unwrap_or_default();

    let out = state.update(|ws| {
        workspace::close_project(ws, project, force)?;
        Ok(Mutated { rev: ws.rev })
    })?;

    if let Some(servers) = app.try_state::<crate::ide::IdeServers>() {
        // Reject first, stop second. `stop` would cancel these anyway, but doing it here
        // records the accurate reason — the project closed — rather than reporting a
        // shutdown to an agent whose editor is still running.
        crate::ide::cancel_for_tabs(
            &servers,
            project,
            blocked,
            cide_ide_mcp::CancelReason::ProjectClosed,
        );
        servers.stop(project);
    }

    // Closing a project drops the window roles that showed parts of it. Those windows are
    // still on screen until something takes them down, and a detached one would sit there
    // blank with an unreachable child behind it.
    crate::cmd::window::reconcile(&app, &state)?;
    Ok(out)
}

/// Bring a project to the front of the window that holds it.
///
/// The header draws a tab per open project and its `onActivate` had nowhere to go: the
/// domain has had `activate_project` since projects were multi-window, and no command ever
/// exposed it. So with two projects open, clicking the other one did nothing — the same
/// dead-control failure the sidebar's search button had.
///
/// Idempotent, and cheap: activating the project that is already active still bumps `rev`
/// through `update`, which is what makes a second window follow along.
#[tauri::command(rename_all = "camelCase")]
pub fn project_activate(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<Mutated, CoreError> {
    state.update(|ws| {
        // Resolved first so an unknown id is an error rather than a silent no-op — the
        // frontend passes an id it read from the tree, so a miss means they have diverged.
        workspace::project(ws, project)?;
        workspace::activate_project(ws, project);
        Ok(Mutated { rev: ws.rev })
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn project_reorder(
    state: State<'_, WorkspaceState>,
    from: usize,
    to: usize,
) -> Result<Mutated, CoreError> {
    state.update(|ws| {
        workspace::reorder_project(ws, from, to)?;
        Ok(Mutated { rev: ws.rev })
    })
}

/// Open a closable full-screen Claude tab.
///
/// The second half of the product's headline feature: the pinned console is the project's
/// one conversation, and this is where a user starts others. Splitting it creates *new*
/// sessions rather than shells — the only behavioural difference between the two Claude
/// tabs, and it lives in `cmd::pane::default_intent`.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_new_claude(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    title: Option<String>,
) -> Result<TabId, CoreError> {
    state.update(|ws| {
        let name = workspace::project(ws, project)?.name.clone();
        let title = title.unwrap_or_else(|| "Claude".to_string());
        workspace::open_tab(
            ws,
            project,
            TabKind::ClaudeFull { title },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Claude,
                // Every pane in a ClaudeFull tab is closable; closing the last closes the
                // tab. Only the pinned console has a pane that cannot go.
                role: PaneRole::Auxiliary,
                session: None,
                title: format!("{name} : claude"),
            },
        )
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn tab_activate(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
) -> Result<Mutated, CoreError> {
    state.update(|ws| {
        workspace::activate_tab(ws, project, tab)?;
        Ok(Mutated { rev: ws.rev })
    })
}

/// Close a tab.
///
/// Two refusals come back from the domain, and both are enforcement rather than courtesy:
///
/// * [`CoreError::TabPinned`] for the project console. The frontend also declines to draw a
///   close button on it, but that is the courtesy — this is why no UI bug can lose it.
/// * [`CoreError::UnsavedChanges`] for a file tab with unsaved edits, unless `force`. The
///   frontend answers that by showing `CloseConfirm` naming the file, and calls again with
///   `force: true` if the user chooses to discard. A `×` on a tab is one click away from a
///   lost afternoon, and the only thing standing between the two is this refusal — the
///   dialog is not, because a dialog that fails to render fails open.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_close(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    force: bool,
) -> Result<Mutated, CoreError> {
    // Closing a diff tab **rejects** it, and this is a deliberate divergence worth naming.
    //
    // The protocol's `TAB_CLOSED` means accepted-as-proposed — verified against the real CLI,
    // which writes the model's version on receiving it — and that is what VS Code sends when
    // its diff tab is closed. cide does not, because the two gestures are not the same thing
    // here: the diff pane carries explicit Reject / Accept as proposed / Accept controls, so
    // a user who means to accept has a button that says so. A `×` on a tab reads as dismiss,
    // and resolving a dismissal as acceptance would write a file change on a gesture nobody
    // makes with that intent. Rejection is recoverable; a write is not.
    let dismissed = crate::ide::request_id_for_tab(&state, project, tab);

    let out = state.update(|ws| {
        workspace::close_tab(ws, project, tab, force)?;
        Ok(Mutated { rev: ws.rev })
    })?;

    if let Some(request_id) = dismissed
        && let Some(servers) = app.try_state::<crate::ide::IdeServers>()
    {
        crate::ide::resolve_or_cancel(&servers, project, &request_id, None);
    }

    // Same reason as `project_close`: a tab can own detached windows, and their roles have
    // just been pruned.
    crate::cmd::window::reconcile(&app, &state)?;
    Ok(out)
}
