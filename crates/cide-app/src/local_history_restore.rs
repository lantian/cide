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
    let mut expected = BTreeSet::new();
    for project in ws.projects.values() {
        for tab in &project.tabs {
            let TabKind::File { path, dirty } = &tab.kind else {
                continue;
            };
            if crate::local_history::canonical_path(path).as_deref() != Some(target) {
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
