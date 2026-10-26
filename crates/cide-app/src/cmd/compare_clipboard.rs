//! Clipboard comparisons and the transient document shared by their editable file views.
//! No clipboard text or CodeMirror history is written into workspace.json.

use crate::workspace_state::WorkspaceState;
use cide_core::{CoreError, Result, workspace};
use cide_ipc::shared_document::SharedDocumentSnapshot;
use cide_ipc::{PaneKind, ProjectId, TabId, TabKind, Workspace};
use parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
use tauri::State;

#[derive(Default)]
pub struct Comparisons {
    workspace_rev: Mutex<Option<u64>>,
    clips: Mutex<HashMap<String, String>>,
    documents: Mutex<HashMap<PathBuf, Document>>,
}

struct Document {
    snapshot: SharedDocumentSnapshot,
    tabs: HashSet<TabId>,
}

fn file_path(kind: &TabKind) -> Option<&PathBuf> {
    match kind {
        TabKind::File { path, .. } | TabKind::ClipboardDiff { path, .. } => Some(path),
        _ => None,
    }
}

impl Comparisons {
    /// A workspace mutation is also the lifetime boundary. Renames move the document by
    /// tab identity; closing the last view releases it. Clipboard snapshots are independent.
    pub fn reconcile(&self, ws: &Workspace) {
        let mut revision = self.workspace_rev.lock();
        if *revision == Some(ws.rev) {
            return;
        }
        *revision = Some(ws.rev);
        let tabs: Vec<_> = ws.projects.values().flat_map(|p| &p.tabs).collect();
        let clips: HashSet<_> = tabs
            .iter()
            .filter_map(|t| match &t.kind {
                TabKind::ClipboardDiff { snapshot_id, .. } => Some(snapshot_id.as_str()),
                _ => None,
            })
            .collect();
        self.clips
            .lock()
            .retain(|id, _| clips.contains(id.as_str()));
        let mut docs = self.documents.lock();
        let previous = std::mem::take(&mut *docs);
        for (path, mut doc) in previous {
            let next = tabs.iter().find_map(|t| {
                let candidate = file_path(&t.kind)?;
                (candidate == &path || doc.tabs.contains(&t.id)).then(|| candidate.clone())
            });
            if let Some(path) = next {
                doc.tabs = tabs
                    .iter()
                    .filter(|t| file_path(&t.kind) == Some(&path))
                    .map(|t| t.id)
                    .collect();
                docs.insert(path, doc);
            }
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_clipboard_diff(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    comparisons: State<'_, Comparisons>,
    project: ProjectId,
    path: String,
    text: String,
) -> Result<TabId> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(CoreError::Invariant(
            "a clipboard comparison needs an absolute file path".into(),
        ));
    }
    // Register before broadcasting the new tab, so its first mount can fetch the snapshot.
    let snapshot_id = uuid::Uuid::new_v4().to_string();
    let result = state.update(|ws| {
        let tab = open_comparison(ws, project, path, snapshot_id.clone())?;
        comparisons.clips.lock().insert(snapshot_id.clone(), text);
        Ok(tab)
    });
    if result.is_err() {
        comparisons.clips.lock().remove(&snapshot_id);
    }
    if let Ok(tab) = result
        && let Some(label) = workspace::detached_tab_window(&state.snapshot(), tab)
    {
        crate::windows::raise(&app, &label);
    }
    result
}

fn open_comparison(
    ws: &mut Workspace,
    project: ProjectId,
    path: PathBuf,
    snapshot_id: String,
) -> Result<TabId> {
    let existing = workspace::project(ws, project)?.tabs.iter().find_map(|t| {
        matches!(&t.kind, TabKind::ClipboardDiff { path: p, .. } if p == &path).then_some(t.id)
    });
    if let Some(id) = existing {
        if let TabKind::ClipboardDiff {
            snapshot_id: slot, ..
        } = &mut workspace::tab_mut(ws, project, id)?.kind
        {
            *slot = snapshot_id.clone();
        }
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }
    let dirty = workspace::project(ws, project)?
        .tabs
        .iter()
        .any(|t| matches!(&t.kind, TabKind::File { path: p, dirty: true } if p == &path));
    let kind = TabKind::ClipboardDiff {
        path: path.clone(),
        snapshot_id: snapshot_id.clone(),
        dirty,
    };
    let mut pane = super::file::diff_pane(kind.title());
    pane.kind = PaneKind::Editor;
    workspace::open_tab(ws, project, kind, pane)
}

#[tauri::command(rename_all = "camelCase")]
pub fn clipboard_diff_content(
    comparisons: State<'_, Comparisons>,
    snapshot_id: String,
) -> Result<String> {
    comparisons
        .clips
        .lock()
        .get(&snapshot_id)
        .cloned()
        .ok_or_else(|| {
            CoreError::Invariant(
                "This clipboard comparison has expired. Compare to Clipboard again.".into(),
            )
        })
}

/// Compare-and-swap, including the undo history. A losing renderer receives the winner's
/// state and rebases its pending transaction before retrying; it never overwrites a peer.
#[tauri::command(rename_all = "camelCase")]
pub fn shared_document_sync(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    comparisons: State<'_, Comparisons>,
    path: String,
    expected: Option<u32>,
    next: Option<SharedDocumentSnapshot>,
) -> Result<Option<SharedDocumentSnapshot>> {
    let path = PathBuf::from(path);
    let (answer, changed) = state.with(|ws| comparisons.sync(ws, &path, expected, next))?;
    if changed {
        crate::emit::shared_document_changed(
            &app,
            &path.to_string_lossy(),
            answer.as_ref().unwrap().version,
        );
    }
    Ok(answer)
}

impl Comparisons {
    fn sync(
        &self,
        ws: &Workspace,
        path: &PathBuf,
        expected: Option<u32>,
        next: Option<SharedDocumentSnapshot>,
    ) -> Result<(Option<SharedDocumentSnapshot>, bool)> {
        // Read the workspace and update the runtime under the same workspace lock, so a
        // rename cannot leave an acknowledged edit behind at the old path.
        self.reconcile(ws);
        let tabs: HashSet<_> = ws
            .projects
            .values()
            .flat_map(|p| &p.tabs)
            .filter(|t| file_path(&t.kind) == Some(path))
            .map(|t| t.id)
            .collect();
        if tabs.is_empty() {
            return Ok((None, false));
        }
        let mut docs = self.documents.lock();
        let mut changed = false;
        if let Some(mut next) = next {
            let version = docs.get(path).map(|d| d.snapshot.version);
            if version == expected {
                next.version = version
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| CoreError::Invariant("document revision exhausted".into()))?;
                docs.insert(
                    path.clone(),
                    Document {
                        snapshot: next,
                        tabs,
                    },
                );
                changed = true;
            }
        }
        Ok((docs.get(path).map(|d| d.snapshot.clone()), changed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(text: &str) -> SharedDocumentSnapshot {
        SharedDocumentSnapshot {
            version: 0,
            state: text.into(),
            saved_text: "disk".into(),
            stamp: None,
        }
    }

    #[test]
    fn clipboard_refresh_reuses_tab_and_releases_old_snapshot() {
        let mut ws = Workspace::default();
        let project =
            workspace::open_project(&mut ws, vec!["/tmp/clipboard-test".into()], None).unwrap();
        let runtime = Comparisons::default();
        let path = PathBuf::from("/tmp/clipboard-test/file.txt");
        runtime
            .clips
            .lock()
            .insert("first".into(), "sensitive clipboard".into());
        let tab = open_comparison(&mut ws, project, path.clone(), "first".into()).unwrap();
        runtime.clips.lock().insert("second".into(), String::new());
        assert_eq!(
            open_comparison(&mut ws, project, path, "second".into()).unwrap(),
            tab
        );
        runtime.reconcile(&ws);
        assert_eq!(runtime.clips.lock().len(), 1);
        assert_eq!(runtime.clips.lock()["second"], "");
        assert!(
            !serde_json::to_string(&ws)
                .unwrap()
                .contains("sensitive clipboard")
        );
        workspace::close_tab(&mut ws, project, tab, false).unwrap();
        runtime.reconcile(&ws);
        assert!(runtime.clips.lock().is_empty());
    }

    #[test]
    fn shared_document_cas_lifetime_and_rename() {
        let mut ws = Workspace::default();
        let project =
            workspace::open_project(&mut ws, vec!["/tmp/clipboard-test".into()], None).unwrap();
        let runtime = Comparisons::default();
        let path = PathBuf::from("/tmp/clipboard-test/file.txt");
        let tab = open_comparison(&mut ws, project, path.clone(), "clip".into()).unwrap();
        let (initial, changed) = runtime
            .sync(&ws, &path, None, Some(snapshot("first")))
            .unwrap();
        assert!(changed);
        assert_eq!(initial.unwrap().version, 1);
        let (winner, changed) = runtime
            .sync(&ws, &path, Some(1), Some(snapshot("winner")))
            .unwrap();
        assert!(changed);
        let (loser, changed) = runtime
            .sync(&ws, &path, Some(1), Some(snapshot("stale")))
            .unwrap();
        assert!(!changed);
        assert_eq!(loser, winner);
        let renamed = PathBuf::from("/tmp/clipboard-test/renamed.txt");
        workspace::retarget_paths(&mut ws, &[(path.clone(), renamed.clone())]);
        assert_eq!(runtime.sync(&ws, &renamed, None, None).unwrap().0, winner);
        assert!(
            runtime
                .sync(&ws, &path, Some(2), Some(snapshot("old path")))
                .unwrap()
                .0
                .is_none()
        );
        workspace::close_tab(&mut ws, project, tab, false).unwrap();
        runtime.reconcile(&ws);
        assert!(runtime.documents.lock().is_empty());
    }
}
