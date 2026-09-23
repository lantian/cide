//! Lifecycle commands: what a window needs to know on the way back up.

use std::collections::HashSet;

use tauri::State;

use crate::lifecycle::{self, PaneRestore};
use crate::state::SessionRegistry;
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
///
/// # A pane whose child is alive has nothing to restore
///
/// "Once per project per window" is once per *mount of the page*, and the page is not only
/// mounted at launch: a webview reload — Vite's full reload in a debug build, above all, which
/// any edit under `ui/` can set off — mounts it again over a process whose children are all
/// still running. The plan used to answer for every Claude pane regardless, and only the console
/// is `eager`, so every other pane came back as a **Resume splash over a live `claude`**:
/// reported as review tabs "closing by themselves", the reviewer still running behind a card
/// that offered to start it. Pressing it then depended on where the transcript was filed, which
/// is the other half of that report (see `claude_tab::open_with_prompt`).
///
/// So a pane whose session is running in this process is left out, and with no entry it does
/// exactly what it did before the reload: `TerminalPane` adopts the session the tree names. An
/// *exited* child stays in the plan — its pane is offered the same Resume it would be offered
/// after a restart, which is the honest state for a conversation nothing is running.
///
/// The `State` parameter does not drift `contract/commands.json` — `session_resumable`'s doc
/// carries that argument.
#[tauri::command]
pub fn app_restore_plan(
    state: State<'_, WorkspaceState>,
    sessions: State<'_, SessionRegistry>,
    project: Option<cide_ipc::ProjectId>,
) -> Vec<PaneRestore> {
    let ws = state.snapshot();
    let running =
        |session: cide_ipc::SessionId| sessions.get(session).is_some_and(|pty| !pty.has_exited());
    let live: HashSet<cide_ipc::PaneId> = ws
        .projects
        .values()
        .flat_map(|project| {
            project
                .tabs
                .iter()
                .flat_map(|tab| tab.tree.panes.values())
                .chain(project.detached.values())
        })
        .filter(|pane| pane.session.is_some_and(running))
        .map(|pane| pane.id)
        .collect();
    let mut plan = lifecycle::plan_restore(&ws, project);
    plan.retain(|entry| !live.contains(&entry.pane));
    plan
}
