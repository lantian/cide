//! Editor preparation and all-exit release. File mutation remains under the core authority.
use crate::{local_history::EDITOR_GATES, workspace_state::WorkspaceState};
use cide_core::local_history::{EditorIdentity, RestorePermit};
use cide_ipc::{PaneKind, TabKind, WindowRole, Workspace, local_history::*};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

fn editors(ws: &Workspace, target: &Path) -> Result<(BTreeSet<String>, String), String> {
    editors_with(ws, target, crate::local_history::canonical_path)
}
fn editors_with(
    ws: &Workspace,
    target: &Path,
    resolve: impl Fn(&Path) -> Option<PathBuf>,
) -> Result<(BTreeSet<String>, String), String> {
    let mut expected = BTreeSet::new();
    for project in ws.projects.values() {
        for tab in &project.tabs {
            let TabKind::File { path, dirty } = &tab.kind else {
                continue;
            };
            if resolve(path).as_deref() != Some(target) {
                continue;
            }
            if *dirty {
                return Err(
                    "An editor has unsaved changes. Save or discard them before restoring.".into(),
                );
            }
            expected.extend(
                tab.tree
                    .panes
                    .values()
                    .filter(|p| p.kind == PaneKind::Editor)
                    .map(|p| p.id.to_string()),
            );
            for role in ws.windows.values() {
                if let WindowRole::DetachedPane {
                    project: owner,
                    tab: home,
                    pane,
                } = role
                {
                    if *owner == project.id && *home == tab.id {
                        expected.insert(pane.to_string());
                    }
                }
            }
        }
    }
    Ok((
        expected,
        serde_json::to_string(&ws.windows).map_err(|e| e.to_string())?,
    ))
}
fn registration(identity: &EditorIdentity, path: &Path) -> HistoryEditorRegistration {
    HistoryEditorRegistration {
        window: identity.window.clone(),
        editor: identity.editor.clone(),
        mount: identity.mount.clone(),
        canonical_path: path.to_string_lossy().into_owned(),
    }
}

pub(crate) struct PreparedEditors {
    permit: Option<RestorePermit>,
    app: tauri::AppHandle,
    path: PathBuf,
    expected: (BTreeSet<String>, String),
    event: HistoryEditorGate,
}
impl PreparedEditors {
    pub fn prepare(
        app: &tauri::AppHandle,
        state: &WorkspaceState,
        path: PathBuf,
    ) -> Result<Self, String> {
        let expected = state.with(|ws| editors(ws, &path))?;
        let permit = EDITOR_GATES.prepare(path.clone(), &expected.0)?;
        let event = HistoryEditorGate {
            request: permit.request.clone(),
            canonical_path: path.to_string_lossy().into_owned(),
            editors: permit
                .editors
                .iter()
                .map(|e| registration(e, &path))
                .collect(),
            phase: HistoryGatePhase::Prepare,
            restored: false,
        };
        let prepared = Self {
            permit: Some(permit),
            app: app.clone(),
            path,
            expected,
            event,
        };
        crate::emit::local_history_editor_gate(app, &prepared.event);
        prepared
            .permit
            .as_ref()
            .unwrap()
            .wait(Duration::from_secs(3))?;
        Ok(prepared)
    }
    pub fn commit<T>(
        &self,
        state: &WorkspaceState,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        // A structural window/editor change cannot slip between the final check and mutation.
        state.with(|ws| {
            if editors(ws, &self.path)? != self.expected {
                return Err("Editors or windows changed; refresh the comparison and retry".into());
            }
            self.permit.as_ref().unwrap().commit(operation)
        })
    }
    pub fn restored(&mut self) {
        self.event.restored = true;
    }
}
impl Drop for PreparedEditors {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.event.phase = HistoryGatePhase::Release;
        crate::emit::local_history_editor_gate(&self.app, &self.event);
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn local_history_editor_register(
    window: tauri::WebviewWindow,
    editor: String,
    path: PathBuf,
) -> Result<HistoryEditorRegistration, String> {
    uuid::Uuid::parse_str(&editor).map_err(|_| "Invalid editor identity")?;
    let physical = crate::local_history::canonical_path(&path)
        .ok_or("This editor is outside open history roots")?;
    let identity = EDITOR_GATES.register(window.label().to_owned(), editor, physical.clone());
    Ok(registration(&identity, &physical))
}
#[tauri::command(rename_all = "camelCase")]
pub fn local_history_editor_unregister(
    window: tauri::WebviewWindow,
    editor: String,
    mount: String,
) {
    EDITOR_GATES.unregister(&EditorIdentity {
        window: window.label().to_owned(),
        editor,
        mount,
    });
}
#[tauri::command(rename_all = "camelCase")]
pub fn local_history_editor_acknowledge(
    window: tauri::WebviewWindow,
    editor: String,
    mount: String,
    request: String,
    dirty: bool,
) -> Result<(), String> {
    EDITOR_GATES.acknowledge(
        &EditorIdentity {
            window: window.label().to_owned(),
            editor,
            mount,
        },
        &request,
        dirty,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_editor_expectations_include_detached_editors_and_dirty_tabs() {
        let mut ws = cide_core::workspace::demo_workspace();
        let project = *ws.projects.keys().next().unwrap();
        let tab = ws.projects[&project]
            .tabs
            .iter()
            .find(|t| matches!(t.kind, TabKind::File { .. }))
            .unwrap()
            .id;
        let path = match &ws.projects[&project]
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .unwrap()
            .kind
        {
            TabKind::File { path, .. } => path.clone(),
            _ => unreachable!(),
        };
        assert!(
            editors_with(&ws, &path, |p| Some(p.into()))
                .unwrap_err()
                .contains("unsaved")
        );
        let file = ws
            .projects
            .get_mut(&project)
            .unwrap()
            .tabs
            .iter_mut()
            .find(|t| t.id == tab)
            .unwrap();
        if let TabKind::File { dirty, .. } = &mut file.kind {
            *dirty = false;
        }
        let original = file.tree.panes.values().next().unwrap().clone();
        let mut detached = original.clone();
        detached.id = cide_ipc::PaneId::new();
        let id = detached.id;
        ws.projects
            .get_mut(&project)
            .unwrap()
            .detached
            .insert(id, detached);
        ws.windows.insert(
            cide_ipc::WindowLabel("detached".into()),
            WindowRole::DetachedPane {
                project,
                tab,
                pane: id,
            },
        );
        let before = editors_with(&ws, &path, |p| Some(p.into())).unwrap();
        assert!(before.0.contains(&original.id.to_string()));
        assert!(before.0.contains(&id.to_string()));
        ws.windows
            .shift_remove(&cide_ipc::WindowLabel("detached".into()));
        assert_ne!(
            editors_with(&ws, &path, |p| Some(p.into())).unwrap(),
            before
        );
    }
}
