//! The git tool window: whether it is open, how tall, and which per-file history tabs it holds.
//! (M19)
//!
//! Glue only, as every module under `cmd/` is. Each command is a `state.update` around a free
//! function in [`cide_core::toolwindow`], which is where the rules and their arguments live and
//! where they can be tested without a window.
//!
//! **None of these is `async`, and none touches the disk.** The same reasoning as
//! `cmd::file::tab_open_diff`: the panel re-reads through `git_log` / `git_blame` when it mounts,
//! and those are the blocking calls, already on `spawn_blocking`. Making a state mutation `async`
//! would put a lock acquisition behind the runtime for no gain.
//!
//! **`project` is optional on the two a projectless shell can reach.** A shell with no project
//! open has a tool window of its own since M74 — [`cide_ipc::Workspace::tool_window`] — and
//! `null` names it. The other two keep a required [`ProjectId`]: a history tab names a file in a
//! repository in a project, so there is no gesture that could ask for one without one.
//!
//! Every one of them bumps `rev` and broadcasts through `state.update`, so a toggle in one window
//! reaches the others. That is a real cost worth naming: a tool window is toggled dozens of times
//! an hour, so this is a more frequent broadcast than most — but it is the same class every tab
//! activation already pays, and the alternative is the state living in the webview, which is what
//! the persistence requirement rules out.

use cide_ipc::{HistoryTabId, ProjectId, RepoId};
use tauri::State;

use crate::workspace_state::WorkspaceState;

type Result<T> = cide_core::Result<T>;

/// The panel's arrangement: visibility, geometry, and how the details pane lists its files.
/// Every field is optional so one gesture writes one thing.
///
/// One command rather than four because they share a broadcast, and a gesture commits exactly one
/// of them: a `pointerup` sends a height and nothing else, the rail button sends an open flag and
/// nothing else, the ⊞ toggle sends `filesAsTree`. Four commands would be four round trips and
/// four `rev` bumps for gestures that each moved one value.
#[tauri::command(rename_all = "camelCase")]
pub fn tool_window_set_layout(
    state: State<'_, WorkspaceState>,
    project: Option<ProjectId>,
    open: Option<bool>,
    height: Option<u16>,
    log_split: Option<u16>,
    files_as_tree: Option<bool>,
) -> Result<()> {
    state.update(|ws| {
        cide_core::toolwindow::set_layout(ws, project, open, height, log_split, files_as_tree)
    })
}

/// Bring a tab to the front, revealing the panel. `tab` of `None` is the Log tab, and
/// `project` of `None` is the projectless shell's own panel — where `None`/`None` is the only
/// pair it can be asked for, since that panel holds no history.
#[tauri::command(rename_all = "camelCase")]
pub fn tool_window_activate(
    state: State<'_, WorkspaceState>,
    project: Option<ProjectId>,
    tab: Option<HistoryTabId>,
) -> Result<()> {
    state.update(|ws| cide_core::toolwindow::activate(ws, project, tab))
}

/// Show a file's history — open-or-activate, keyed on the repository and the path.
///
/// The id is minted here rather than by the caller so a webview cannot hand in one that collides
/// with a tab it cannot see, and it is *returned* because the caller needs it to know which tab
/// its gesture ended on. It is only consumed when the tab is genuinely new: opening a file whose
/// history is already up re-activates that tab and answers with the id it already had.
#[tauri::command(rename_all = "camelCase")]
pub fn tool_window_open_history(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
) -> Result<HistoryTabId> {
    state.update(|ws| {
        cide_core::toolwindow::open_history(ws, project, repo, &path, HistoryTabId::new())
    })
}

/// Close one history tab. The Log tab has no id and cannot be closed.
#[tauri::command(rename_all = "camelCase")]
pub fn tool_window_close_history(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: HistoryTabId,
) -> Result<()> {
    state.update(|ws| cide_core::toolwindow::close_history(ws, project, tab))
}
