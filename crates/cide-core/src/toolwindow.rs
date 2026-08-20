//! The git tool window's persisted state: whether it is open, how tall, and which per-file
//! history tabs it holds. (M19)
//!
//! Free functions over [`Workspace`], like the rest of this crate — no `AppHandle`, no IPC, no
//! disk. `cide-app`'s `cmd::toolwindow` is the thin `#[tauri::command]` layer over these, and
//! everything decidable is decided here so it can be tested without a window.
//!
//! # Why the state lives on `Project` and not in `Settings`
//!
//! `Settings` is global and rides every `cide://workspace-changed` to **every** window, so a
//! persisted "open" toggled in one `PerProject` window would open another window's panel. A
//! `Project` cannot do that: a shell window renders exactly the project its own `WindowRole`
//! names, and [`crate::workspace::rebuild_windows`] gives each project one shell in `PerProject`
//! mode and one shell overall in `Stacked` mode. So the bleed is impossible by construction
//! rather than by convention — provided the panel is drawn only in a `shell` window, which is a
//! rule `App.tsx` enforces and which exists for the `DetachedTab` role that would otherwise be
//! the hole.
//!
//! Not a map keyed by `WindowLabel` either, although those labels *are* stable across a restart.
//! A history tab names a file in a repository in a *project*; keyed per window, a `Stacked` shell
//! would show project A's history while displaying project B. Per project it also survives a
//! `window_mode` flip, which re-mints labels and would silently empty a per-window map.
//!
//! # What is stored, and what is deliberately not
//!
//! A history tab is a **query**, not a document: an id, a repository, a path. No commits, no
//! diffs, no blobs. Same rule as `DiffSpec` — a saved log would be megabytes nobody reads back
//! *and* stale, because the branch moves while the tab is open. `HistoryPane` re-runs the query
//! when it mounts, and a commit is cheap to re-read.
//!
//! # The Log tab is a position, not an entity
//!
//! It is always present, always first, and can never be closed, so it has no id and does not
//! appear in [`ToolWindowState::history`]. `active: None` *is* the Log tab. Giving it an id and
//! putting it at `history[0]` would make every close path carry an "unless it is the first one"
//! clause, and the first `history.retain(...)` written without that clause loses the panel's only
//! permanent tab.

use cide_ipc::{HistoryTab, HistoryTabId, ProjectId, RepoId, Workspace};

use crate::error::{CoreError, Result};
use crate::workspace::{bump, project, project_mut};

/// Geometry and visibility, each field optional so one gesture writes one thing.
///
/// One function rather than three because the three share a broadcast and a drag commits exactly
/// one of them: a `pointerup` sends a height and nothing else, and the rail button sends an open
/// flag and nothing else. Three commands would be three round trips and three `rev` bumps for a
/// gesture that changed one number.
pub fn set_layout(
    ws: &mut Workspace,
    project_id: ProjectId,
    open: Option<bool>,
    height: Option<u16>,
    log_split: Option<u16>,
    files_as_tree: Option<bool>,
) -> Result<()> {
    let current = &project(ws, project_id)?.tool_window;
    let mut next = current.clone();
    if let Some(open) = open {
        next.open = open;
    }
    if let Some(height) = height {
        next.height = height;
    }
    /*
     * The divider goes to whichever tab is in front. (M21)
     *
     * One field on the wire and two places it can land, rather than two commands: the caller is a
     * splitter that knows it dragged *this panel's* divider and has no business knowing which
     * kind of tab is showing. `active` already says, and it is the same value the panel drew
     * from, so the two cannot disagree.
     *
     * A History tab that is not in front is not written, which is the point of routing on
     * `active` rather than on "every history tab": dragging the Log tab's divider must not
     * silently re-lay out four history diffs the user is not looking at.
     */
    if let Some(split) = log_split {
        match next.active {
            None => next.log_split = split,
            Some(id) => {
                if let Some(tab) = next.history.iter_mut().find(|t| t.id == id) {
                    tab.split = Some(split);
                }
            }
        }
    }
    if let Some(tree) = files_as_tree {
        next.files_as_tree = tree;
    }
    // Clamped where the patch lands rather than on every read, so the stored document converges
    // on a legal value instead of being quietly corrected for ever — the rule
    // `SidebarSettings::clamped` already follows.
    let next = next.clamped();
    // A no-op must not bump. A splitter that commits the height it already had would broadcast a
    // snapshot to every window for a gesture that changed nothing, which is the guard
    // `retarget_diff` and `tab_set_dirty` both carry for the same reason.
    if next == *current {
        return Ok(());
    }
    project_mut(ws, project_id)?.tool_window = next;
    bump(ws);
    Ok(())
}

/// Bring a tab to the front. `tab` of `None` is the Log tab, which always exists.
pub fn activate(
    ws: &mut Workspace,
    project_id: ProjectId,
    tab: Option<HistoryTabId>,
) -> Result<()> {
    let state = &project(ws, project_id)?.tool_window;
    if let Some(id) = tab
        && !state.history.iter().any(|t| t.id == id)
    {
        return Err(CoreError::Invariant(format!(
            "history tab {id} is not open in this project"
        )));
    }
    if state.active == tab && state.open {
        return Ok(());
    }
    let state = &mut project_mut(ws, project_id)?.tool_window;
    state.active = tab;
    // Activating reveals. Every caller got here by asking to *see* something, and a version that
    // left a closed panel closed would be a command that sometimes did nothing visible — the
    // argument `sidebarView::showPanel` makes for the sidebar's own commands.
    state.open = true;
    bump(ws);
    Ok(())
}

/// Show a file's history: **open-or-activate, never a duplicate.**
///
/// Keyed on `(repo, path)` and not on the path alone, because in a multi-root project two
/// repositories can both contain `src/main.rs` and they are two different histories.
///
/// Four menus reach this — the file tab strip, the file tree, the git changes tree and the
/// editor's own context menu — and they reach it for the same file often. Without the lookup,
/// right-clicking one file in three places gives three identical tabs, each re-running the same
/// walk. `id` is only consumed when the tab is genuinely new, so a caller may mint one
/// unconditionally.
pub fn open_history(
    ws: &mut Workspace,
    project_id: ProjectId,
    repo: RepoId,
    path: &str,
    id: HistoryTabId,
) -> Result<HistoryTabId> {
    if path.is_empty() {
        return Err(CoreError::Invariant(
            "a history tab needs a repo-relative path".into(),
        ));
    }
    let state = &project(ws, project_id)?.tool_window;
    if let Some(existing) = state
        .history
        .iter()
        .find(|t| t.repo == repo && t.path == path)
    {
        let id = existing.id;
        activate(ws, project_id, Some(id))?;
        return Ok(id);
    }
    let tab = HistoryTab {
        split: None,
        id,
        repo,
        path: path.to_owned(),
        title: basename(path),
    };
    let state = &mut project_mut(ws, project_id)?.tool_window;
    state.history.push(tab);
    state.active = Some(id);
    state.open = true;
    bump(ws);
    Ok(id)
}

/// Close one history tab.
///
/// **The successor is the left neighbour, then the right, then the Log tab — and never
/// "nothing".** A panel that vanished because you closed one of its tabs reads as a crash rather
/// than as a close, and the Log tab is always there to fall back to, so the closed state is
/// unreachable from this gesture. `close_tab`'s left-neighbour rule is the same choice for the
/// same reason: the thing you were reading before is a better guess than the thing after it.
pub fn close_history(ws: &mut Workspace, project_id: ProjectId, tab: HistoryTabId) -> Result<()> {
    let state = &project(ws, project_id)?.tool_window;
    let Some(index) = state.history.iter().position(|t| t.id == tab) else {
        return Err(CoreError::Invariant(format!(
            "history tab {tab} is not open in this project"
        )));
    };
    let was_active = state.active == Some(tab);
    let state = &mut project_mut(ws, project_id)?.tool_window;
    state.history.remove(index);
    if was_active {
        state.active = index
            .checked_sub(1)
            .and_then(|left| state.history.get(left))
            .or_else(|| state.history.get(index))
            .map(|t| t.id);
    }
    bump(ws);
    Ok(())
}

/// The basename, which is what a tab is called. Mirrors `toolWindowModel.ts::historyTitle`.
fn basename(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((_, name)) if !name.is_empty() => name.to_owned(),
        _ => path.to_owned(),
    }
}
