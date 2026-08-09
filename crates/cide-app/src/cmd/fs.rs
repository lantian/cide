//! File tree and file operation commands.
//!
//! Thin, like every other handler here: unwrap arguments, check the path belongs to the
//! project, call `cide-fs`, wrap the result. The one rule enforced at this layer rather than
//! below it is containment — every path argument is checked against the project's roots
//! before anything touches the disk, because this is where a path stops being a value the
//! backend produced and becomes one the webview sent back.

use std::path::PathBuf;

use cide_fs::{FsError, ops};
use cide_ipc::{FsStatus, ProjectId, TreeRow};
use tauri::{Manager, State};

use crate::files::FsRegistry;
use crate::workspace_state::WorkspaceState;

/// The most rows one request will return.
///
/// The tree draws about thirty. A frontend bug asking for a million must cost a clamped
/// answer, not a serialised megabyte.
const MAX_ROWS: usize = 2048;

fn project_fs(
    registry: &FsRegistry,
    project: ProjectId,
) -> Result<std::sync::Arc<crate::files::ProjectFs>, FsError> {
    registry.get(project).ok_or(FsError::NoIndex)
}

/// Walk a project's roots and start watching them.
///
/// Called by the frontend after `project.open` rather than from `project_open` itself: the
/// walk is seconds of work on a large repository, and a project that opens instantly with an
/// empty tree that fills in is better than one that hangs before it appears. The picker is
/// answerable from the moment this starts.
#[tauri::command(rename_all = "camelCase")]
pub fn fs_index(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
) -> Result<FsStatus, FsError> {
    let roots: Vec<PathBuf> = state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .map(|p| p.roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    if roots.is_empty() {
        return Err(FsError::NoIndex);
    }
    Ok(registry.index(&app, project, roots))
}

/// Drop a project's index and stop watching it. Idempotent.
#[tauri::command(rename_all = "camelCase")]
pub fn fs_close(registry: State<'_, FsRegistry>, project: ProjectId) -> bool {
    registry.remove(project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn fs_status(registry: State<'_, FsRegistry>, project: ProjectId) -> Result<FsStatus, FsError> {
    Ok(project_fs(&registry, project)?.status())
}

/// How many rows the tree currently has. The virtual scroller's range.
#[tauri::command(rename_all = "camelCase")]
pub fn fs_tree_count(registry: State<'_, FsRegistry>, project: ProjectId) -> Result<u32, FsError> {
    Ok(project_fs(&registry, project)?.with_index(|index| index.count() as u32))
}

/// The rows in `[offset, offset + len)`.
#[tauri::command(rename_all = "camelCase")]
pub fn fs_tree_rows(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    offset: u32,
    len: u32,
) -> Result<Vec<TreeRow>, FsError> {
    let len = (len as usize).min(MAX_ROWS);
    Ok(project_fs(&registry, project)?.with_index(|index| index.rows(offset as usize, len)))
}

/// Expand a directory. Returns the new row count.
#[tauri::command(rename_all = "camelCase")]
pub fn fs_expand(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<u32, FsError> {
    let fs = project_fs(&registry, project)?;
    fs.with_index_mut(|index| index.expand(&path))
        .map(|n| n as u32)
        .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))
}

/// Collapse a directory. Returns the new row count.
#[tauri::command(rename_all = "camelCase")]
pub fn fs_collapse(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<u32, FsError> {
    let fs = project_fs(&registry, project)?;
    fs.with_index_mut(|index| index.collapse(&path))
        .map(|n| n as u32)
        .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))
}

/// Expand everything above a path and return the row it sits on.
///
/// `None` rather than an error when the path is not in the tree: revealing a file that is
/// gitignored, or that has just been deleted, is an ordinary thing for the editor to ask and
/// the honest answer is "there is no row for that".
#[tauri::command(rename_all = "camelCase")]
pub fn fs_reveal(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<Option<u32>, FsError> {
    let fs = project_fs(&registry, project)?;
    Ok(fs.with_index_mut(|index| index.reveal(&path).map(|n| n as u32)))
}

#[tauri::command(rename_all = "camelCase")]
pub fn fs_read_file(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<String, FsError> {
    let fs = project_fs(&registry, project)?;
    ops::check_within(&fs.root_paths(), &path)?;
    ops::read_to_string(&path)
}

#[tauri::command(rename_all = "camelCase")]
pub fn fs_write_file(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
    contents: String,
) -> Result<(), FsError> {
    let fs = project_fs(&registry, project)?;
    ops::check_within(&fs.root_paths(), &path)?;
    ops::write(&path, &contents)
}

#[tauri::command(rename_all = "camelCase")]
pub fn fs_create(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
    directory: bool,
) -> Result<(), FsError> {
    let fs = project_fs(&registry, project)?;
    ops::check_within(&fs.root_paths(), &path)?;
    ops::create(&path, directory)
}

#[tauri::command(rename_all = "camelCase")]
pub fn fs_rename(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    from: PathBuf,
    to: PathBuf,
) -> Result<(), FsError> {
    let fs = project_fs(&registry, project)?;
    let roots = fs.root_paths();
    ops::check_within(&roots, &from)?;
    ops::check_within(&roots, &to)?;
    ops::rename(&from, &to)
}

/// Move paths to the desktop trash. Returns where each one landed.
///
/// Trash rather than `unlink`, and the reason is worth stating where it can be read: a file
/// tree's delete is one keystroke away from a misclick, and the freedesktop trash is the only
/// undo a file manager offers. See `cide_fs::trash`.
#[tauri::command(rename_all = "camelCase")]
pub fn fs_delete(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    paths: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, FsError> {
    let fs = project_fs(&registry, project)?;
    let roots = fs.root_paths();
    let mut trashed = Vec::with_capacity(paths.len());
    for path in &paths {
        ops::check_within(&roots, path)?;
    }
    for path in &paths {
        trashed.push(ops::delete(path)?);
    }
    Ok(trashed)
}

/// Stop watching everything. The quit path.
pub fn close_all(app: &tauri::AppHandle) {
    if let Some(registry) = app.try_state::<FsRegistry>() {
        registry.close_all();
    }
}
