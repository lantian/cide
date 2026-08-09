//! Pane commands — the split tree, driven from the frontend.
//!
//! Every one of these is a thin wrapper over `cide-core::layout`, which owns the invariants.
//! The interesting decision here is what a split does *not* do: it creates the pane and
//! leaves `session: None`.
//!
//! Spawning belongs to the frontend because only the frontend knows the pane's size. A
//! child spawned before its slot is laid out gets the fallback 80x24 and, for a fullscreen
//! TUI, draws a ruined first frame — measured directly in M0, where `TIOCGWINSZ` on a real
//! `claude` child reported `rows=1 cols=2`. So the pane appears first, the frontend measures
//! it, spawns, and calls [`pane_bind_session`].

use cide_core::{CoreError, layout, workspace};
use cide_ipc::{
    Axis, Direction, Pane, PaneId, PaneKind, PaneRole, ProjectId, SessionId, Side, SplitId,
    SplitIntent, TabId, TabKind,
};
use tauri::State;

use crate::workspace_state::WorkspaceState;

/// Returned by every mutation so the caller can order or discard a later snapshot.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutated {
    pub rev: u64,
}

/// What a split produces when the caller does not name an intent.
///
/// This is the **only** behavioural difference between the pinned console and an ordinary
/// full-screen Claude tab, and it lives here rather than in the frontend so both windows of
/// a detached project agree:
///
/// * In the pinned console, splitting sideways gives a shell and splitting downwards gives
///   another Claude session. That matches the mock, and it keeps the *primary* session
///   singular — the console is the project's one conversation plus whatever supports it.
/// * In a `ClaudeFull` tab every split is a new session, which is what that tab is for.
fn default_intent(kind: &TabKind, axis: Axis) -> SplitIntent {
    match kind {
        TabKind::ClaudeHome => match axis {
            Axis::Row => SplitIntent::Shell,
            Axis::Col => SplitIntent::NewClaude,
        },
        _ => SplitIntent::NewClaude,
    }
}

/// The pane a given intent describes, before any process exists.
fn pane_for(intent: &SplitIntent, project_name: &str) -> Pane {
    let (kind, suffix) = match intent {
        SplitIntent::NewClaude | SplitIntent::ForkPrimary => (PaneKind::Claude, "claude"),
        SplitIntent::Mirror { .. } => (PaneKind::Claude, "claude"),
        SplitIntent::Shell => (PaneKind::Shell, "bash"),
        SplitIntent::Diff => (PaneKind::Diff, "claude — diff"),
    };
    Pane {
        id: PaneId::new(),
        kind,
        // Only the console's founding pane is Primary; everything a split creates can be
        // closed. Marking a second pane Primary would make two panes unclosable and leave
        // the tab with no way back to one.
        role: PaneRole::Auxiliary,
        // A mirror shares an existing session rather than waiting for one to be bound.
        session: match intent {
            SplitIntent::Mirror { session } => Some(*session),
            _ => None,
        },
        title: format!("{project_name} : {suffix}"),
    }
}

/// Split a pane in two.
///
/// A `None` intent asks for the tab's default — see [`default_intent`].
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub fn pane_split(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    axis: Axis,
    side: Side,
    intent: Option<SplitIntent>,
) -> Result<PaneId, CoreError> {
    state.update(|ws| {
        let name = workspace::project(ws, project)?.name.clone();
        let t = workspace::tab_mut(ws, project, tab)?;
        let intent = intent.unwrap_or_else(|| default_intent(&t.kind, axis));
        let fresh = pane_for(&intent, &name);
        layout::split(&mut t.tree, pane, axis, side, fresh)
    })
}

/// Close a pane.
///
/// Refused for the console's primary pane while it is alone ([`CoreError::PanePrimary`]) and
/// for the last pane of any tab ([`CoreError::LastPane`]). The session is untouched: it is
/// owned by the registry, and a closed pane is a detached sink, not a dead process. The
/// caller decides separately whether the child should also go.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_close(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
) -> Result<Mutated, CoreError> {
    state
        .update(|ws| {
            let t = workspace::tab_mut(ws, project, tab)?;
            layout::close(&mut t.tree, pane)
        })
        .map(|()| Mutated { rev: state.rev() })
}

#[tauri::command(rename_all = "camelCase")]
pub fn pane_focus(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
) -> Result<Mutated, CoreError> {
    state
        .update(|ws| {
            let t = workspace::tab_mut(ws, project, tab)?;
            layout::focus(&mut t.tree, pane)
        })
        .map(|()| Mutated { rev: state.rev() })
}

/// Give one pane the whole tab, or clear the flag with `None`.
///
/// Not a tree mutation — it sets a flag the renderer honours, so restore is exact and no
/// terminal reflows. Maximizing also focuses, since the renderer hides every other pane.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_maximize(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: Option<PaneId>,
) -> Result<Mutated, CoreError> {
    state
        .update(|ws| {
            let t = workspace::tab_mut(ws, project, tab)?;
            layout::maximize(&mut t.tree, pane)
        })
        .map(|()| Mutated { rev: state.rev() })
}

/// Move a divider. Returns the value actually stored, which may be clamped.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_set_ratio(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    split: SplitId,
    ratio: f32,
) -> Result<f32, CoreError> {
    state.update(|ws| {
        let t = workspace::tab_mut(ws, project, tab)?;
        layout::set_ratio(&mut t.tree, split, ratio)
    })
}

/// The pane the keyboard should move to, or `None` at the edge of the tree.
///
/// Read-only: it answers where focus *would* go. The caller follows with [`pane_focus`],
/// which is what keeps "navigate" and "focus" one decision rather than two.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_navigate(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    direction: Direction,
) -> Result<Option<PaneId>, CoreError> {
    let ws = state.snapshot();
    let t = workspace::tab(&ws, project, tab)?;
    Ok(layout::navigate(&t.tree, pane, direction))
}

#[tauri::command(rename_all = "camelCase")]
pub fn pane_swap(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    a: PaneId,
    b: PaneId,
) -> Result<Mutated, CoreError> {
    state
        .update(|ws| {
            let t = workspace::tab_mut(ws, project, tab)?;
            layout::swap(&mut t.tree, a, b)
        })
        .map(|()| Mutated { rev: state.rev() })
}

/// Record which session a pane is showing.
///
/// Called once the frontend has measured the pane and spawned a child at the right size.
/// The binding is what survives a restart: `SessionId` is the value passed to
/// `claude --session-id`, so a persisted pane knows which conversation to resume.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_bind_session(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    session: SessionId,
) -> Result<Mutated, CoreError> {
    state
        .update(|ws| {
            let t = workspace::tab_mut(ws, project, tab)?;
            let Some(p) = t.tree.panes.get_mut(&pane) else {
                return Err(CoreError::NoSuchPane(pane));
            };
            p.session = Some(session);
            Ok(())
        })
        .map(|()| Mutated { rev: state.rev() })
}
