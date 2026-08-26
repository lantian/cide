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
    SplitIntent, SplitOutcome, TabId, TabKind,
};
use tauri::{Manager, State};

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
///
/// Every arm here produces a pane a user can then use. There is deliberately no arm making a
/// `PaneKind::Diff`: what a diff pane renders lives on its *tab* (`TabKind::Diff { spec }`),
/// so one split out beside a Claude console would have nothing to show. `SplitIntent::Diff`
/// was removed for that reason — see the enum in `cide-ipc`.
fn pane_for(intent: &SplitIntent, project_name: &str) -> Pane {
    let (kind, suffix) = match intent {
        SplitIntent::NewClaude | SplitIntent::ForkPrimary => (PaneKind::Claude, "claude"),
        SplitIntent::Mirror { .. } => (PaneKind::Claude, "claude"),
        SplitIntent::Shell => (PaneKind::Shell, "bash"),
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
        // A mirror shows the same child as its source, so the CLI's conversation for it is
        // whatever the source already recorded; this pane learns its own from the next hook
        // frame rather than copying a value that may be a turn out of date.
        conversation: None,
        conversation_since: None,
        title: format!("{project_name} : {suffix}"),
    }
}

/// Which gesture an axis names, now that a tab is a column of rows.
///
/// Re-routed here rather than in `cide-core` on purpose: `layout::split` still stacks a tile
/// inside one cell and is still the right primitive, and keeping the routing in the command
/// layer is what lets every shipped gesture — the header `⊞`, `pane.split.right`,
/// `pane.split.down`, `claude.fork`, `claude.mirror`, `claude.split.newSession`,
/// `terminal.splitBelow` — build rows with no keymap, contract or frontend change at all.
fn apply_split(
    tree: &mut cide_ipc::PaneTree,
    pane: PaneId,
    axis: Axis,
    side: Side,
    fresh: Pane,
) -> Result<PaneId, CoreError> {
    match axis {
        Axis::Row => layout::add_tile(tree, pane, side, fresh),
        Axis::Col => layout::add_row(tree, Some(pane), side, fresh),
    }
}

/// Split a pane in two.
///
/// Sideways adds a *tile* to the pane's row; downwards adds a full-width *row* below it,
/// rather than stacking inside the pane's own cell — see [`apply_split`]. A `None` intent
/// asks for the tab's default, which is unchanged: a shell sideways, a new session
/// downwards ([`default_intent`]).
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
) -> Result<SplitOutcome, CoreError> {
    state.update(|ws| {
        let name = workspace::project(ws, project)?.name.clone();
        let t = workspace::tab_mut(ws, project, tab)?;
        let intent = intent.unwrap_or_else(|| default_intent(&t.kind, axis));
        let fresh = pane_for(&intent, &name);
        let pane = apply_split(&mut t.tree, pane, axis, side, fresh)?;
        Ok(SplitOutcome { pane, intent })
    })
}

/// A new full-width row holding one pane.
///
/// `after: None` appends at the bottom. `intent: None` asks for the tab's default, which is
/// the same one splitting downwards has always used — see [`default_intent`].
///
/// Like [`pane_split`], this leaves `session: None`: the frontend measures the pane and
/// spawns the child at the right size, then calls [`pane_bind_session`].
#[tauri::command(rename_all = "camelCase")]
pub fn pane_add_row(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    after: Option<PaneId>,
    side: Side,
    intent: Option<SplitIntent>,
) -> Result<SplitOutcome, CoreError> {
    state.update(|ws| {
        let name = workspace::project(ws, project)?.name.clone();
        let t = workspace::tab_mut(ws, project, tab)?;
        let intent = intent.unwrap_or_else(|| default_intent(&t.kind, Axis::Col));
        let fresh = pane_for(&intent, &name);
        let pane = layout::add_row(&mut t.tree, after, side, fresh)?;
        Ok(SplitOutcome { pane, intent })
    })
}

/// Close a pane.
///
/// Refused for the console's primary pane ([`CoreError::PanePrimary`]) however many panes
/// the tab has, and for the last pane of any tab ([`CoreError::LastPane`]). The frontend
/// withholds the close button for the primary as a courtesy; this is the enforcement.
/// The session is untouched: it is
/// owned by the registry, and a closed pane is a detached sink, not a dead process. The
/// caller decides separately whether the child should also go.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_close(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    force: bool,
) -> Result<Mutated, CoreError> {
    state
        .update(|ws| workspace::close_pane(ws, project, tab, pane, force))
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

/// Give every member of a pane's row the same share of it.
///
/// The user's words: *"set all panels in current row to same width (proportional)"*. The
/// dividers of one chain only — evening out a row cannot change its height, or touch the row
/// under it.
///
/// `axis` names which chain, exactly as it does for [`pane_split`]: `row` is the tiles beside
/// this pane, `col` the rows of the tab. Only `row` is reachable from the UI today, and the
/// parameter is here rather than hard-coded because the domain operation is one operation and
/// a second command for the other axis would be the same call with a constant in it.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_distribute(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    axis: Axis,
) -> Result<Mutated, CoreError> {
    state
        .update(|ws| {
            let t = workspace::tab_mut(ws, project, tab)?;
            layout::distribute(&mut t.tree, pane, axis)
        })
        .map(|()| Mutated { rev: state.rev() })
}

/// Move a divider. `ratio` is the **pair share** — the first of the two members this divider
/// separates within its row, not the split node's `a`-share.
///
/// The two are the same number for every two-pane tab and for the shipped console, which is
/// why the wire shape is unchanged and no caller had to move. Returns the value actually
/// stored, which may be clamped.
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
/// Called once the frontend has measured the pane and spawned a child at the right size, and
/// again whenever that pane restarts into a new one. The binding is what survives a restart:
/// `SessionId` is the value passed to `claude --session-id`, so a persisted pane knows which
/// conversation to resume.
///
/// The mutation itself is [`workspace::bind_session`], in `cide-core`, and it does one thing
/// more than this used to: for the console's primary pane it moves `Project::primary_session`
/// too. That field decides whether the console comes back *live* on the next launch or at a
/// Resume splash, and nothing had ever written it after project creation — see the function's
/// own note.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_bind_session(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    registry: State<'_, crate::state::SessionRegistry>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    session: SessionId,
) -> Result<Mutated, CoreError> {
    let out = state
        .update(|ws| workspace::bind_session(ws, project, tab, pane, session))
        .map(|()| Mutated { rev: state.rev() })?;

    // This is the only point at which a pid and a pane are both known, which is what the IDE
    // server needs: `claude` announces itself with `ide_connected{pid}`, and that pid is the
    // join key deciding which pane a selection or an `@`-mention belongs to. Attributing by
    // tab name or cwd instead would break the moment a project has two Claude panes, which is
    // the normal case here.
    if let Some(servers) = app.try_state::<crate::ide::IdeServers>()
        && let Some(s) = registry.get(session)
        && let Some(pid) = s.child_pid()
    {
        servers.bind_pane(project, pid, pane);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_core::layout::{MIN_TILE, add_row, add_tile, leaves, new_tree, validate};
    use cide_ipc::{LayoutNode, PaneTree};

    fn fresh() -> Pane {
        pane_for(&SplitIntent::Shell, "cide")
    }

    /// A console tab holding one row of `n` tiles.
    fn tree_of(n: usize) -> PaneTree {
        let first = Pane {
            id: PaneId::new(),
            kind: PaneKind::Claude,
            role: PaneRole::Primary,
            session: None,
            conversation: None,
            conversation_since: None,
            title: "cide : claude".into(),
        };
        let mut tree = new_tree(first);
        let mut last = tree.focused;
        for _ in 1..n {
            last = add_tile(&mut tree, last, Side::After, fresh()).expect("a tile joins");
        }
        tree
    }

    /// The tab's rows, each as the pane ids it holds in reading order.
    fn rows(tree: &PaneTree) -> Vec<Vec<PaneId>> {
        fn walk(node: &LayoutNode, out: &mut Vec<Vec<PaneId>>) {
            match node {
                LayoutNode::Split {
                    axis: Axis::Col,
                    a,
                    b,
                    ..
                } => {
                    walk(a, out);
                    walk(b, out);
                }
                other => out.push(leaves(other)),
            }
        }
        let mut out = Vec::new();
        walk(&tree.root, &mut out);
        out
    }

    #[test]
    fn pane_split_with_col_adds_a_row_rather_than_stacking() {
        // The whole point of routing in this layer: `pane.split.down` used to stack a tile
        // inside one cell of a four-tile row. It must now span the tab.
        let mut tree = tree_of(4);
        let third = leaves(&tree.root)[2];

        let added = apply_split(&mut tree, third, Axis::Col, Side::After, fresh())
            .expect("splitting downwards adds a row");

        let shape = rows(&tree);
        assert_eq!(shape.len(), 2, "a tab of two rows, not a stacked cell");
        assert_eq!(shape[0].len(), 4, "row one keeps all four tiles");
        assert_eq!(shape[1], vec![added], "row two holds the new pane alone");
        validate(&tree).expect("valid");
    }

    #[test]
    fn pane_split_with_row_adds_a_tile_to_the_row() {
        let mut tree = tree_of(2);
        let first = leaves(&tree.root)[0];

        apply_split(&mut tree, first, Axis::Row, Side::After, fresh()).expect("adds a tile");

        let shape = rows(&tree);
        assert_eq!(shape.len(), 1, "still one row");
        assert_eq!(shape[0].len(), 3);
    }

    #[test]
    fn pane_add_row_returns_a_session_less_pane_and_its_intent() {
        // What this layer contributes over the core: the resolved intent, and a pane with no
        // session on it — the frontend measures the slot and spawns the child at that size.
        let mut tree = tree_of(1);
        let intent = default_intent(&TabKind::ClaudeHome, Axis::Col);
        assert_eq!(
            intent,
            SplitIntent::NewClaude,
            "the console still answers a downwards split with a session"
        );
        let pane = pane_for(&intent, "cide");
        assert!(pane.session.is_none());
        assert_eq!(pane.role, PaneRole::Auxiliary);

        let id = add_row(&mut tree, None, Side::After, pane).expect("a row is added");
        assert_eq!(tree.focused, id);
        assert!(tree.panes[&id].session.is_none());
        assert_eq!(rows(&tree).len(), 2);
    }

    #[test]
    fn a_row_refuses_an_eleventh_tile_through_the_command_layer_too() {
        let mut tree = tree_of(10);
        let last = *leaves(&tree.root).last().expect("panes");

        let Err(CoreError::Invariant(msg)) =
            apply_split(&mut tree, last, Axis::Row, Side::After, fresh())
        else {
            panic!("an eleventh tile must be refused");
        };
        assert!(msg.contains("add a row instead"), "{msg}");
        // The floor that refusal protects is the one the frontend mirrors as a literal.
        assert!((MIN_TILE - 0.1).abs() < f32::EPSILON);
    }
}
