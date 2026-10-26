use crate::workspace_state::WorkspaceState;
use cide_core::local_history::{HistoryCoordinator, HistoryStore, validate_relative};
use cide_ipc::{ProjectId, local_history::*};
use std::{path::Path, sync::Arc};
use tauri::State;

fn scope(
    state: &WorkspaceState,
    project: ProjectId,
    root: &HistoryRootId,
) -> Result<Arc<HistoryCoordinator>, String> {
    state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })?;
    crate::local_history::scoped(project, root)
}

fn path_in(store: &HistoryStore, path: &str) -> Result<String, String> {
    validate_relative(Path::new(path))?;
    let (_, relative) = store.eligibility.root.resolve(Path::new(path))?;
    Ok(relative)
}
fn restore_path_in(store: &HistoryStore, path: &str) -> Result<String, String> {
    let relative = path_in(store, path)?;
    let physical = store.eligibility.root.path.join(&relative);
    if let Some(owner) = crate::local_history::for_path(&physical) {
        if owner.status().root != store.eligibility.root.id {
            return Err("This file belongs to a deeper open history root".into());
        }
    } else {
        return Err("This history root is no longer open".into());
    }
    Ok(relative)
}
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub fn local_history_status(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<Vec<HistoryStatus>, String> {
    state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })?;
    Ok(crate::local_history::project_roots(project)
        .iter()
        .map(|c| c.status())
        .collect())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_paths(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    root: HistoryRootId,
    generation: u32,
    after: Option<String>,
    filter: String,
    deleted_only: bool,
) -> Result<HistoryPathPage, String> {
    let c = scope(&state, project, &root)?;
    if filter.len() > 1024 || after.as_ref().is_some_and(|s| s.len() > 8192) {
        return Err("History path query is too large".into());
    }
    blocking(move || {
        c.with_store(|s| {
            s.check_generation(generation)?;
            Ok(s.paths(after.as_deref(), &filter, deleted_only))
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_list(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    root: HistoryRootId,
    generation: u32,
    path: String,
    before: Option<u32>,
) -> Result<HistoryRevisionPage, String> {
    let c = scope(&state, project, &root)?;
    blocking(move || {
        c.with_store(|s| {
            s.check_generation(generation)?;
            s.list(&path_in(s, &path)?, before)
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_compare(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    root: HistoryRootId,
    generation: u32,
    path: String,
    target: LocalRevisionId,
    other: Option<LocalRevisionId>,
) -> Result<HistoryComparison, String> {
    let c = scope(&state, project, &root)?;
    blocking(move || {
        c.with_store(|s| {
            s.check_generation(generation)?;
            s.compare(&path_in(s, &path)?, &target, other.as_ref())
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_set_policy(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    root: HistoryRootId,
    generation: u32,
    policy: HistoryPolicy,
) -> Result<HistoryStatus, String> {
    let c = scope(&state, project, &root)?;
    blocking(move || {
        c.with_store(|s| {
            s.check_generation(generation)?;
            s.set_policy(policy)
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_clear(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    root: HistoryRootId,
    generation: u32,
    confirmed: bool,
) -> Result<HistoryStatus, String> {
    let c = scope(&state, project, &root)?;
    blocking(move || {
        c.with_store(|s| {
            s.check_generation(generation)?;
            s.clear(confirmed)
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_restore(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    root: HistoryRootId,
    token: HistoryCompareToken,
    confirmed: bool,
) -> Result<HistoryRestoreResult, String> {
    use tauri::Manager;
    if !confirmed {
        return Err("Confirm the path and revision before restoring".into());
    }
    let coordinator = scope(&state, project, &root)?;
    blocking(move || {
        let physical = coordinator.with_store(|s| {
            restore_path_in(s, &token.path)?;
            s.validate_restore(&token)
        })?;
        let state = app.state::<WorkspaceState>();
        let mut prepared =
            crate::local_history_restore::PreparedEditors::prepare(&app, &state, physical.clone())?;
        let result = coordinator.with_store(|s| {
            restore_path_in(s, &token.path)?;
            s.restore_with(&token, |write| prepared.commit(&state, write))
        })?;
        if result.restored {
            prepared.restored();
            let change = cide_ipc::FsChange {
                paths: vec![physical.clone()],
                truncated: false,
                git: false,
            };
            crate::emit::fs_changed(&app, project, &change);
            crate::files::FsEvents::files_changed(&app, project, &change.paths);
            if result.degraded.is_some() {
                coordinator.observe(vec![physical], false);
            }
        }
        Ok(result)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub fn local_history_open_view(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    root: Option<HistoryRootId>,
    path: Option<String>,
) -> Result<LocalHistoryView, String> {
    let view =
        crate::local_history::view_for(project, path.as_deref().map(Path::new), root.as_ref())?;
    state
        .update(|ws| cide_core::toolwindow::open_local_history(ws, project, view.clone()))
        .map_err(|e| e.to_string())?;
    if let Some(label) = state.with(|ws| {
        ws.windows.iter().find_map(|(label, role)| match role {
            cide_ipc::WindowRole::Shell { projects, .. } if projects.contains(&project) => {
                Some(label.clone())
            }
            _ => None,
        })
    }) {
        crate::windows::raise(&app, &label);
    }
    Ok(view)
}
#[tauri::command(rename_all = "camelCase")]
pub fn local_history_close_view(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    view: LocalHistoryView,
) -> Result<(), String> {
    state
        .update(|ws| cide_core::toolwindow::close_local_history(ws, project, &view))
        .map_err(|e| e.to_string())
}
