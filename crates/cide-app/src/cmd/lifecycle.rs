//! Lifecycle commands: what a window needs to know on the way back up.

use tauri::State;

use crate::lifecycle::{self, PaneRestore};
use crate::workspace_state::WorkspaceState;

/// What to do with each pane's session on launch.
///
/// The frontend asks once per window, keeps the entries whose `window` is its own, and
/// spawns only the `eager` ones. Everything else renders its splash and waits to be asked:
/// reopening a six-pane project must not silently start six agents at once.
///
/// Advice rather than instruction, because spawning belongs to the frontend — only it knows
/// a pane's size, and a child spawned before its slot is laid out draws a ruined first
/// frame. This says what each pane *may* become, not when.
#[tauri::command]
pub fn app_restore_plan(state: State<'_, WorkspaceState>) -> Vec<PaneRestore> {
    lifecycle::plan_restore(&state.snapshot())
}
