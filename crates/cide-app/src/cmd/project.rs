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
use tauri::State;

use crate::workspace_state::WorkspaceState;

/// Returned by every mutation so the caller can order or discard a later snapshot.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutated {
    pub rev: u64,
}

#[tauri::command(rename_all = "camelCase")]
pub fn project_open(
    state: State<'_, WorkspaceState>,
    paths: Vec<String>,
    name: Option<String>,
) -> Result<ProjectId, CoreError> {
    let roots: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    state.update(|ws| workspace::open_project(ws, roots, name))
}

#[tauri::command(rename_all = "camelCase")]
pub fn project_close(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<Mutated, CoreError> {
    let out = state.update(|ws| {
        workspace::close_project(ws, project)?;
        Ok(Mutated { rev: ws.rev })
    })?;
    // Closing a project drops the window roles that showed parts of it. Those windows are
    // still on screen until something takes them down, and a detached one would sit there
    // blank with an unreachable child behind it.
    crate::cmd::window::reconcile(&app, &state)?;
    Ok(out)
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
/// Returns [`CoreError::TabPinned`] for the project console. The frontend also declines to
/// draw a close button on it, but that is a courtesy — this is the enforcement, so no UI
/// bug can lose a project's console.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_close(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
) -> Result<Mutated, CoreError> {
    let out = state.update(|ws| {
        workspace::close_tab(ws, project, tab)?;
        Ok(Mutated { rev: ws.rev })
    })?;
    // Same reason as `project_close`: a tab can own detached windows, and their roles have
    // just been pruned.
    crate::cmd::window::reconcile(&app, &state)?;
    Ok(out)
}
