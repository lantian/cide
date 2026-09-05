//! Lifecycle commands: what a window needs to know on the way back up.

use tauri::State;

use crate::lifecycle::{self, PaneRestore};
use crate::workspace_state::WorkspaceState;

/// What to do with each pane's session when its project appears in a window.
///
/// The frontend asks once per project per window — at launch for every project the workspace
/// holds, and again for a project opened during the run — and spawns only the `eager`
/// entries. Everything else renders its splash and waits to be asked: reopening a six-pane
/// project must not silently start six agents at once.
///
/// `project` narrows the answer to one project; `None` is the whole workspace, which is what
/// the frontend asked before projects could be reopened with their layouts and is kept for
/// anything driving the app from a script. See `lifecycle::plan_restore` for why the
/// per-project form exists.
///
/// Advice rather than instruction, because spawning belongs to the frontend — only it knows
/// a pane's size, and a child spawned before its slot is laid out draws a ruined first
/// frame. This says what each pane *may* become, not when.
#[tauri::command]
pub fn app_restore_plan(
    state: State<'_, WorkspaceState>,
    project: Option<cide_ipc::ProjectId>,
) -> Vec<PaneRestore> {
    lifecycle::plan_restore(&state.snapshot(), project)
}
