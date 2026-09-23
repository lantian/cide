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
        SplitIntent::Mirror { .. } | SplitIntent::Resume { .. } => (PaneKind::Claude, "claude"),
        // The kind follows the harness, and there is deliberately no `PaneKind` for an agent
        // (see `AgentsPanel/openRun.ts` for the six-module argument). A `claude` conversation
        // re-opened is a Claude pane in every respect — hooks, the awaiting model, `claude.fork`
        // and friends. An opencode TUI is a program in a terminal, which is what a Shell pane
        // is: no hooks, the job watch (suppressed for a fullscreen TUI), a plain restart bar.
        SplitIntent::Continue { conversation } => match conversation.harness {
            cide_ipc::Harness::Claude => (PaneKind::Claude, "claude"),
            cide_ipc::Harness::Opencode => (PaneKind::Shell, "opencode"),
            // A Qwen Code TUI is `claude`'s shape without cide's hooks: a program in a
            // terminal, so a Shell-kind pane like opencode's. (M43)
            cide_ipc::Harness::Qwen => (PaneKind::Shell, "qwen"),
            // A codex conversation re-opened is `codex resume <id>`: opencode's shape, a
            // program in a terminal. (M44)
            cide_ipc::Harness::Codex => (PaneKind::Shell, "codex"),
            // MiMo Code is an opencode fork, and its TUI is re-opened the same way. (M81)
            cide_ipc::Harness::Mimo => (PaneKind::Shell, "mimo"),
        },
        SplitIntent::Shell => (PaneKind::Shell, "bash"),
        // A `Shell` pane, deliberately — see `cide_ipc::Pane::docker` for why a container's pane
        // is not a kind of its own. The suffix is the container's own name rather than the word
        // "docker", because a tab strip with four panes all called `docker` tells you nothing.
        SplitIntent::Docker { name, .. } => (PaneKind::Shell, name.as_str()),
        // A `Shell` pane too, and for a stronger version of the same reason: the session this
        // adopts *is* a shell — a local child with a local cwd — and only its argv differs. The
        // argv is never seen here: it was spent on the `session_spawn` that produced the id.
        SplitIntent::Adopt { title, .. } => (PaneKind::Shell, title.as_str()),
    };
    Pane {
        id: PaneId::new(),
        kind,
        // Only the console's founding pane is Primary; everything a split creates can be
        // closed. Marking a second pane Primary would make two panes unclosable and leave
        // the tab with no way back to one.
        role: PaneRole::Auxiliary,
        // A mirror shares an existing session rather than waiting for one to be bound, and a
        // resume re-uses a dead one's id — a `SessionId` *is* what cide passes to
        // `claude --session-id`, so keeping it is what makes `--resume` continue the same
        // transcript and keeps every record naming that conversation correct.
        session: match intent {
            SplitIntent::Mirror { session, .. } | SplitIntent::Resume { session } => Some(*session),
            // The whole point of this variant: the pane names its session *durably*, so nothing
            // has to survive the gap between the split resolving and the pane mounting. See
            // `SplitIntent::Adopt` for the race that gap actually loses.
            SplitIntent::Adopt { session, .. } => Some(*session),
            // A `claude` conversation's id *is* a session id, and `session_spawn` keeps it
            // (`--resume` under the same uuid), so the row holds it before the spawn for the
            // reason the test below states. An opencode `ses_…` is not one, and that pane's
            // session is minted at the spawn and bound afterwards like a plain split's.
            SplitIntent::Continue { conversation } => match conversation.harness {
                cide_ipc::Harness::Claude => conversation.id.trim().parse().ok(),
                // A uuid too, but not a *claude* session: `agent_rpc` scopes Claude panes by it
                // and a qwen TUI is not one, so it is bound at the spawn like a shell's.
                cide_ipc::Harness::Opencode
                | cide_ipc::Harness::Qwen
                | cide_ipc::Harness::Codex
                | cide_ipc::Harness::Mimo => None,
            },
            _ => None,
        },
        // A mirror shows the same child as its source, so the CLI's conversation for it is
        // whatever the source already recorded; this pane learns its own from the next hook
        // frame rather than copying a value that may be a turn out of date.
        conversation: None,
        conversation_since: None,
        // What the pane keeps for later — see `Pane::continues`. A mirror of a run carries
        // the run's conversation so the pane can re-open it once the child ends; a
        // continuation *is* one.
        continues: match intent {
            SplitIntent::Mirror { continues, .. } => continues.clone(),
            SplitIntent::Continue { conversation } => Some(conversation.clone()),
            _ => None,
        },
        docker: match intent {
            SplitIntent::Docker {
                container,
                name,
                stream,
            } => Some(cide_ipc::workspace::DockerPane {
                container: container.clone(),
                name: name.clone(),
                stream: *stream,
            }),
            _ => None,
        },
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

/// Move a pane that is already in this tab to a new home in it, beside `target`.
///
/// `axis` and `side` mean what they mean for [`pane_split`] — `row` puts the pane beside
/// `target` as a tile, `col` puts it in a full-width row above or below. The pane keeps its
/// identity and its session, so the child process never notices; the frontend's host registry
/// is keyed by `PaneId` and parks the terminal's DOM rather than destroying it, which is what
/// carries the scrollback across.
///
/// Distinct from [`pane_swap`], which exchanges two panes' positions and cannot re-parent one
/// across a row boundary or collapse the row it left behind. This is the gesture behind the
/// pane's grab handle and `pane.move.*`.
#[tauri::command(rename_all = "camelCase")]
pub fn pane_move(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    target: PaneId,
    axis: Axis,
    side: Side,
) -> Result<Mutated, CoreError> {
    state
        .update(|ws| {
            let t = workspace::tab_mut(ws, project, tab)?;
            layout::move_pane(&mut t.tree, pane, target, axis, side)
        })
        .map(|()| Mutated { rev: state.rev() })
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

    #[test]
    fn an_adopted_pane_is_a_shell_that_durably_names_its_session() {
        // The whole reason this variant exists. The first cut of the compose road carried the
        // argv on the intent and parked it in `layout/spawnPlans.ts`; a plan is recorded after
        // `pane_split` resolves and the pane can render as soon as the `workspace_changed`
        // broadcast lands, which is sent *before* the command returns. A mirror that loses its
        // plan adopts a held session and looks fine. A compose pane that lost its plan fell
        // through to a login shell — reported as "compose up does nothing, only opens an empty
        // terminal", which is precisely what it was.
        let session = SessionId::new();
        let pane = pane_for(
            &SplitIntent::Adopt {
                session,
                title: "compose up : shop".to_string(),
            },
            "cide",
        );

        // A shell pane: every gesture on it behaves like a shell's, and only its argv differed.
        assert_eq!(pane.kind, PaneKind::Shell);
        // **Durable**, which is the fix: nothing has to survive the gap any more.
        assert_eq!(pane.session, Some(session));
        // The title is how a user tells this pane from the login shell it otherwise looks like.
        assert_eq!(pane.title, "cide : compose up : shop");
        assert!(pane.docker.is_none());
        // And no conversation: a compose run is a child with an argv, not a harness conversation,
        // so there is nothing for the exit bar to re-open. See `SplitIntent::Continue`.
        assert!(pane.continues.is_none());

        /*
         * And the argv is nowhere, because it was spent on the spawn that produced the id. A
         * restored pane re-running `docker compose up` at launch — against a file that may have
         * changed, on a machine just unlocked — is the outcome worth designing against.
         *
         * # Structurally, and never by grepping the JSON for short tokens
         *
         * The first cut of this test greped for `["docker-compose", "compose.yaml", "/srv/shop",
         * "-d"]`, and **`-d` matches a uuid**: a hyphen followed by the hex digit `d` occurs in a
         * `Pane`'s two ids about **40% of the time** (measured). So it failed roughly two runs in
         * five, passed alone often enough to look fine, and accused a line of production code that
         * was correct.
         *
         * The key set says the real thing anyway, and it is worth being exact about which half of
         * it the compiler already covers. Adding a field to `Pane` is a **compile error** at every
         * construction site (`Pane` has no `Default`), so a smuggled field cannot arrive quietly.
         * What the compiler cannot see is the **wire**: a `#[serde(rename)]` on an existing field
         * changes what `workspace.json` carries with nothing failing to build — verified by doing
         * it, which is the only way to know an assertion like this is live.
         */
        let stored = serde_json::to_value(&pane).expect("a pane serialises");
        let mut keys: Vec<&str> = stored
            .as_object()
            .expect("a pane is a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "continues",
                "conversation",
                "conversationSince",
                "docker",
                "id",
                "kind",
                "role",
                "session",
                "title",
            ],
            "a new durable field on `Pane` needs a decision about whether a compose run may write \
             to it — see `SplitIntent::Adopt`",
        );

        // And the distinctive halves of the argv, which cannot collide with a uuid: every one of
        // them holds a character no hex digit or hyphen can produce.
        let text = stored.to_string();
        for token in ["docker-compose", "compose.yaml", "/srv/shop"] {
            assert!(
                !text.contains(token),
                "`{token}` reached workspace.json: {text}"
            );
        }
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
            continues: None,
            title: "cide : claude".into(),
            docker: None,
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

    /// A resumed or mirrored pane carries its session **at creation**, before any child exists.
    ///
    /// The arm above it does not, and the difference is load-bearing one module over.
    /// `agent_rpc::project_of_claude_pane` answers *which project's tracker this connection may
    /// write* by finding the pane holding the session, and a connection's scope is resolved once
    /// and fixed for its life (`initialize` advertises `listChanged: false`). For a pane whose
    /// session is bound *after* the spawn — `NewClaude` — that is a race the child has to lose,
    /// and it does by a wide margin: the binding is one IPC round trip while the child has a
    /// whole CLI to boot before it forks `cide-hook mcp`. For a task's conversation there is no
    /// race at all, because `TasksPanel/openSession.ts` splits with `SplitIntent::Resume` and
    /// this arm writes the session into the row synchronously.
    ///
    /// So: moving `Resume` or `Mirror` into the `_ => None` arm would compile, would look like a
    /// simplification, and would turn every task-conversation pane back into a `claude` served
    /// an empty `tools/list` whenever it won the race.
    #[test]
    fn a_resumed_pane_holds_its_session_before_anything_is_spawned() {
        let session = cide_ipc::SessionId::new();

        let resumed = pane_for(&SplitIntent::Resume { session }, "cide");
        assert_eq!(resumed.session, Some(session));
        assert_eq!(resumed.kind, PaneKind::Claude);

        let mirrored = pane_for(
            &SplitIntent::Mirror {
                session,
                continues: None,
            },
            "cide",
        );
        assert_eq!(mirrored.session, Some(session));
        assert_eq!(mirrored.kind, PaneKind::Claude);
        assert!(mirrored.continues.is_none());

        // And the pane a plain split makes is still session-less, which is the case above.
        assert!(pane_for(&SplitIntent::NewClaude, "cide").session.is_none());
    }

    /// A continued conversation is a pane of its harness's kind, holding what it needs to
    /// re-open the conversation later — and, for `claude`, its session before the spawn, for
    /// the reason the test above states. (M42)
    #[test]
    fn a_continued_conversation_is_a_pane_of_its_harness_kind() {
        use cide_ipc::{Harness, HarnessSession};

        let session = cide_ipc::SessionId::new();
        let claude = HarnessSession {
            harness: Harness::Claude,
            id: session.to_string(),
            cwd: std::path::PathBuf::from("/work/p/.cide/worktrees/developer-t-1"),
        };
        let pane = pane_for(
            &SplitIntent::Continue {
                conversation: claude.clone(),
            },
            "cide",
        );
        assert_eq!(pane.kind, PaneKind::Claude);
        assert_eq!(
            pane.session,
            Some(session),
            "the id is the session, held before the spawn"
        );
        assert_eq!(pane.continues.as_ref(), Some(&claude));
        assert_eq!(pane.title, "cide : claude");

        let opencode = HarnessSession {
            harness: Harness::Opencode,
            id: "ses_0123abc".into(),
            cwd: std::path::PathBuf::from("/work/p"),
        };
        let pane = pane_for(
            &SplitIntent::Continue {
                conversation: opencode.clone(),
            },
            "cide",
        );
        assert_eq!(
            pane.kind,
            PaneKind::Shell,
            "an opencode TUI is a program in a terminal"
        );
        assert!(
            pane.session.is_none(),
            "an opencode id is not a session id; the spawn mints one"
        );
        assert_eq!(pane.continues.as_ref(), Some(&opencode));
        assert_eq!(pane.title, "cide : opencode");

        // A mirror of a run keeps the run's conversation for the same later.
        let mirrored = pane_for(
            &SplitIntent::Mirror {
                session,
                continues: Some(claude.clone()),
            },
            "cide",
        );
        assert_eq!(mirrored.continues.as_ref(), Some(&claude));
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
