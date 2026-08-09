//! File-tab and document commands. (M9)
//!
//! Two pairs, and they are unrelated to each other. `tab_open_file` / `tab_set_dirty` are
//! workspace mutations in the same shape as everything in `cmd::project`. `file_read` /
//! `file_write` touch the disk instead, and are the only commands in the app that do
//! arbitrary blocking IO on a path the user chose — so they are the only ones that go
//! through `spawn_blocking`. A `cargo build` saturating the page cache, or a project on a
//! stalled NFS mount, would otherwise freeze the event loop and with it every terminal in
//! the window.

use std::path::PathBuf;

use cide_core::document;
use cide_core::workspace;
use cide_core::{CoreError, Result};
use cide_ipc::{FileDoc, Pane, PaneId, PaneKind, PaneRole, ProjectId, TabId, TabKind};
use tauri::State;

use crate::cmd::project::Mutated;
use crate::workspace_state::WorkspaceState;

/// Open a file tab, or activate the one already showing this path.
///
/// Re-opening rather than duplicating is not a nicety: two tabs over one path are two
/// buffers over one file, and whichever saves second silently discards the other's edits.
/// The domain has no opinion about paths, so the search is here.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_file(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    path: String,
) -> Result<TabId> {
    let path = PathBuf::from(path);
    state.update(|ws| {
        let existing = workspace::project(ws, project)?
            .tabs
            .iter()
            .find(|t| matches!(&t.kind, TabKind::File { path: p, .. } if *p == path))
            .map(|t| t.id);
        if let Some(id) = existing {
            workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }

        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        workspace::open_tab(
            ws,
            project,
            TabKind::File { path, dirty: false },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                // Auxiliary, like every pane outside the pinned console: splitting a file
                // tab and closing one of the halves must not be refused, and closing the
                // last one closes the tab.
                role: PaneRole::Auxiliary,
                session: None,
                title,
            },
        )
    })
}

/// Record whether a file tab has unsaved edits.
///
/// The dirty flag lives in the Rust-owned tree rather than in the editor component because
/// it outlives the component: a tab switch unmounts nothing today, but a detached editor
/// window is a different JavaScript realm, and the tab strip that draws the dot is in the
/// other one.
///
/// Nothing yet *guards* on it. `tab_close` does not consult it and neither does
/// `app_quit_requested`, so closing a tab with unsaved edits discards them without asking —
/// the dot in the tab strip is the only warning there is. Putting the flag in the tree is
/// what makes that guard writable in one place later; it is not that guard.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_set_dirty(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    dirty: bool,
) -> Result<Mutated> {
    state.update(|ws| {
        let t = workspace::tab_mut(ws, project, tab)?;
        let TabKind::File { dirty: flag, .. } = &mut t.kind else {
            return Err(CoreError::Invariant(format!(
                "tab {tab} is not a file tab and has no dirty state"
            )));
        };
        // Bumping `rev` on a no-op would broadcast a snapshot per keystroke: the editor
        // reports on every transaction, and only the transitions are news.
        if *flag == dirty {
            return Ok(Mutated { rev: ws.rev });
        }
        *flag = dirty;
        workspace::bump(ws);
        Ok(Mutated { rev: ws.rev })
    })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn file_read(path: String) -> Result<FileDoc> {
    blocking(move || document::read(&PathBuf::from(path))).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn file_write(path: String, text: String) -> Result<()> {
    blocking(move || document::write(&PathBuf::from(path), &text)).await
}

/// Run a fallible blocking job on the pool, reporting a lost worker as an IO error.
///
/// The join can only fail if the task panicked or the runtime is shutting down. Neither is
/// something a caller can act on differently from a failed read, and inventing a variant
/// for it would put a case in the frontend's error switch that no test can reach.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tauri::async_runtime::spawn_blocking(job).await {
        Ok(result) => result,
        Err(error) => Err(CoreError::Io(format!("file worker failed: {error}"))),
    }
}
