//! Operations on the workspace tree: projects, windows, tabs.
//!
//! The data lives in `cide_ipc::workspace`; this module is the only place allowed to
//! mutate it, because it is the only place that knows the invariants:
//!
//! * `tabs[0]` is the pinned Claude console, always present and never movable. The rule is
//!   enforced by index rather than by [`TabKind::closable`] alone, so no frontend bug can
//!   lose a project's console.
//! * A [`TabKind::File`] tab whose `dirty` flag is set does not close. [`close_tab`] and
//!   [`close_project`] take a `force` flag and refuse without it, for the same reason the
//!   console is pinned here rather than in the frontend: a dialog is a courtesy, and a
//!   courtesy is not a guard.
//! * A project always has at least one root, and `roots[0]` supplies `display_path`.
//! * `active_tab` always names a tab that exists.
//! * `ws.windows` is a *derived* view: it is nothing but a mapping from projects onto
//!   window roles, recomputed from `settings.window_mode`. Flipping the mode therefore
//!   touches no project, tab, pane or session — see [`set_window_mode`].
//!
//! Every successful mutation advances `ws.rev` exactly once, so an event carrying a
//! revision is enough for the frontend to order or discard a snapshot.
//!
//! Pane-tree surgery is not here: this module hands trees to [`crate::layout`] and never
//! walks a [`LayoutNode`] itself.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cide_ipc::{
    Axis, DiffOrigin, DiffSpec, Pane, PaneId, PaneKind, PaneRole, Project, ProjectId, ProjectRoot,
    SessionId, SettingsSection, Side, Tab, TabId, TabKind, ToolWindowState, UnsavedTab,
    WindowLabel, WindowMode, WindowRole, Workspace,
};
use indexmap::IndexMap;

use crate::error::{CoreError, Result};
use crate::layout;

/// Header-dot colours, handed out by [`next_dot`] so that no two open projects share one
/// until the palette runs out.
///
/// CSS custom properties rather than literals, so the dots follow the theme.
const DOT_PALETTE: [&str; 6] = [
    "var(--accent)",
    "var(--blue)",
    "var(--green)",
    "var(--purple)",
    "var(--yellow)",
    "var(--cyan)",
];

/// Advance the revision counter and return the new value.
///
/// Callers that perform their own compound mutation call this once at the end; every
/// mutator in this module already does it for them.
pub fn bump(ws: &mut Workspace) -> u64 {
    ws.rev += 1;
    ws.rev
}

/// Make `tab` the project's active one **and** put it at the head of its focus order.
///
/// The only writer of [`Project::active_tab`] in this crate, and that is the point rather than
/// tidiness: the two fields are one fact written twice, and any assignment that moved one
/// without the other would leave `close_tab` choosing a successor from a history that had
/// stopped recording. `grep -n "active_tab = " crates/` should find this function and nothing
/// else outside test fixtures.
///
/// Deliberately *not* a `pub` mutator and deliberately not bumping `rev`: it is a fragment of
/// four larger mutations ([`open_project`] via its literal, [`open_tab`], [`close_tab`],
/// [`activate_tab`]), each of which bumps once for the whole of what it did.
///
/// The tab is not checked for membership in `p.tabs`. Every caller has already resolved it —
/// `open_tab` just inserted it, the other two looked it up — and a second lookup here would be a
/// second answer to a question already asked, which is how [`close_tab`] and the close dialog
/// once came to disagree about "unsaved".
fn set_active(p: &mut Project, tab: TabId) {
    p.active_tab = tab;
    p.tab_mru.retain(|id| *id != tab);
    p.tab_mru.insert(0, tab);
}

/// Open a project over `roots`, with its pinned console tab already populated.
///
/// The console is created before anything else and holds a single `Primary` Claude pane
/// bound to the project's `primary_session`, which is the id later passed to
/// `claude --session-id`.
///
/// Returns [`CoreError::NoRoots`] when `roots` is empty: a rootless project has no cwd to
/// spawn that session in.
pub fn open_project(
    ws: &mut Workspace,
    roots: Vec<PathBuf>,
    name: Option<String>,
) -> Result<ProjectId> {
    let Some(primary_root) = roots.first() else {
        return Err(CoreError::NoRoots);
    };

    // Opening a path that is already open activates it instead of opening it twice.
    //
    // Two projects over one directory are not a state a user ever wants: they share a
    // working tree, a git repository and a file index, and the header shows the same name
    // twice with no way to tell which is which. It is also trivially reachable — the `+`
    // button, a repeated drag, a script — and in `PerProject` mode each duplicate is another
    // OS window. A development workspace here reached 242 copies of one directory this way,
    // and the next launch tried to open a window for every one of them.
    if let Some(existing) = ws
        .projects
        .iter()
        .find(|(_, p)| p.roots.first().is_some_and(|r| r.path == *primary_root))
        .map(|(id, _)| *id)
    {
        activate_project(ws, existing);
        return Ok(existing);
    }

    let name = name.unwrap_or_else(|| basename(primary_root));
    let display_path = display_path_of(primary_root);
    let primary_session = SessionId::new();

    let console = Tab {
        id: TabId::new(),
        kind: TabKind::ClaudeHome,
        tree: layout::new_tree(Pane {
            id: PaneId::new(),
            kind: PaneKind::Claude,
            role: PaneRole::Primary,
            session: Some(primary_session),
            conversation: None,
            conversation_since: None,
            title: format!("{name} : claude"),
        }),
    };
    let active_tab = console.id;

    let id = ProjectId::new();
    let dot = next_dot(ws);
    ws.projects.insert(
        id,
        Project {
            id,
            name,
            display_path,
            dot,
            roots: roots.into_iter().map(project_root).collect(),
            tabs: vec![console],
            active_tab,
            // The literal rather than `set_active`, because the struct does not exist yet to
            // pass one a `&mut` to. Same two writes, and `validate` checks the result.
            tab_mru: vec![active_tab],
            detached: IndexMap::new(),
            dock_anchors: IndexMap::new(),
            // Closed, at the default height, with no history tabs. A new project has nothing
            // to show in a git log yet and opening one uninvited would resize every pane in
            // the window on the gesture that was supposed to open a project.
            tool_window: ToolWindowState::default(),
            primary_session,
        },
    );

    rebuild_windows(ws);
    bump(ws);
    Ok(id)
}

/// Bring an already-open project to the front of its window.
///
/// Separate from [`open_project`] because "open this path" and "show me this project" are
/// the same gesture from the user's side and must not be two different outcomes.
///
/// Bumps only when some window's `active` actually moves, like [`activate_tab`]: clicking
/// the header tab that is already active is a no-op, and reporting it as a change would
/// cost every window a broadcast and a re-render for news that isn't.
pub fn activate_project(ws: &mut Workspace, project: ProjectId) {
    let mut changed = false;
    for role in ws.windows.values_mut() {
        if let WindowRole::Shell { projects, active } = role
            && projects.contains(&project)
            && *active != Some(project)
        {
            *active = Some(project);
            changed = true;
        }
    }
    if changed {
        bump(ws);
    }
}

/// Close a project and every window that existed only to show part of it.
///
/// Sessions are owned by the core, not by the project record, so nothing here kills a
/// child process; the caller decides that separately.
///
/// `force` is the opt-in that discards unsaved edits. Without it, a project holding a dirty
/// file tab is refused with [`CoreError::UnsavedChanges`] naming every such tab: closing a
/// project closes its tabs, so the same data loss is reachable here as through
/// [`close_tab`], and guarding only the narrower gesture would leave the wider one open.
pub fn close_project(ws: &mut Workspace, id: ProjectId, force: bool) -> Result<()> {
    if !ws.projects.contains_key(&id) {
        return Err(CoreError::NoSuchProject(id));
    }
    // Checked before the removal, not after: once the project is out of the map its tabs are
    // gone with it and there is nothing left to name in the refusal.
    if !force {
        let tabs = unsaved_tabs(ws, Some(id));
        if !tabs.is_empty() {
            return Err(CoreError::UnsavedChanges { tabs });
        }
    }

    // `shift_remove`, not `swap_remove`: insertion order is the header tab order.
    if ws.projects.shift_remove(&id).is_none() {
        return Err(CoreError::NoSuchProject(id));
    }

    // A detached pane or tab outlives its window, but not its project — there would be
    // nowhere left for it to re-dock.
    ws.windows.retain(|_, role| match role {
        WindowRole::Shell { .. } => true,
        WindowRole::DetachedPane { project, .. } | WindowRole::DetachedTab { project, .. } => {
            *project != id
        }
    });

    rebuild_windows(ws);
    bump(ws);
    Ok(())
}

/// Move a project within the header tab strip.
pub fn reorder_project(ws: &mut Workspace, from: usize, to: usize) -> Result<()> {
    let len = ws.projects.len();
    check_index(from, len)?;
    check_index(to, len)?;
    if from == to {
        return Ok(());
    }

    ws.projects.move_index(from, to);
    // The stacked shell window lists projects in header order, so it moves with them.
    rebuild_windows(ws);
    bump(ws);
    Ok(())
}

/// Add a root directory to a project.
///
/// Adding a path the project already holds is a no-op rather than an error: the gesture
/// that produces it (dropping a folder onto the tree) can legitimately repeat.
pub fn add_root(ws: &mut Workspace, project: ProjectId, path: PathBuf) -> Result<()> {
    let p = project_mut(ws, project)?;
    if p.roots.iter().any(|r| r.path == path) {
        return Ok(());
    }

    p.roots.push(project_root(path));
    bump(ws);
    Ok(())
}

/// Remove a root directory from a project.
///
/// Returns [`CoreError::NoRoots`] rather than emptying the project. Removing a path the
/// project does not hold is a no-op, mirroring [`add_root`].
pub fn remove_root(ws: &mut Workspace, project: ProjectId, path: &Path) -> Result<()> {
    let p = project_mut(ws, project)?;
    let Some(index) = p.roots.iter().position(|r| r.path == path) else {
        return Ok(());
    };
    if p.roots.len() == 1 {
        return Err(CoreError::NoRoots);
    }

    p.roots.remove(index);
    if index == 0 {
        // `display_path` is a projection of the primary root and has just gone stale.
        let derived = display_path_of(&p.roots[0].path);
        p.display_path = derived;
    }
    bump(ws);
    Ok(())
}

/// Open a tab holding a single pane, and make it active.
///
/// [`TabKind::ClaudeHome`] is refused with [`CoreError::TabPinned`]: a second console
/// would report itself unclosable through [`TabKind::closable`] while sitting outside the
/// index-0 guard, leaving a tab that nothing could ever remove.
///
/// # A new tab lands immediately right of the pinned console, not at the end
///
/// It used to `push`. The strip therefore grew rightwards for ever, and on a real session the
/// tab a user had *just* opened was the one furthest from the eye, off the end of a strip that
/// had already started eliding titles — so the answer to "where did the file I just opened go"
/// was "scroll". Opening at index 1 puts the newest thing next to the console, which is where
/// the user is already looking, and makes the strip read newest-first from a fixed anchor.
///
/// The rule is deliberately **every** kind, not just [`TabKind::File`]. A per-kind rule would
/// mean a file opened from the tree and a diff opened from the git panel land in different
/// places from the same user's point of view — the strip's order would stop being a fact about
/// when things were opened and become a fact about which gesture opened them, which nobody can
/// read off the screen. [`reinsert_tab`] already clamps into `1..=len` for the same reason: 0 is
/// the console's, and the console's alone.
///
/// The index is `min`'d against the length rather than written as a literal `1`. `Vec::insert`
/// **panics** past the end, `validate` is what guarantees a console at 0, and this crate is
/// linked into a binary built with `panic = "abort"` — so a workspace that had somehow lost its
/// console would take the process down here rather than be refused by the validator one line
/// later.
pub fn open_tab(
    ws: &mut Workspace,
    project: ProjectId,
    kind: TabKind,
    first_pane: Pane,
) -> Result<TabId> {
    // A pane id is the address every later command uses — focus, close, detach, attach a
    // session. Two panes sharing one would make both unaddressable, and `validate` rejects
    // the resulting workspace, so refuse before building a state we would then call corrupt.
    if let Some((existing_project, existing_tab)) = find_pane(ws, first_pane.id) {
        return Err(CoreError::Invariant(format!(
            "pane {} is already live in project {existing_project} tab {existing_tab}",
            first_pane.id
        )));
    }

    // The project is resolved first so that an unknown id is reported as such whatever the
    // kind, rather than the caller being told about a pinning rule it never reached.
    let p = project_mut(ws, project)?;
    if matches!(kind, TabKind::ClaudeHome) {
        return Err(CoreError::TabPinned);
    }

    let id = TabId::new();
    let at = 1.min(p.tabs.len());
    p.tabs.insert(
        at,
        Tab {
            id,
            kind,
            tree: layout::new_tree(first_pane),
        },
    );
    set_active(p, id);
    bump(ws);
    Ok(id)
}

/// Put a whole tab back: its kind, its pane tree, and its position in the strip.
///
/// [`open_tab`]'s counterpart for Ctrl+Shift+T. It is a separate function rather than an
/// `Option<PaneTree>` parameter on `open_tab` because the two differ in every interesting way:
/// `open_tab` mints one pane and puts the tab at the head of the strip, this one accepts a tree
/// it did not build and restores the position the tab *had* — and the *checks* it therefore has
/// to run are the whole of its body.
///
/// The two clamp the same way and for the same reason (`1..=len`; 0 is the pinned console's),
/// which is the one thing they do share; see [`open_tab`] for why a `Vec::insert` past the end
/// would be an abort rather than a refusal.
///
/// # The pane ids are the ones the tab had, and that is on purpose
///
/// `close_tab` removes the tab from `p.tabs`, so its pane ids stop being live and are free to
/// use again — and the frontend's `paneHosts` map is keyed by pane id and is **not** cleared by
/// a tab close (only `close_pane` calls `destroyHost`). So a `ClaudeFull` tab reopened this way
/// re-adopts its own parked terminal, with its scrollback and its live session, instead of
/// mounting a fresh one and replaying a mirror. Reminting would have cost that for no gain: a
/// `PaneId` is a UUID, so a preserved one cannot collide with a pane minted meanwhile, and the
/// check below refuses the case anyway rather than trusting the argument.
///
/// The **tab** id is fresh. Nothing outside holds the old one — `close_tab` has already pruned
/// the detached windows anchored to it — and a `WindowRole` naming a tab id that came back
/// would re-dock a pane into a tab it never left.
///
/// `index` is clamped into `1..=tabs.len()`: 0 is the pinned console's and a record made when
/// the strip was longer must not be refused for naming a position past its end.
pub fn reinsert_tab(
    ws: &mut Workspace,
    project: ProjectId,
    index: usize,
    kind: TabKind,
    tree: cide_ipc::PaneTree,
) -> Result<TabId> {
    // Tree first, and against the *whole* workspace: a pane id is the address every later
    // command uses, and `validate` refuses a workspace in which one appears twice — so a
    // reopen that collided would be rolled back by `WorkspaceState::update` with nothing but a
    // log line to say why. Same refusal, and same wording, as `open_tab`'s.
    layout::validate(&tree)?;
    for pane in tree.panes.keys() {
        if let Some((existing_project, existing_tab)) = find_pane(ws, *pane) {
            return Err(CoreError::Invariant(format!(
                "pane {pane} is already live in project {existing_project} tab {existing_tab}",
            )));
        }
        if ws.projects.values().any(|p| p.detached.contains_key(pane)) {
            return Err(CoreError::Invariant(format!(
                "pane {pane} is detached into its own window",
            )));
        }
    }

    let p = project_mut(ws, project)?;
    if matches!(kind, TabKind::ClaudeHome) {
        return Err(CoreError::TabPinned);
    }

    let id = TabId::new();
    let at = index.clamp(1, p.tabs.len());
    p.tabs.insert(at, Tab { id, kind, tree });
    set_active(p, id);
    bump(ws);
    Ok(id)
}

/// Every file tab with unsaved edits, in header order then tab order.
///
/// `only` narrows to one project; `None` answers for the whole workspace, which is what the
/// quit path asks. Both callers want the same list in the same shape, and a second traversal
/// that agreed with this one by inspection is a second traversal that stops agreeing.
///
/// A tab is unsaved when it is a [`TabKind::File`] whose `dirty` flag is set. Nothing else
/// in the tree can hold unwritten work: a diff tab's edits are answered rather than saved,
/// and a terminal has no buffer.
pub fn unsaved_tabs(ws: &Workspace, only: Option<ProjectId>) -> Vec<UnsavedTab> {
    let mut out = Vec::new();
    for (id, p) in &ws.projects {
        if only.is_some_and(|wanted| wanted != *id) {
            continue;
        }
        for t in &p.tabs {
            if let Some(unsaved) = describe_unsaved(*id, p, t) {
                out.push(unsaved);
            }
        }
    }
    out
}

/// One tab's unsaved state, or `None` when it holds nothing to lose.
///
/// Errors rather than answering `None` for an id that does not exist, so a caller cannot
/// read "no such tab" as "nothing at risk" — that mistake fails open, which is the one
/// direction this guard must never fail.
pub fn unsaved_in_tab(
    ws: &Workspace,
    project: ProjectId,
    tab: TabId,
) -> Result<Option<UnsavedTab>> {
    let p = self::project(ws, project)?;
    let t = p
        .tabs
        .iter()
        .find(|t| t.id == tab)
        .ok_or(CoreError::NoSuchTab(tab))?;
    Ok(describe_unsaved(project, p, t))
}

/// Close a tab, activating the tab to its left if it was the active one, and dropping any
/// window detached from it.
///
/// Two refusals, and they are refusals rather than frontend courtesies for the same reason:
///
/// * `tabs[0]` is the pinned console — [`CoreError::TabPinned`].
/// * a [`TabKind::File`] tab with `dirty` set, unless `force` — [`CoreError::UnsavedChanges`],
///   carrying the tab so the caller can name the file it is about to lose.
///
/// `force` is a *bool* rather than a second function (`close_tab_discarding`) because the
/// call sites are the same call sites: the frontend calls this, is refused, shows the
/// dialog, and calls it again with the user's answer. Two functions would put the choice of
/// which to call in the hands of whoever wrote the call site, and the wrong choice there is
/// silent — a bool is impossible to pass by accident and greps in one line.
/// Close one pane, refusing to discard an editor's unsaved buffer.
///
/// The gap this closes: `layout::close` is a tree operation and knows nothing about
/// documents, so closing an editor pane directly used to drop the buffer with no guard at
/// all — while `close_tab` right below refused the same loss. A user could not close the
/// tab, but could close the pane inside it, and lose exactly as much.
///
/// Only an `Editor` pane can hold unsaved text. A `File` tab has exactly one (`PaneBody`
/// dispatches on the pane kind for this reason), so closing it *is* discarding the buffer
/// even though the tab survives.
pub fn close_pane(
    ws: &mut Workspace,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    force: bool,
) -> Result<()> {
    // Immutable borrow first, like `close_tab`: a refusal must leave `rev` untouched.
    if !force {
        let t = self::tab(ws, project, tab)?;
        let is_editor = t
            .tree
            .panes
            .get(&pane)
            .is_some_and(|p| p.kind == PaneKind::Editor);
        if is_editor && let Some(unsaved) = unsaved_in_tab(ws, project, tab)? {
            return Err(CoreError::UnsavedChanges {
                tabs: vec![unsaved],
            });
        }
    }

    let t = tab_mut(ws, project, tab)?;
    crate::layout::close(&mut t.tree, pane)?;
    bump(ws);
    Ok(())
}

pub fn close_tab(ws: &mut Workspace, project: ProjectId, tab: TabId, force: bool) -> Result<()> {
    // Both checks run against an immutable borrow and precede every mutation, so a refusal
    // leaves the workspace exactly as it was — including `rev`, which a caller uses to
    // decide whether a snapshot it holds is stale.
    //
    // The pinned check is first because the console is never a file tab, so reporting
    // `UnsavedChanges` for it could not happen today; ordering it deliberately means the
    // answer does not depend on that staying true.
    let index = index_of_tab(self::project(ws, project)?, tab)?;
    if index == 0 {
        return Err(CoreError::TabPinned);
    }
    // Through the shared query rather than reading `tab.kind` here: `unsaved_in_tab` is what
    // the rest of the app asks, and a guard that decides "unsaved" its own way is a guard
    // that will one day disagree with the dialog it triggers.
    if !force && let Some(unsaved) = unsaved_in_tab(ws, project, tab)? {
        return Err(CoreError::UnsavedChanges {
            tabs: vec![unsaved],
        });
    }

    // Which tabs are torn out into windows of their own, read before the mutable borrow:
    // the successor below must not hand the shell a tab it is deliberately not drawing.
    // The mru arm is screened by construction — `activate_tab` refuses to admit a detached
    // tab to the order — but the positional fallback walks the strip itself.
    let torn: Vec<TabId> = ws
        .windows
        .values()
        .filter_map(|role| match role {
            WindowRole::DetachedTab { tab, .. } => Some(*tab),
            _ => None,
        })
        .collect();

    let p = project_mut(ws, project)?;
    p.tabs.remove(index);
    // Out of the focus order whether or not it was the active tab, and *before* the successor
    // is chosen. Dropping it only in the active branch is the subtle version of this bug:
    // closing an inactive tab would leave its id in `tab_mru`, and the next close would hand
    // the user a tab that no longer exists — a ghost with an `index - 1` shape, one gesture
    // removed from the gesture that caused it.
    p.tab_mru.retain(|id| *id != tab);
    if p.active_tab == tab {
        // # Which tab the user lands on
        //
        // The **most recently used survivor**, asked for by name. The old rule was the tab to
        // the *left*, which is strip position — and strip position is insertion order, so
        // closing a file opened an hour ago dropped the user next to whatever happened to have
        // been opened just before it, rather than back where they came from.
        //
        // `tab_mru[0]` is the tab being closed (it was active), so the successor is the first
        // entry after the `retain` above. It is checked against `p.tabs` anyway rather than
        // trusted: `validate` guarantees the order holds only live ids, but this runs on a
        // workspace read from disk that may predate that guarantee, and activating a tab that
        // does not exist is the one outcome worse than landing in the wrong place.
        //
        // # The fallback is the old rule, and it is reachable
        //
        // A `workspace.json` written before `tab_mru` existed loads with an empty order, and a
        // project whose only activation was its own creation has a one-entry one. Both leave
        // nothing here, and both then get the left neighbour — which always exists, because
        // `index` is at least 1 and `tabs[0]` is the console, which cannot be closed. So the
        // successor is never absent and is never the tab just removed.
        // Both arms skip tabs that are torn out into their own windows: activating one from
        // here would draw it in two windows at once. The positional arm walks left from the
        // closed tab's slot rather than taking `index - 1` blindly — `tabs[0]` is the
        // console, which can never detach, so the walk always lands somewhere.
        let successor = p
            .tab_mru
            .iter()
            .copied()
            .find(|id| !torn.contains(id) && p.tabs.iter().any(|t| t.id == *id))
            .or_else(|| {
                p.tabs[..index]
                    .iter()
                    .rev()
                    .find(|t| !torn.contains(&t.id))
                    .map(|t| t.id)
            });
        if let Some(next) = successor {
            set_active(p, next);
        }
    }

    // A detached pane or tab outlives its window, but not the tab it re-docks into — the
    // same reason [`close_project`] prunes the detached windows of a project going away.
    ws.windows.retain(|_, role| match role {
        WindowRole::Shell { .. } => true,
        WindowRole::DetachedPane { tab: home, .. } | WindowRole::DetachedTab { tab: home, .. } => {
            *home != tab
        }
    });

    bump(ws);
    Ok(())
}

/// Make `tab` the project's active tab, promoting it to the head of the focus order.
///
/// `changed` asks about the *order* as well as the active id, and not for symmetry: a
/// workspace restored from a build without [`Project::tab_mru`] has an active tab that is not
/// at the head of an order that is empty, and re-activating it is the first chance to repair
/// that. Without the second clause the repair is skipped precisely for the tab the user is
/// looking at, and a `close_tab` on it would take the left-neighbour fallback for ever.
/// `persist::load` repairs on the way in as well; this is the belt to that pair of braces, and
/// it settles after one activation because `set_active` puts the tab at the head.
pub fn activate_tab(ws: &mut Workspace, project: ProjectId, tab: TabId) -> Result<()> {
    // A tab torn out into a window of its own is already on screen — there, not here.
    // Making it the shell's active tab would draw the same tab in two windows at once,
    // which for a file tab is two live editors over one buffer, the exact failure the
    // per-tab buffer registry exists to prevent (see `ui/src/editor/openBuffers.ts`).
    //
    // Answered with success rather than refused, deliberately: the callers that reach this
    // are reveal flows — a mention landing in a pane, `tab_open_file` finding the path
    // already open — and each goes on to raise the tab's own window, which is the honest
    // rendering of "activate". A refusal here would surface an error toast in the middle of
    // a gesture that is about to succeed.
    if detached_tab_window(ws, tab).is_some() {
        index_of_tab(self::project(ws, project)?, tab)?;
        return Ok(());
    }
    let p = project_mut(ws, project)?;
    index_of_tab(p, tab)?;

    let changed = p.active_tab != tab || p.tab_mru.first() != Some(&tab);
    set_active(p, tab);
    if changed {
        bump(ws);
    }
    Ok(())
}

/// Which scratch slot a diff tab competes for. (M18)
///
/// # Why there are two and not one
///
/// The preview slot exists so that clicking down a thirty-file changelist produces one tab
/// rather than thirty — see [`retarget_diff`]. Until M18 one slot was the whole story, because
/// there was one surface that could ask for a diff.
///
/// There are now two, and they are on screen **at the same time**: the git panel's changes
/// tree in the sidebar, and the log's changed-file list in the tool window. With a single slot
/// the second surface eats the first one's tab, so a click in the log throws away the working
/// diff the user was staging from — the mirror image of the thirty-tab report the preview slot
/// was written to fix, and worse, because what is lost is a tab holding a half-made selection
/// rather than a tab that should never have existed.
///
/// Two slots, one per family. A working-tree diff and a revision diff are different documents
/// with different affordances (three side buttons and a Stage footer against a read-only pair),
/// so retargeting one at the other is refused outright; see [`retarget_diff`]'s fourth refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewSlot {
    /// [`DiffOrigin::Git`] — the working tree, the index, and HEAD. The git panel's slot.
    Working,
    /// [`DiffOrigin::GitRevision`] — two frozen revisions. The tool window's slot.
    Revision,
}

/// Which slot an origin belongs to, or `None` for an origin that has no scratch slot at all.
///
/// **Derived, never stored.** A `preview_slot` field on the tab would be a second spelling of a
/// fact the origin already carries, and the two would disagree the first time a tab was
/// retargeted across families — which is exactly the case [`retarget_diff`] now refuses, so the
/// field would be a stored copy of something that cannot legally change. `tab_mru` is the
/// precedent for adding state to `Project` when it is genuinely new information; this is the
/// opposite case.
///
/// [`DiffOrigin::ClaudeMcp`] is `None` rather than a third variant: that tab is holding an agent
/// turn open, it is never a preview (nothing opens it as a scratch tab and
/// [`retarget_diff`] refuses to re-point it), and giving it a slot would invite a future caller
/// to treat it as reusable.
pub fn slot_of(origin: &DiffOrigin) -> Option<PreviewSlot> {
    match origin {
        DiffOrigin::Git { .. } => Some(PreviewSlot::Working),
        DiffOrigin::GitRevision { .. } => Some(PreviewSlot::Revision),
        DiffOrigin::ClaudeMcp { .. } => None,
    }
}

/// The project's preview diff tab **for one slot**, if it has one.
///
/// At most one exists per project per slot by construction: [`retarget_diff`] is the only thing
/// that ever sets the flag, it reuses the tab this finds, and it refuses to move a tab between
/// slots. `find_map` over the strip in order anyway rather than `debug_assert`ing uniqueness — a
/// `workspace.json` hand-edited or written by a future version is not a reason to panic, and
/// taking the leftmost is a defined answer.
///
/// The `slot` argument is not defaultable and deliberately has no "any" value. A caller that
/// does not know which family it is opening is a caller that will steal the other one's tab,
/// which is the whole failure [`PreviewSlot`] exists to prevent; making it say so is what stops
/// the single-slot behaviour coming back by omission.
pub fn preview_diff_tab(
    ws: &Workspace,
    project: ProjectId,
    slot: PreviewSlot,
) -> Result<Option<TabId>> {
    Ok(self::project(ws, project)?
        .tabs
        .iter()
        .find_map(|t| match &t.kind {
            TabKind::Diff {
                spec,
                preview: true,
            } if slot_of(&spec.origin) == Some(slot) => Some(t.id),
            _ => None,
        }))
}

/// Point an existing diff tab at a different diff — the mutation behind "change current diff
/// to selected file".
///
/// # Why this is not `close_tab` + `open_tab`
///
/// Closing and re-opening produces a *new* [`TabId`] at the end of the strip, so the row the
/// user is clicking down walks rightwards under their pointer, any pane detached from that
/// tab is dropped by [`close_tab`]'s window prune, and the split the user made inside it is
/// gone. Retargeting is one field: the tab, its position, its id and its pane tree all stay,
/// and only what the panes are *about* changes.
///
/// # Four refusals
///
/// * Not a diff tab — [`CoreError::Invariant`]. There is no sensible reading of "retarget a
///   terminal".
/// * A [`DiffOrigin::ClaudeMcp`] tab — [`CoreError::Invariant`]. That tab is the visible half
///   of a blocked agent turn: `cide-ide-mcp`'s broker is holding a future that resolves when
///   the user answers *this* diff, and re-pointing it at a git file would leave the CLI
///   waiting on a question that is no longer on screen.
/// * **A retarget across families** — [`CoreError::Invariant`]. A [`DiffOrigin::Git`] tab and a
///   [`DiffOrigin::GitRevision`] tab are not two settings of one pane: the first draws three
///   side buttons, tick boxes and a Stage/Unstage/Commit footer, the second is read-only,
///   because there is nothing in the index to stage two 2019 commits into. Re-pointing one at
///   the other would put a Stage button over a diff of two commits — on the one surface in this
///   application where a wrong click writes to the index — and the frontend would have to
///   notice the origin changed under a mounted pane and rebuild itself around a different set
///   of controls. Refusing here means each family keeps its own scratch tab
///   ([`PreviewSlot`]) and neither can ever be handed the other's. Asked through [`slot_of`],
///   so "which family" is derived from the origin at both ends and cannot be stored wrongly;
///   an origin with no slot at all — `ClaudeMcp`, already refused above — differs from every
///   slot and is refused here too, which is the safe direction for a variant added later.
/// * A tab holding unsaved work — [`CoreError::UnsavedChanges`]. Asked through
///   [`unsaved_in_tab`], the same query [`close_tab`] uses, and for the same reason: a guard
///   that decides "unsaved" its own way is a guard that will one day disagree with the dialog
///   it triggers. Today it can never fire — only a [`TabKind::File`] can be dirty and the git
///   diff pane is a viewer with staging ticks, not an editor — but retargeting *is* a
///   discard, so it asks rather than assuming. (The ticks themselves are not workspace state:
///   they live in the pane and in `partialStore.ts`, keyed by repo and path, so they are
///   re-found rather than lost when the tab comes back to that file.)
///
/// Renaming the panes is part of the operation, not a courtesy. A diff pane's `title` is
/// what the pane header draws, and it was written at open time from the old spec; leaving it
/// would put "old.rs — diff" above the diff of `new.rs`. Only [`PaneKind::Diff`] panes are
/// touched, so a split holding something else keeps its own label.
pub fn retarget_diff(
    ws: &mut Workspace,
    project: ProjectId,
    tab: TabId,
    spec: DiffSpec,
) -> Result<()> {
    // Every check runs before the first mutation, so a refusal leaves `rev` untouched and a
    // snapshot the caller holds stays valid.
    let existing = self::tab(ws, project, tab)?;
    let TabKind::Diff { spec: current, .. } = &existing.kind else {
        return Err(CoreError::Invariant(format!(
            "tab {tab} is not a diff tab and cannot be retargeted"
        )));
    };
    if matches!(current.origin, DiffOrigin::ClaudeMcp { .. }) {
        return Err(CoreError::Invariant(format!(
            "tab {tab} answers a Claude diff request and cannot be retargeted"
        )));
    }
    // The family check. See the fourth refusal above: a working-tree diff and a revision diff
    // are different documents with different controls, and the slot arithmetic in
    // [`preview_diff_tab`] only holds because no tab ever crosses.
    if slot_of(&current.origin) != slot_of(&spec.origin) {
        return Err(CoreError::Invariant(format!(
            "tab {tab} shows a diff of a different kind and cannot be retargeted at this one — \
             a working-tree diff can be staged from and a revision diff cannot"
        )));
    }
    if let Some(unsaved) = unsaved_in_tab(ws, project, tab)? {
        return Err(CoreError::UnsavedChanges {
            tabs: vec![unsaved],
        });
    }
    // Clicking the file already on screen is the common case at the top and bottom of a
    // changelist walk. Bumping for it would broadcast a snapshot per click that changed
    // nothing — the same reason `tab_set_dirty` guards its no-op.
    if *current == spec {
        return Ok(());
    }

    let title = spec.title.clone();
    // Collected before the mutable borrow below, because `ws.windows` is the only record of
    // which tab a torn-out pane belongs to and `project_mut` wants the whole workspace.
    let torn_out = detached_panes_of(ws, project, tab);

    let t = tab_mut(ws, project, tab)?;
    let TabKind::Diff { spec: slot, .. } = &mut t.kind else {
        unreachable!("checked above under an immutable borrow")
    };
    *slot = spec;
    for pane in t.tree.panes.values_mut() {
        if pane.kind == PaneKind::Diff {
            pane.title = title.clone();
        }
    }

    // A detached pane is *not* in its tab's tree — [`detach_pane`] moved it into
    // `project.detached` to keep the leaf set and the key set equal — so the loop above
    // cannot reach one, and the `pane:<uuid>` window draws its header straight from
    // `project.detached[pane].title`. Without this a diff torn into its own window would go
    // on naming the file it was torn out on while showing every file clicked since.
    let p = project_mut(ws, project)?;
    for id in torn_out {
        if let Some(pane) = p.detached.get_mut(&id)
            && pane.kind == PaneKind::Diff
        {
            pane.title = title.clone();
        }
    }

    bump(ws);
    Ok(())
}

/// The panes torn out of one tab, as recorded by the windows showing them.
///
/// `project.detached` is keyed by [`PaneId`] alone and does not say which tab a pane came
/// from; [`WindowRole::DetachedPane`] does, so the window map is the index. Scanned rather
/// than cached — a project holds a handful of windows at most, and a second map keyed by tab
/// would be a third thing to keep in step with `detached` and `dock_anchors`.
fn detached_panes_of(ws: &Workspace, project: ProjectId, tab: TabId) -> Vec<PaneId> {
    ws.windows
        .values()
        .filter_map(|role| match role {
            WindowRole::DetachedPane {
                project: p,
                tab: t,
                pane,
            } if *p == project && *t == tab => Some(*pane),
            _ => None,
        })
        .collect()
}

/// Promote a preview diff tab to a kept one — the double-click half of the pair.
///
/// Idempotent, and silent on a tab that was never a preview: `tab_open_diff` calls it on
/// whatever tab it found for the file, and "the user double-clicked a tab that was already
/// permanent" is not an error, it is the ordinary case.
///
/// Named for what it means rather than `set_preview(false)`: there is no gesture in the
/// product that turns a kept tab back into a scratch one, so the reverse direction would be
/// an API with no caller and one plausible misuse.
pub fn promote_diff(ws: &mut Workspace, project: ProjectId, tab: TabId) -> Result<()> {
    let t = tab_mut(ws, project, tab)?;
    let TabKind::Diff { preview, .. } = &mut t.kind else {
        return Err(CoreError::Invariant(format!(
            "tab {tab} is not a diff tab and has no preview state"
        )));
    };
    if !*preview {
        return Ok(());
    }
    *preview = false;
    bump(ws);
    Ok(())
}

/// Follow files that moved on disk: rewrite every [`TabKind::File`] tab that named one. (M25)
///
/// `moves` is `(from, to)` pairs, and a pair matches a tab either **exactly** — the file itself
/// was renamed — or as a **directory prefix**, which is the case that is easy to forget: renaming
/// `src/` moves every open file under it, and a tab left pointing at `src/old/main.rs` is as
/// broken as one pointing at the old name of the file itself.
///
/// # Why this exists at all
///
/// A `File` tab is an absolute path and nothing else, and `cide_core::document::write` resolves
/// that path with `canonicalize`, which **requires the file to exist**. So a rename in the file
/// tree used to leave the editor holding a path with nothing behind it: the tab went on showing
/// the old name, and every save — Ctrl+S and autosave alike — failed with `ENOENT` on a buffer
/// the user could still type into. The tab stayed dirty with no gesture that could clean it.
/// That is the bug this closes, and it is why the retarget belongs beside the rename in Rust
/// rather than in a webview: `workspace.json` is the record, one gesture moved the file, and two
/// windows deciding separately what their mirror should say is the thing ADR 0002 exists to stop.
///
/// # Every project, not the one the gesture came from
///
/// The path is absolute and says nothing about which project opened it — `TabKind::File`'s own
/// documentation makes the point that nothing there claims the file is even *in* the project. Two
/// projects with overlapping roots, or a file reached by go-to-definition, can both hold a tab on
/// one file, and the file moved for both of them. Scoping this to the calling project would leave
/// the second one holding exactly the broken tab described above.
///
/// # What is deliberately left alone
///
/// * **[`TabKind::Diff`]**. A `ClaudeMcp` diff is the one tab that holds an agent's turn open, and
///   its spec is the *key the pane fetches by* (`claude_diff_content`) rather than a label —
///   re-pointing it would leave a `claude` blocked on a request nobody can answer, which is the
///   failure `cmd::ide`'s "every early return must cancel first" rule exists to prevent. A `Git`
///   diff's paths are repo-relative, so this function does not hold enough to rewrite one.
/// * **[`TabKind::Revision`] and [`TabKind::Merge`]**, for the second of those reasons: both spell
///   their path repo-relative, and a revision tab names a blob in a commit that a rename in the
///   working tree does not touch.
/// * **The `dirty` flag.** The buffer did not change because the file was renamed; the edits are
///   still in the editor and still unsaved. `ui/src/panes/EditorPane.tsx` carries them across the
///   path change for the same reason.
///
/// Bumps `rev` **only if a tab moved**, which is the ordinary rule here and matters because the
/// common rename is of a file nobody has open: a mirror that was already right must not be told
/// it is stale. Silent on a workspace with nothing to move, exactly as [`promote_diff`] is on a
/// tab that was never a preview — that is not an error, it is the usual case.
pub fn retarget_paths(ws: &mut Workspace, moves: &[(PathBuf, PathBuf)]) {
    // Read first, mutate second. The pane titles have to be rewritten in two places — the tab's
    // own tree and `project.detached`, which `detach_pane` moved panes *out* of the tree into —
    // and the second is reached through `detached_panes_of`, which takes `&Workspace`. Collecting
    // the plan is what lets both happen without a second index keyed by tab.
    let plan: Vec<(ProjectId, TabId, PathBuf)> = ws
        .projects
        .iter()
        .flat_map(|(id, p)| {
            p.tabs.iter().filter_map(move |t| {
                let TabKind::File { path, .. } = &t.kind else {
                    return None;
                };
                let next = moves
                    .iter()
                    .find_map(|(from, to)| moved_path(path, from, to))?;
                Some((*id, t.id, next))
            })
        })
        .collect();
    if plan.is_empty() {
        return;
    }

    for (project, tab, next) in plan {
        // A basename, which is what `cmd::file::tab_open_file` minted the pane with: the pane
        // header draws this string, and leaving it would put `old.rs` above a buffer over
        // `new.rs` — the same staleness `retarget_diff` renames its panes to avoid.
        let title = basename(&next);
        let torn_out = detached_panes_of(ws, project, tab);
        let Ok(t) = tab_mut(ws, project, tab) else {
            continue;
        };
        if let TabKind::File { path, .. } = &mut t.kind {
            *path = next;
        }
        for pane in t.tree.panes.values_mut() {
            if pane.kind == PaneKind::Editor {
                pane.title = title.clone();
            }
        }
        let Ok(p) = project_mut(ws, project) else {
            continue;
        };
        for id in torn_out {
            if let Some(pane) = p.detached.get_mut(&id)
                && pane.kind == PaneKind::Editor
            {
                pane.title = title.clone();
            }
        }
    }

    bump(ws);
}

/// Where `path` ends up when `from` becomes `to`, or `None` if this move does not touch it.
///
/// `pub` because a moved file has more than one record pointing at it: [`retarget_paths`] moves
/// the tabs, and `cide-app`'s position store moves the remembered scroll with the same rule. Two
/// spellings of "is this path inside that folder" is two answers the day one of them is fixed.
///
/// The exact case is answered before `strip_prefix` rather than through it, and that is not
/// tidiness: `strip_prefix` on an equal path yields `""`, and `to.join("")` is `to` **with a
/// trailing separator**. `Path` compares by component so Rust would never notice, but the value is
/// serialised into `workspace.json` and compared as a *string* by every consumer in the webview —
/// `EditorPane`'s path prop, `paneHosts`' map, the tab-already-open check in `tab_open_file`. One
/// stray slash there is a second tab for a file that is already open.
pub fn moved_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    if path == from {
        return Some(to.to_path_buf());
    }
    // Only ever a *directory* prefix: `strip_prefix` matches whole components, so renaming
    // `src/main.rs` does not claim `src/main.rs.bak`.
    let rest = path.strip_prefix(from).ok()?;
    Some(to.join(rest))
}

/// Normalise the walk that led to a revision tab — [`TabKind::Revision::from`]. (M18)
///
/// Pure, and here rather than in `cmd::file` for the reason every other rule in this module is:
/// what a chain does under a merge cannot be exercised through a `State<WorkspaceState>`, and a
/// rule that can only be tested at the level of "does the tab open" is a rule that gets tested
/// once, by hand, on a linear history.
///
/// Three things, and each one is a bug it prevents:
///
/// * **Newest last**, which is [`TabKind::Revision::from`]'s documented order and therefore the
///   order already sitting in every `workspace.json` that has one. The pane draws the strip
///   left to right straight off the slice — `working tree ← from[0] ← … ← from[n-1] ← rev` —
///   so reversing it here would silently reverse the breadcrumb of a restored tab.
/// * **`rev` itself is never in it.** The field is the walk that led *here*, so a chain
///   containing the tab's own revision would draw the current commit twice in its own trail and
///   give the user a crumb that navigates to the tab they are already in.
/// * **Each oid at most once, earliest occurrence kept.** This is the one that is not tidiness.
///   History is a DAG: walking back through a merge and then back again down the other parent
///   rejoins, and the same commit is reached a second time by a different route. Without this
///   the chain grows by one entry per lap around a diamond, for ever, in a `Vec<String>` that
///   `persist` writes to disk on a 500 ms debounce. Keeping the *earliest* occurrence — rather
///   than moving the oid to the end — is what makes the crumb strip a route the user can retrace
///   in the order they actually walked it; moving it would rewrite history behind them so that
///   the trail no longer matches the hops they made.
///
/// Deliberately **not** capped. Deduplication already bounds the chain by the number of distinct
/// revisions that have touched the path, every entry cost the user a deliberate gesture, and a
/// cap would have to drop the *oldest* hops — which are the ones nearest the working tree and so
/// the only route back out of a deep walk.
pub fn revision_chain(rev: &str, from: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::with_capacity(from.len());
    for hop in from {
        if hop == rev || seen.iter().any(|kept| kept == hop) {
            continue;
        }
        seen.push(hop.clone());
    }
    seen
}

/// Move a tab within a project's tab strip.
///
/// Index 0 is the pinned console: moving *to* or *from* it is [`CoreError::TabPinned`].
pub fn reorder_tab(ws: &mut Workspace, project: ProjectId, from: usize, to: usize) -> Result<()> {
    let p = project_mut(ws, project)?;
    let len = p.tabs.len();
    check_index(from, len)?;
    check_index(to, len)?;
    if from == 0 || to == 0 {
        return Err(CoreError::TabPinned);
    }
    if from == to {
        return Ok(());
    }

    let moved = p.tabs.remove(from);
    p.tabs.insert(to, moved);
    bump(ws);
    Ok(())
}

/// Switch between one window holding every project and one window per project.
///
/// This rewrites `ws.windows` and nothing else. No project, tab, pane or session is
/// touched, which is why the setting can be flipped with live Claude sessions running.
/// Tell the workspace which OS window is the stacked shell.
///
/// The core mints its own `shell:<uuid>` labels in [`rebuild_windows`], because on a fresh
/// launch it has to name a window before one exists. The app then creates a *real* Tauri
/// window with a different uuid, and unless the two are reconciled every lookup by the real
/// label misses: `app.get_bootstrap` falls back to an empty shell, and the window shows no
/// active project even though projects are open. Renaming here rather than at creation
/// keeps the core free of any notion of what a window really is.
///
/// Only meaningful in [`WindowMode::Stacked`], where there is exactly one shell. In
/// `PerProject` each project owns a window and the labels are established as they open.
pub fn adopt_shell_window(ws: &mut Workspace, label: WindowLabel) {
    if ws.settings.window_mode != WindowMode::Stacked {
        return;
    }
    let existing = ws
        .windows
        .iter()
        .find(|(_, role)| matches!(role, WindowRole::Shell { .. }))
        .map(|(label, _)| label.clone());

    let role = match existing {
        Some(old) if old == label => return,
        Some(old) => ws.windows.shift_remove(&old).unwrap_or(WindowRole::Shell {
            projects: ws.projects.keys().copied().collect(),
            active: ws.projects.keys().next().copied(),
        }),
        None => WindowRole::Shell {
            projects: ws.projects.keys().copied().collect(),
            active: ws.projects.keys().next().copied(),
        },
    };
    ws.windows.insert(label, role);
    bump(ws);
}

/// Tear a pane out of its tab and give it a window of its own.
///
/// The pane keeps its identity and its session binding: it moves into the project's
/// `detached` holding map rather than being destroyed and rebuilt, so the window that opens
/// attaches to the *same* `SessionId` and the child process never notices. That is the
/// whole reason sessions are owned by the registry rather than by the tree.
///
/// Its position is recorded too, in `project.dock_anchors`, so [`redock_pane`] can put it
/// back in the split it came out of instead of merely somewhere in the same tab.
///
/// Returns the label of the window to create. The caller opens it; this module has no idea
/// what a window really is.
pub fn detach_pane(
    ws: &mut Workspace,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
) -> Result<WindowLabel> {
    // `take_pane` enforces the same refusals as closing: the console's primary pane cannot
    // leave at all, and neither can a tab's last pane. Detaching the only pane of a tab
    // would leave an empty tab behind, which the tree has no way to represent; detaching
    // the primary would leave the console tab with no `Primary` for as long as that window
    // stayed open, and permanently if its re-dock anchor went stale first.
    let (taken, anchor) = {
        let t = tab_mut(ws, project, tab)?;
        // Read before the surgery, in this order and not the other: `take_pane` collapses the
        // split the anchor describes, so asking afterwards would find nothing to record.
        let anchor = layout::anchor_of(&t.tree, pane);
        let taken = layout::take_pane(&mut t.tree, pane)?;
        (taken, anchor)
    };

    let label = WindowLabel::detached_pane();
    let p = project_mut(ws, project)?;
    p.detached.insert(pane, taken);
    // `None` only for a pane that is its tab's whole tree, which `take_pane` has already
    // refused above — so in practice this always records. Kept as an `Option` rather than an
    // `expect` because the absence is a real state the re-dock path must handle anyway: a
    // workspace written by a build older than this one has no anchors at all.
    if let Some(anchor) = anchor {
        p.dock_anchors.insert(pane, anchor);
    }
    ws.windows.insert(
        label.clone(),
        WindowRole::DetachedPane { project, tab, pane },
    );
    bump(ws);
    Ok(label)
}

/// Put a detached pane back where it came from.
///
/// **Exactly** where it came from, whenever that is still a place: [`detach_pane`] recorded
/// the sibling node, the axis, the side and the ratio of the split it was torn out of, and
/// if that sibling is still in the home tab the split is rebuilt around it — same divider
/// id, same ratio. That is the case the gesture is usually made of: detach, look at it on
/// the other monitor, put it back, with the tab untouched in between. Coming back into a
/// fresh 50/50 there is not a rounding error, it is a resize, and a resized terminal makes a
/// `claude` TUI repaint its whole transcript.
///
/// The anchor can still go stale — the sibling may have been closed, or the home tab may be
/// gone entirely and the pane lands in the console — and then it falls back to entering
/// beside whatever currently holds focus. That fallback is the right answer rather than a
/// failure: the position the user is asking for no longer exists, so the pane goes where
/// their attention is, which is what the previous unconditional behaviour did for every case.
///
/// Returns the window label that should now close.
pub fn redock_pane(ws: &mut Workspace, label: &WindowLabel) -> Result<WindowLabel> {
    let Some(WindowRole::DetachedPane { project, tab, pane }) = ws.windows.get(label).cloned()
    else {
        return Err(CoreError::Invariant(format!(
            "window {label} is not a detached pane"
        )));
    };

    let p = project_mut(ws, project)?;
    let Some(taken) = p.detached.shift_remove(&pane) else {
        return Err(CoreError::NoSuchPane(pane));
    };
    // Removed whether or not it gets used: the pane is about to stop being detached, and an
    // anchor outliving it would be a record of a position for a pane that has one.
    let anchor = p.dock_anchors.shift_remove(&pane);

    // The home tab may have been closed while the pane was out. Its console always exists,
    // so that is where it lands rather than being lost with the window.
    let home = if tab_mut(ws, project, tab).is_ok() {
        tab
    } else {
        console_tab(ws, project)?
    };

    let t = tab_mut(ws, project, home)?;
    // `can_restore` also settles the landed-in-the-console case for free: node ids are unique
    // across the workspace, so an anchor from a closed tab matches nothing in the console and
    // takes the fallback without needing a separate check for it.
    let restore = anchor.filter(|a| layout::can_restore(&t.tree, a));
    match restore {
        Some(anchor) => {
            layout::insert_pane_at(&mut t.tree, &anchor, taken)?;
        }
        None => {
            let target = t.tree.focused;
            layout::insert_pane(&mut t.tree, target, Axis::Row, Side::After, taken)?;
        }
    }

    ws.windows.shift_remove(label);
    bump(ws);
    Ok(label.clone())
}

/// The window one tab is torn out into, if any.
///
/// `pub` because three sides of the feature ask it: [`detach_tab`] for idempotence,
/// [`activate_tab`] for the guard below, and `cide-app`'s file-open path, which raises this
/// window instead of activating a tab the shell is deliberately not drawing.
pub fn detached_tab_window(ws: &Workspace, tab: TabId) -> Option<WindowLabel> {
    ws.windows.iter().find_map(|(label, role)| match role {
        WindowRole::DetachedTab { tab: t, .. } if *t == tab => Some(label.clone()),
        _ => None,
    })
}

/// Give a whole tab a window of its own.
///
/// The counterpart of [`detach_pane`] one level up, and deliberately *not* the same
/// mechanism: a pane moves into `project.detached` because an empty slot in a tree is
/// unrepresentable, but a tab needs no holding map — **it stays in `project.tabs`**, and the
/// window role is an overlay saying "this tab is drawn elsewhere". That is what keeps every
/// walk over `p.tabs` honest while the tab is out: `unsaved_tabs` still refuses a quit that
/// would discard its buffer, `plan_restore` still plans its panes, and `sessions_of` still
/// counts its sessions — for the tab's own window rather than the shell's.
///
/// This is also the only road a torn-out *editor* can take. A detached-pane window refuses
/// `PaneKind::Editor` outright (see `ui/src/windows/detachedPane.ts`): buffers are registered
/// per **tab**, and a pane taken out of its tab would need a second buffer over the same
/// file — whichever saved second would silently discard the other's edits. Detaching the tab
/// keeps the `TabId`, so the one buffer moves with it.
///
/// What the shell must then uphold — and this function starts — is that **no shell draws a
/// detached tab**: two windows rendering one tab is two live editors over one buffer, the
/// exact failure the per-tab registry exists to prevent. So the tab leaves `tab_mru` (the
/// switcher's order and `close_tab`'s successor pool) and, when it was active, the shell is
/// moved to the same successor a close would pick. [`activate_tab`] holds the line from the
/// other side.
///
/// Refused for the pinned console ([`CoreError::TabPinned`]): the console is the project's
/// anchor, and the tab strip with a hole at index 0 is a state nothing else handles. Asked
/// twice for a tab already out, it answers the existing window's label without bumping —
/// the caller then has a window to raise rather than an error to word.
pub fn detach_tab(ws: &mut Workspace, project: ProjectId, tab: TabId) -> Result<WindowLabel> {
    let index = index_of_tab(self::project(ws, project)?, tab)?;
    if index == 0 {
        return Err(CoreError::TabPinned);
    }
    if let Some(label) = detached_tab_window(ws, tab) {
        return Ok(label);
    }

    // Which other tabs are already torn out, read before the mutable borrow: the successor
    // below must not hand the shell a tab some other window is drawing.
    let torn: Vec<TabId> = ws
        .windows
        .values()
        .filter_map(|role| match role {
            WindowRole::DetachedTab { tab, .. } => Some(*tab),
            _ => None,
        })
        .collect();

    let p = project_mut(ws, project)?;
    // Out of the focus order exactly as `close_tab` takes a closing tab out: the order is
    // what the Ctrl+Tab switcher walks and what picks a successor, and both must stop
    // offering a tab this window no longer shows. `redock_tab`'s `set_active` puts it back.
    p.tab_mru.retain(|id| *id != tab);
    if p.active_tab == tab {
        // The same successor rule as `close_tab`, because to the shell this *is* a close:
        // the most recently used survivor, else the nearest live neighbour to the left. The
        // `torn` filter is belt over the mru braces — `activate_tab` never lets a detached
        // tab into the order — and load-bearing on the positional arm, where nothing else
        // screens it. `tabs[0]` is the console, which cannot detach, so the arm always finds
        // something.
        let successor = p
            .tab_mru
            .iter()
            .copied()
            .find(|id| !torn.contains(id) && p.tabs.iter().any(|t| t.id == *id))
            .or_else(|| {
                p.tabs[..index]
                    .iter()
                    .rev()
                    .find(|t| !torn.contains(&t.id))
                    .map(|t| t.id)
            });
        if let Some(next) = successor {
            set_active(p, next);
        }
    }

    let label = WindowLabel::detached_tab();
    ws.windows
        .insert(label.clone(), WindowRole::DetachedTab { project, tab });
    bump(ws);
    Ok(label)
}

/// Put a detached tab back in its shell's strip and name the window that should now close.
///
/// Far smaller than [`redock_pane`] because the detach was smaller: the tab never left
/// `project.tabs`, so there is nothing to re-insert and no anchor to restore — dropping the
/// window role *is* the re-dock. `set_active` rather than a bare reappearance, because the
/// gesture is "put it back where I can see it": a tab that silently rejoined a strip the
/// user is not looking at would read as the window closing and the file going with it.
pub fn redock_tab(ws: &mut Workspace, label: &WindowLabel) -> Result<WindowLabel> {
    let Some(WindowRole::DetachedTab { project, tab }) = ws.windows.get(label).cloned() else {
        return Err(CoreError::Invariant(format!(
            "window {label} is not a detached tab"
        )));
    };

    ws.windows.shift_remove(label);
    // The project can have closed while the window was up only if something skipped
    // `close_project`'s pruning; the tab likewise. Neither is worth failing the re-dock
    // over — the window is going away either way, and the role is already gone.
    if let Ok(p) = project_mut(ws, project)
        && p.tabs.iter().any(|t| t.id == tab)
    {
        set_active(p, tab);
    }
    bump(ws);
    Ok(label.clone())
}

/// Every window showing part of `project`, so the app can close them with it.
pub fn windows_for_project(ws: &Workspace, project: ProjectId) -> Vec<WindowLabel> {
    ws.windows
        .iter()
        .filter(|(_, role)| match role {
            WindowRole::Shell { projects, .. } => projects.contains(&project),
            WindowRole::DetachedPane { project: p, .. }
            | WindowRole::DetachedTab { project: p, .. } => *p == project,
        })
        .map(|(label, _)| label.clone())
        .collect()
}

pub fn set_window_mode(ws: &mut Workspace, mode: WindowMode) -> Result<()> {
    if ws.settings.window_mode == mode {
        return Ok(());
    }

    ws.settings.window_mode = mode;
    rebuild_windows(ws);
    bump(ws);
    Ok(())
}

/// Borrow a project by id.
pub fn project(ws: &Workspace, id: ProjectId) -> Result<&Project> {
    ws.projects.get(&id).ok_or(CoreError::NoSuchProject(id))
}

/// Mutably borrow a project by id.
pub fn project_mut(ws: &mut Workspace, id: ProjectId) -> Result<&mut Project> {
    ws.projects.get_mut(&id).ok_or(CoreError::NoSuchProject(id))
}

/// Borrow a tab by id, within a project.
pub fn tab(ws: &Workspace, project: ProjectId, tab: TabId) -> Result<&Tab> {
    let p = self::project(ws, project)?;
    p.tabs
        .iter()
        .find(|t| t.id == tab)
        .ok_or(CoreError::NoSuchTab(tab))
}

/// Mutably borrow a tab by id, within a project.
pub fn tab_mut(ws: &mut Workspace, project: ProjectId, tab: TabId) -> Result<&mut Tab> {
    let p = project_mut(ws, project)?;
    p.tabs
        .iter_mut()
        .find(|t| t.id == tab)
        .ok_or(CoreError::NoSuchTab(tab))
}

/// The id of a project's pinned console tab.
pub fn console_tab(ws: &Workspace, project: ProjectId) -> Result<TabId> {
    self::project(ws, project)?
        .tabs
        .first()
        .map(|t| t.id)
        .ok_or(CoreError::LastTab)
}

/// Record which session a pane is showing — and move the project's primary session with it.
///
/// # Why the second half is not optional
///
/// `Project::primary_session` is minted once, in [`open_project`], and until this function
/// existed **nothing ever wrote it again**. It is not decoration: `lifecycle::entry_for`
/// computes a pane's `eager` flag as `pane.session == Some(project.primary_session)`, which is
/// what brings the project console up *live* on the next launch instead of at a Resume splash.
///
/// So the moment the console's pane bound a different session — which any respawn does, and
/// which already happened on every restore whose transcript had gone — the console stopped
/// being eager and came back at a splash. The field went on naming a conversation no pane held,
/// which is the state `cmd::file`'s tests and `App.tsx`'s mention target both had to write
/// comments about. Moving it with the pane is what makes a restart survive the next launch.
///
/// Only for [`PaneRole::Primary`]. Every other pane binds its own session and has no claim on
/// the project's: a second Claude pane, a mirror or a fork must not silently become the console.
///
/// # A detached pane is not in its tab, and looking only there orphaned live children
///
/// `detach_pane` *removes* the pane from `tab.tree.panes` and parks it in `project.detached`,
/// because the tree invariant is that every leaf is present. So the tab lookup alone answered
/// `NoSuchPane` for exactly the panes a restart is most likely to happen in — a torn-out window
/// is where a user watches a long turn — and the workspace went on naming the child that had
/// died while the one that replaced it belonged to nobody. Re-docking then restored the *old*
/// id, so the pane came back showing a session that no longer existed and the live `claude`
/// survived with no pane at all until the app quit.
///
/// `set_diff_spec` above documents the same fact about the same map. Two functions needing the
/// same correction is the argument for making the fallback explicit here rather than leaving
/// each caller to remember it.
/// Record which conversation the CLI moved a pane's session onto.
///
/// Keyed by `session` — cide's own handle — because that is the only id a hook frame and a
/// pane are guaranteed to agree on. Searches every project, every tab and the detached map,
/// since a hook says nothing about where its pane currently lives and the pane may have been
/// detached into another window since it spawned.
///
/// Returns whether anything changed, so the caller can skip a `rev` bump and the
/// `workspace.json` write behind it. That matters more here than elsewhere: a busy turn
/// sends hook frames continuously and all but the first carry a conversation id the pane has
/// already recorded.
pub fn note_conversation(
    ws: &mut Workspace,
    session: SessionId,
    conversation: SessionId,
    now_ms: u64,
) -> bool {
    for project in ws.projects.values_mut() {
        let panes = project
            .tabs
            .iter_mut()
            .flat_map(|t| t.tree.panes.values_mut())
            .chain(project.detached.values_mut());

        for pane in panes {
            if pane.session != Some(session) {
                continue;
            }
            // `Some(conversation) == pane.conversation` is the common case on a busy turn.
            // Also skip when the CLI is simply using our id, so the field stays `None` for
            // the ordinary pane and `workspace.json` does not grow a redundant uuid per pane.
            let next = (conversation != session).then_some(conversation);
            if pane.conversation == next {
                return false;
            }
            pane.conversation = next;
            // In lockstep, never independently: the stamp means "when the pane arrived on
            // *this* conversation", so a stamp left behind by the previous one would be worse
            // than none at all — it would date a cleared-away name to the clear that replaced
            // it and let it win the comparison it exists to lose. Cleared with the id when the
            // CLI comes back to cide's own, for the same reason.
            pane.conversation_since = next.map(|_| now_ms);
            return true;
        }
    }
    false
}

/// The instant each open conversation became the one its pane is on, keyed by conversation id.
///
/// The half of the `/rename`-across-`/clear` rule that can live here. The other half is
/// `cide_claude::roster`, which reads the CLI's `~/.claude/sessions/<pid>.json` and reports a
/// name **with the `nameSince` it was given at**; this crate cannot see that module (the
/// dependency runs the other way, `cide-claude` → `cide-core`), so the comparison is split:
/// this side answers *what would make a name stale*, and the caller — `cmd::file`'s
/// `claude_session_names` — drops every name not later than the cutoff for its id.
///
/// # Why a name has to be dated at all
///
/// The CLI's name belongs to the **process**, not to the conversation. `/rename` sets it on a
/// per-process singleton; `/clear` starts a fresh conversation inside that same `claude` and
/// rewrites the record with the new `sessionId` and the old `name` still on it. A bare
/// id → name map therefore hands the name the user gave one conversation to the one that
/// replaced it, which is what *"`/clear` doesn't reset the session fully"* looked like from
/// the menu.
///
/// The obvious one-liner — stop consulting `Pane::conversation` and look names up under
/// `Pane::session` — is wrong in the other direction, and quietly: the CLI files its record
/// under the conversation it is *running*, so a rename made after a `/clear` (or after any
/// resume) would then be invisible for the rest of the pane's life. Only a timestamp separates
/// the two cases.
///
/// Panes that have never diverged are absent from the map, not present with a zero: they have
/// no cutoff, and a name found under their id is theirs whenever it was given.
pub fn claude_name_cutoffs(ws: &Workspace) -> std::collections::HashMap<String, u64> {
    let mut cutoffs = std::collections::HashMap::new();
    for project in ws.projects.values() {
        let panes = project
            .tabs
            .iter()
            .flat_map(|t| t.tree.panes.values())
            .chain(project.detached.values());
        for pane in panes {
            if let (Some(conversation), Some(since)) = (pane.conversation, pane.conversation_since)
            {
                // `max` rather than `insert`, because two panes can legitimately name one
                // conversation id — a resume hands the same id to a second pane — and the
                // honest cutoff is the latest move onto it. Taking either arbitrarily would
                // make the answer depend on iteration order.
                let at = cutoffs.entry(conversation.to_string()).or_insert(since);
                *at = (*at).max(since);
            }
        }
    }
    cutoffs
}

pub fn bind_session(
    ws: &mut Workspace,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    session: SessionId,
) -> Result<()> {
    // The detached map first only when the tab does not hold it: an id is in one place or the
    // other, never both, and preferring the tab keeps the ordinary path a single lookup.
    if let Ok(t) = tab_mut(ws, project, tab)
        && let Some(p) = t.tree.panes.get_mut(&pane)
    {
        p.session = Some(session);
        let primary = p.role == PaneRole::Primary;
        if primary {
            project_mut(ws, project)?.primary_session = session;
        }
        return Ok(());
    }

    let proj = project_mut(ws, project)?;
    let Some(p) = proj.detached.get_mut(&pane) else {
        return Err(CoreError::NoSuchPane(pane));
    };
    p.session = Some(session);
    // The console keeps its claim while detached — it is still the project's primary pane, it
    // is merely being shown somewhere else, and re-docking must not find the field stale.
    if p.role == PaneRole::Primary {
        proj.primary_session = session;
    }
    Ok(())
}

/// Locate the tab that owns a pane.
///
/// Pane ids are unique across the whole workspace (asserted by [`validate`]), so a pane
/// found in one tab cannot also be in another.
pub fn find_pane(ws: &Workspace, pane: PaneId) -> Option<(ProjectId, TabId)> {
    ws.projects.iter().find_map(|(id, p)| {
        p.tabs
            .iter()
            .find(|t| t.tree.panes.contains_key(&pane))
            .map(|t| (*id, t.id))
    })
}

/// Check every workspace invariant, including each tab's pane tree.
///
/// Used by tests, by `persist` after loading a file written by an older build, and by the
/// headless binary. A failure here is a bug in this crate or a corrupt file, never
/// something the user did.
/// Recompute every project's `display_path` from its primary root.
///
/// Called after loading, because the stored value was abbreviated against whatever `$HOME`
/// was set when the file was written. Copy a workspace between machines, or open it under
/// a different user, and the stored string is someone else's home directory — harmless,
/// since it is only ever displayed, but wrong on screen until this runs.
pub fn refresh_display_paths(ws: &mut Workspace) {
    for p in ws.projects.values_mut() {
        let Some(primary) = p.roots.first() else {
            continue;
        };
        p.display_path = display_path_of(&primary.path);
    }
}

/// Mark every file tab clean. Called after loading, and only there.
///
/// `dirty` describes an *in-memory buffer*, and no buffer survives the process — the text
/// lives in the editor component, `workspace.json` stores only the path. So a tab restored
/// with the flag set is describing edits that no longer exist anywhere: the editor re-reads
/// the file from disk and shows exactly what is on it.
///
/// Left set, that stale flag is worse than cosmetic now that [`close_tab`] enforces it. The
/// tab could not be closed without a dialog offering to discard changes that are not there,
/// with no gesture that clears it short of editing the file and saving — and a destructive
/// confirmation the user learns is usually a lie is a confirmation they stop reading, which
/// costs them the one time it is true.
///
/// Cleared here rather than by the editor reporting clean on mount: the editor's report is
/// deduplicated against what it last said, so a pane that opens clean says nothing at all,
/// and a file tab restored into a window nobody activates has no editor to report anything.
/// Make every project's [`Project::tab_mru`] satisfy [`validate`], dropping what it cannot.
///
/// Called by `persist::load`, beside [`clear_dirty_flags`], and **repairing rather than
/// rejecting is the whole point**. `WorkspaceState::load` throws the entire workspace away and
/// starts from defaults when validation fails, so a `workspace.json` written by any build
/// before this field existed would cost its author every open project and every open tab —
/// paid on upgrade, for a field that holds nothing a user would miss. That is precisely the
/// "a broken layout must not become a launch loop" rule `persist` opens with.
///
/// Three repairs, in order: drop ids that name no live tab, drop duplicates, and put
/// `active_tab` at the head. The common case is the empty order of a migrated file, which comes
/// out as `[active_tab]` and nothing else.
///
/// The remaining tabs are deliberately **not** appended in strip order. Insertion order is not
/// use order, and inventing one would put a plausible-looking history in a field whose only
/// consumer is a decision about where the user lands — [`close_tab`] would then pick a tab the
/// user has never visited and present it as the one they came from. An order of one is honest,
/// and `close_tab`'s left-neighbour fallback covers exactly this case.
pub fn repair_tab_mru(ws: &mut Workspace) {
    for p in ws.projects.values_mut() {
        let live: HashSet<TabId> = p.tabs.iter().map(|t| t.id).collect();
        let mut seen: HashSet<TabId> = HashSet::new();
        p.tab_mru
            .retain(|id| live.contains(id) && *id != p.active_tab && seen.insert(*id));
        // `active_tab` is validated to name a live tab by the check `validate` already had, so
        // this cannot introduce a dead id — and it cannot duplicate one, because the `retain`
        // above removed every copy of it first.
        p.tab_mru.insert(0, p.active_tab);
    }
}

pub fn clear_dirty_flags(ws: &mut Workspace) {
    for p in ws.projects.values_mut() {
        for t in &mut p.tabs {
            if let TabKind::File { dirty, .. } = &mut t.kind {
                *dirty = false;
            }
        }
    }
}

pub fn validate(ws: &Workspace) -> Result<()> {
    let mut panes: HashSet<PaneId> = HashSet::new();

    for (id, p) in &ws.projects {
        if p.id != *id {
            return Err(CoreError::Invariant(format!(
                "project keyed as {id} carries id {}",
                p.id
            )));
        }
        if p.roots.is_empty() {
            return Err(CoreError::NoRoots);
        }
        if p.tabs.is_empty() {
            return Err(CoreError::LastTab);
        }

        // `display_path` is deliberately NOT validated. It is a projection of `roots[0]`
        // through `$HOME`, so checking it here would make a `workspace.json` valid only
        // under the HOME that wrote it — a file copied between machines, read under `sudo`,
        // or opened in a container would fail validation and be discarded as corrupt over a
        // cosmetic string. `refresh_display_paths` recomputes it on load instead.

        let mut tabs: HashSet<TabId> = HashSet::new();
        for (index, t) in p.tabs.iter().enumerate() {
            // The console is identified by position, so a `ClaudeHome` anywhere else — or
            // anything else at index 0 — would make `TabKind::closable` and `close_tab`
            // disagree about the same tab.
            let is_console = matches!(t.kind, TabKind::ClaudeHome);
            if is_console != (index == 0) {
                return Err(CoreError::TabPinned);
            }
            if !tabs.insert(t.id) {
                return Err(CoreError::Invariant(format!(
                    "project {id} holds tab {} twice",
                    t.id
                )));
            }

            layout::validate(&t.tree)?;
            for pane in t.tree.panes.keys() {
                if !panes.insert(*pane) {
                    return Err(CoreError::Invariant(format!(
                        "pane {pane} appears in more than one tab"
                    )));
                }
            }
        }
        // Detached panes share the one pane-id namespace with every tab's tree: the id is
        // the address every command uses, and a detached pane is still addressable — its
        // window renders it and its session is still bound to it.
        for (pane_id, pane) in &p.detached {
            if pane.id != *pane_id {
                return Err(CoreError::Invariant(format!(
                    "detached pane keyed as {pane_id} carries id {}",
                    pane.id
                )));
            }
            if !panes.insert(*pane_id) {
                return Err(CoreError::Invariant(format!(
                    "pane {pane_id} is detached but also live in a tab"
                )));
            }
        }
        // Anchors are a side map on `detached` and are inserted and removed with it. One left
        // behind describes where a pane that is no longer detached used to sit, and the pane
        // it names is by then live in some tab — where a later re-dock of a *different* pane
        // could match its sibling and rebuild a split that nothing asked for. The stored
        // sibling is deliberately not checked: it is allowed to be stale, which is the whole
        // reason `redock_pane` tests it before trusting it.
        for pane_id in p.dock_anchors.keys() {
            if !p.detached.contains_key(pane_id) {
                return Err(CoreError::Invariant(format!(
                    "pane {pane_id} has a dock anchor but is not detached"
                )));
            }
        }

        if !tabs.contains(&p.active_tab) {
            return Err(CoreError::NoSuchTab(p.active_tab));
        }

        // The focus order names live tabs, each at most once, and starts at the active one.
        //
        // A duplicate is the failure mode of a `set_active` that forgot its `retain`, and it is
        // invisible until a close: the order still *looks* right and Ctrl+Tab still walks it,
        // but the stale copy becomes the successor the moment the tab in front of it goes away.
        // A dead id is the same failure one step further on. Both are cheap to check and
        // impossible to see by inspection, which is the whole argument for checking them here.
        //
        // Shorter than `tabs` is legal and stays legal — see [`Project::tab_mru`]; only ids
        // that name nothing are refused.
        let mut seen_mru: HashSet<TabId> = HashSet::new();
        for id in &p.tab_mru {
            if !tabs.contains(id) {
                return Err(CoreError::NoSuchTab(*id));
            }
            if !seen_mru.insert(*id) {
                return Err(CoreError::Invariant(format!(
                    "tab {id} appears twice in project {}'s focus order",
                    p.id
                )));
            }
        }
        if p.tab_mru.first() != Some(&p.active_tab) {
            return Err(CoreError::Invariant(format!(
                "project {id} is active on tab {} but its focus order starts at {:?}",
                p.active_tab,
                p.tab_mru.first()
            )));
        }
    }

    validate_windows(ws)
}

/// A 3-project workspace with twelve panes, used by this crate's tests, by `persist`'s
/// round-trip test and by `cide-headless`.
///
/// Hidden from the docs because it is a fixture, not API. It is a plain function rather
/// than a cargo feature so that every consumer gets the same one without a feature flag
/// that could be left off in one crate and on in another.
#[doc(hidden)]
pub fn demo_workspace() -> Workspace {
    match build_demo() {
        Ok(ws) => ws,
        Err(e) => {
            // Only reachable if this module and `layout` disagree, which the tests below
            // would catch; an empty workspace is still a valid one to hand back.
            tracing::error!(error = %e, "demo workspace could not be built");
            Workspace::default()
        }
    }
}

// --- internals ---------------------------------------------------------------------

/// Recompute `ws.windows` from the projects and the current window mode.
///
/// Existing shell labels are reused wherever the same set of projects survives the
/// rebuild, so an open OS window keeps its label — and with it its saved geometry, which
/// `tauri-plugin-window-state` keys off the label prefix.
fn rebuild_windows(ws: &mut Workspace) {
    let ids: Vec<ProjectId> = ws.projects.keys().copied().collect();
    let previously_active = ws.windows.values().find_map(|role| match role {
        WindowRole::Shell { active, .. } => Some(*active),
        _ => None,
    });

    let mut next: IndexMap<WindowLabel, WindowRole> = IndexMap::new();
    match ws.settings.window_mode {
        WindowMode::Stacked => {
            // The shell exists even with no projects open — that is the empty frame with a
            // `+` in its header, and dropping the window would leave the app with nothing
            // on screen after the last project closes.
            let label = ws
                .windows
                .iter()
                .find(|(_, role)| matches!(role, WindowRole::Shell { .. }))
                .map(|(label, _)| label.clone())
                .unwrap_or_else(WindowLabel::shell);
            let active = previously_active
                .flatten()
                .filter(|id| ws.projects.contains_key(id))
                .or_else(|| ids.first().copied());
            next.insert(
                label,
                WindowRole::Shell {
                    projects: ids,
                    active,
                },
            );
        }
        WindowMode::PerProject => {
            for id in ids {
                let label = ws
                    .windows
                    .iter()
                    .find(|(_, role)| {
                        matches!(role, WindowRole::Shell { projects, .. }
                            if projects.len() == 1 && projects.first() == Some(&id))
                    })
                    .map(|(label, _)| label.clone())
                    .unwrap_or_else(WindowLabel::shell);
                next.insert(
                    label,
                    WindowRole::Shell {
                        projects: vec![id],
                        active: Some(id),
                    },
                );
            }
        }
    }

    // Detached windows are orthogonal to the mode: they show one pane or one tab, and how
    // projects map onto shells says nothing about them.
    for (label, role) in &ws.windows {
        if !matches!(role, WindowRole::Shell { .. }) {
            next.insert(label.clone(), role.clone());
        }
    }

    ws.windows = next;
}

/// The window half of [`validate`].
fn validate_windows(ws: &Workspace) -> Result<()> {
    let mut shells: Vec<&Vec<ProjectId>> = Vec::new();

    for role in ws.windows.values() {
        match role {
            WindowRole::Shell { projects, active } => {
                for id in projects {
                    if !ws.projects.contains_key(id) {
                        return Err(CoreError::NoSuchProject(*id));
                    }
                }
                // `active` is None exactly when the shell holds nothing. Either half of
                // that being false is a window that names a project it cannot show, or an
                // occupied window with nothing selected.
                match active {
                    Some(id) if !projects.contains(id) => {
                        return Err(CoreError::Invariant(format!(
                            "shell window is active on {id}, which it does not hold"
                        )));
                    }
                    None if !projects.is_empty() => {
                        return Err(CoreError::Invariant(
                            "shell window holds projects but none is active".into(),
                        ));
                    }
                    _ => {}
                }
                shells.push(projects);
            }
            WindowRole::DetachedPane { project, tab, pane } => {
                // `tab` records where the pane re-docks, not where it is: detaching took the
                // leaf out of that tree. The pane itself must be in the project's holding
                // map, or the window renders nothing and re-docking has nothing to put back.
                self::tab(ws, *project, *tab)?;
                if !self::project(ws, *project)?.detached.contains_key(pane) {
                    return Err(CoreError::Invariant(format!(
                        "window shows detached pane {pane}, which the project does not hold"
                    )));
                }
            }
            WindowRole::DetachedTab { project, tab } => {
                self::tab(ws, *project, *tab)?;
            }
        }
    }

    match ws.settings.window_mode {
        WindowMode::Stacked => {
            // Exactly one, always — the shell outlives its projects so that closing the
            // last one leaves the empty frame rather than no window at all.
            let expected = 1;
            if shells.len() != expected {
                return Err(CoreError::Invariant(format!(
                    "stacked mode wants {expected} shell window(s), found {}",
                    shells.len()
                )));
            }
            if let Some(projects) = shells.first()
                && projects.iter().ne(ws.projects.keys())
            {
                return Err(CoreError::Invariant(
                    "the stacked shell window does not list every project in header order".into(),
                ));
            }
        }
        WindowMode::PerProject => {
            let mut covered: HashSet<ProjectId> = HashSet::new();
            for projects in &shells {
                match projects.as_slice() {
                    [id] => {
                        if !covered.insert(*id) {
                            return Err(CoreError::Invariant(format!(
                                "project {id} has more than one window"
                            )));
                        }
                    }
                    _ => {
                        return Err(CoreError::Invariant(format!(
                            "per-project mode wants one project per window, found {}",
                            projects.len()
                        )));
                    }
                }
            }
            if covered.len() != ws.projects.len() {
                return Err(CoreError::Invariant(format!(
                    "{} projects across {} windows",
                    ws.projects.len(),
                    covered.len()
                )));
            }
        }
    }

    Ok(())
}

/// Describe `t` as an [`UnsavedTab`], or `None` if it is not a dirty file tab.
///
/// The one place that decides what "unsaved" means, so [`unsaved_tabs`] and
/// [`unsaved_in_tab`] cannot answer differently about the same tab — which they would
/// eventually, as one of them grew a case the other did not.
fn describe_unsaved(id: ProjectId, p: &Project, t: &Tab) -> Option<UnsavedTab> {
    let TabKind::File { path, dirty: true } = &t.kind else {
        return None;
    };
    Some(UnsavedTab {
        tab: t.id,
        project: id,
        project_name: p.name.clone(),
        path: path.clone(),
        // `TabKind::title` rather than a second basename computation here: the dialog names
        // the tab the user is looking at, so it has to be the string the tab strip drew.
        title: t.kind.title(),
    })
}

/// The header dot for a project about to be opened.
///
/// The first unused colour rather than `len % palette`: closing a project frees its colour,
/// and counting instead of looking would hand the next project the dot of the neighbour it
/// is opened beside.
fn next_dot(ws: &Workspace) -> String {
    let unused = DOT_PALETTE
        .iter()
        .find(|dot| !ws.projects.values().any(|p| p.dot == **dot));
    match unused {
        Some(dot) => (*dot).to_string(),
        // Every colour is taken, so a repeat is unavoidable; rotate to spread it out.
        None => DOT_PALETTE[ws.projects.len() % DOT_PALETTE.len()].to_string(),
    }
}

/// Wrap a path as a project root.
///
/// A root is a path and a label and nothing else. It used to carry a `repo: Option<RepoId>`
/// that this function set to `None` and no other code ever set to anything — the frontend
/// derived a context flag from it and hid every git command behind the result. See
/// [`ProjectRoot`] for the whole account; the short version is that which repositories a
/// project contains is a fact about the disk, so `git_repos` asks the disk.
fn project_root(path: PathBuf) -> ProjectRoot {
    ProjectRoot {
        label: basename(&path),
        path,
    }
}

/// A path's final component, falling back to the whole path for `/` and the like.
fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The display form of a project's primary root, e.g. `~/work/cide`.
fn display_path_of(path: &Path) -> String {
    abbreviate(path, std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

/// [`display_path_of`] with `$HOME` passed in, so it can be tested without touching the
/// process environment.
fn abbreviate(path: &Path, home: Option<&Path>) -> String {
    // An unset or empty `$HOME` must not abbreviate: `Path::strip_prefix("")` succeeds for
    // every relative path, which would turn `work/cide` into `~/work/cide`.
    let Some(home) = home.filter(|home| !home.as_os_str().is_empty()) else {
        return path.display().to_string();
    };
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Position of a tab within its project.
fn index_of_tab(project: &Project, tab: TabId) -> Result<usize> {
    project
        .tabs
        .iter()
        .position(|t| t.id == tab)
        .ok_or(CoreError::NoSuchTab(tab))
}

fn check_index(index: usize, len: usize) -> Result<()> {
    if index < len {
        Ok(())
    } else {
        Err(CoreError::IndexOutOfRange { index, len })
    }
}

/// The fallible body of [`demo_workspace`].
fn build_demo() -> Result<Workspace> {
    let home = PathBuf::from("/home/dev");
    let mut ws = Workspace::default();

    // Project one: two roots, a four-pane console nested three splits deep, plus a full
    // Claude tab and a file tab.
    let cide = open_project(
        &mut ws,
        vec![home.join("work/cide"), home.join("work/cide-docs")],
        None,
    )?;
    let console = console_tab(&ws, cide)?;
    let claude = tab(&ws, cide, console)?.tree.focused;

    let tree = &mut tab_mut(&mut ws, cide, console)?.tree;
    let shell = layout::split(
        tree,
        claude,
        Axis::Row,
        Side::After,
        demo_pane(PaneKind::Shell, "cide : bash", true),
    )?;
    let second = layout::split(
        tree,
        shell,
        Axis::Col,
        Side::After,
        demo_pane(PaneKind::Claude, "cide : claude — tests", true),
    )?;
    layout::split(
        tree,
        second,
        Axis::Row,
        Side::After,
        demo_pane(PaneKind::Diff, "cide : claude — diff", false),
    )?;

    let full = open_tab(
        &mut ws,
        cide,
        TabKind::ClaudeFull {
            title: "refactor pty".into(),
        },
        demo_pane(PaneKind::Claude, "cide : claude — refactor", true),
    )?;
    let full_root = tab(&ws, cide, full)?.tree.focused;
    layout::split(
        &mut tab_mut(&mut ws, cide, full)?.tree,
        full_root,
        Axis::Col,
        Side::After,
        demo_pane(PaneKind::Shell, "cide : bash", true),
    )?;

    open_tab(
        &mut ws,
        cide,
        TabKind::File {
            path: home.join("work/cide/crates/cide-core/src/workspace.rs"),
            dirty: true,
        },
        demo_pane(PaneKind::Editor, "workspace.rs", false),
    )?;

    // Project two: a console split once.
    let atlas = open_project(&mut ws, vec![home.join("work/atlas")], None)?;
    let atlas_console = console_tab(&ws, atlas)?;
    let atlas_claude = tab(&ws, atlas, atlas_console)?.tree.focused;
    layout::split(
        &mut tab_mut(&mut ws, atlas, atlas_console)?.tree,
        atlas_claude,
        Axis::Row,
        Side::Before,
        demo_pane(PaneKind::Shell, "atlas : bash", true),
    )?;

    // Project three: an explicit name, a diff tab and the settings tab.
    let sandbox = open_project(
        &mut ws,
        vec![home.join("scratch/sandbox")],
        Some("sandbox".into()),
    )?;
    open_tab(
        &mut ws,
        sandbox,
        TabKind::Diff {
            spec: DiffSpec {
                title: "main.rs".into(),
                old_path: home.join("scratch/sandbox/src/main.rs"),
                new_path: home.join("scratch/sandbox/src/main.rs.new"),
                origin: DiffOrigin::Git {
                    repo: cide_ipc::RepoId::new(),
                    path: "src/main.rs".into(),
                    side: cide_ipc::git::DiffSide::Combined,
                },
            },
            preview: false,
        },
        demo_pane(PaneKind::Diff, "main.rs — diff", false),
    )?;
    open_tab(
        &mut ws,
        sandbox,
        TabKind::Settings {
            section: SettingsSection::Appearance,
        },
        demo_pane(PaneKind::Editor, "settings", false),
    )?;

    Ok(ws)
}

/// A demo pane. `attached` gives it a fresh session, as a Claude or shell pane has.
fn demo_pane(kind: PaneKind, title: &str, attached: bool) -> Pane {
    Pane {
        id: PaneId::new(),
        kind,
        role: PaneRole::Auxiliary,
        session: attached.then(SessionId::new),
        conversation: None,
        conversation_since: None,
        title: title.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{LayoutNode, SplitId};

    /// Every divider id under a node, outermost first.
    ///
    /// `layout` has its own collector but keeps it private, and these tests want only the
    /// ids; duplicating five lines is cheaper than widening that module's surface for a test.
    fn collect_split_ids_of(node: &LayoutNode) -> Vec<SplitId> {
        let mut out = Vec::new();
        push_split_ids(node, &mut out);
        out
    }

    fn push_split_ids(node: &LayoutNode, out: &mut Vec<SplitId>) {
        if let LayoutNode::Split { id, a, b, .. } = node {
            out.push(*id);
            push_split_ids(a, out);
            push_split_ids(b, out);
        }
    }

    fn open(ws: &mut Workspace, path: &str) -> ProjectId {
        open_project(ws, vec![PathBuf::from(path)], None).expect("a rooted project opens")
    }

    /// Give a project's console a second pane and return its id.
    ///
    /// The console starts with one `Primary` pane, and `take_pane` refuses to remove the
    /// only leaf — so anything testing detach has to widen the tree first.
    fn split_console(ws: &mut Workspace, project: ProjectId) -> PaneId {
        let console = console_tab(ws, project).expect("exists");
        let t = tab_mut(ws, project, console).expect("exists");
        let target = t.tree.focused;
        layout::split(&mut t.tree, target, Axis::Row, Side::After, aux_pane()).expect("splits")
    }

    fn aux_pane() -> Pane {
        demo_pane(PaneKind::Shell, "test : bash", true)
    }

    /// Open a file tab over `path`, clean or dirty. What `tab_open_file` followed by
    /// `tab_set_dirty` produces, without the app layer.
    fn open_file(ws: &mut Workspace, project: ProjectId, path: &str, dirty: bool) -> TabId {
        let path = PathBuf::from(path);
        let title = basename(&path);
        open_tab(
            ws,
            project,
            TabKind::File { path, dirty },
            demo_pane(PaneKind::Editor, &title, false),
        )
        .expect("a file tab opens")
    }

    fn depth(node: &LayoutNode) -> usize {
        match node {
            LayoutNode::Leaf { .. } => 1,
            LayoutNode::Split { a, b, .. } => 1 + depth(a).max(depth(b)),
        }
    }

    fn pane_count(ws: &Workspace) -> usize {
        ws.projects
            .values()
            .flat_map(|p| &p.tabs)
            .map(|t| t.tree.panes.len())
            .sum()
    }

    #[test]
    fn a_new_project_opens_on_a_pinned_console_holding_its_primary_session() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");

        let p = project(&ws, id).expect("the project exists");
        assert_eq!(p.tabs.len(), 1);
        assert!(matches!(p.tabs[0].kind, TabKind::ClaudeHome));
        assert_eq!(p.active_tab, p.tabs[0].id);

        let tree = &p.tabs[0].tree;
        assert_eq!(tree.panes.len(), 1);
        let pane = tree.panes.values().next().expect("one pane");
        assert_eq!(pane.kind, PaneKind::Claude);
        assert_eq!(pane.role, PaneRole::Primary);
        assert_eq!(pane.session, Some(p.primary_session));
        assert_eq!(tree.focused, pane.id);
        validate(&ws).expect("a fresh project is valid");
    }

    /// A restarted console keeps being the console — which is what `eager` is read from.
    ///
    /// `lifecycle::entry_for` computes `eager: pane.session == Some(project.primary_session)`,
    /// and that is the flag that brings the console up **live** on the next launch instead of at
    /// a Resume splash. Nothing ever wrote `primary_session` after project creation, so the
    /// first time the console's pane bound a different session — a restore whose transcript had
    /// gone, and now every restart — the two stopped matching for the rest of the project's
    /// life. The user reports that as a second bug a launch later.
    #[test]
    fn binding_the_console_pane_moves_the_project_primary_session() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("a console exists");
        let pane = project(&ws, id).expect("exists").tabs[0].tree.focused;
        let before = project(&ws, id).expect("exists").primary_session;

        let restarted = SessionId::new();
        bind_session(&mut ws, id, console, pane, restarted).expect("the pane binds");

        let p = project(&ws, id).expect("exists");
        assert_eq!(p.tabs[0].tree.panes[&pane].session, Some(restarted));
        assert_eq!(
            p.primary_session, restarted,
            "the console's session is the project's primary session, or `eager` is false for ever"
        );
        assert_ne!(
            before, restarted,
            "the fixture only means anything if it moved"
        );
        validate(&ws).expect("still valid");
    }

    /// A pane records the conversation its CLI moved onto, and stops recording it once it has.
    ///
    /// The dedupe is the load-bearing half: `note_conversation` is reached from a hook frame,
    /// every frame of a busy turn repeats the same id, and each `true` here costs a `rev` bump
    /// and a `workspace.json` write.
    #[test]
    fn a_pane_records_the_conversation_its_cli_moved_onto_exactly_once() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let pane = project(&ws, id).expect("exists").tabs[0].tree.focused;
        let session = project(&ws, id).expect("exists").primary_session;

        let cleared = SessionId::new();
        assert!(
            note_conversation(&mut ws, session, cleared, 1_700),
            "the first frame after a `/clear` has something to say"
        );
        assert_eq!(
            project(&ws, id).expect("exists").tabs[0].tree.panes[&pane].conversation,
            Some(cleared)
        );
        assert_eq!(
            project(&ws, id).expect("exists").tabs[0].tree.panes[&pane].conversation_since,
            Some(1_700),
            "the stamp moves with the id, because a name is dated against it"
        );
        assert!(
            !note_conversation(&mut ws, session, cleared, 9_999),
            "every later frame of the same turn repeats it and must cost nothing"
        );
        assert_eq!(
            project(&ws, id).expect("exists").tabs[0].tree.panes[&pane].conversation_since,
            Some(1_700),
            "and a repeat must not re-date the conversation the pane is already on"
        );

        // A pane whose CLI is simply using our id stays `None`, so `workspace.json` does not
        // grow a redundant uuid per pane and `restore_for`'s fallback stays the common path.
        let mut plain = Workspace::default();
        let id = open(&mut plain, "/home/dev/work/cide");
        let session = project(&plain, id).expect("exists").primary_session;
        let pane = project(&plain, id).expect("exists").tabs[0].tree.focused;
        assert!(!note_conversation(&mut plain, session, session, 1_700));
        assert_eq!(
            project(&plain, id).expect("exists").tabs[0].tree.panes[&pane].conversation,
            None
        );
        assert_eq!(
            project(&plain, id).expect("exists").tabs[0].tree.panes[&pane].conversation_since,
            None
        );

        // And a session no pane holds is not an error: a hook can outlive its pane.
        assert!(!note_conversation(
            &mut plain,
            SessionId::new(),
            SessionId::new(),
            1_700
        ));
        validate(&plain).expect("still valid");
    }

    /// A cleared pane dates its conversation, so a name given before the `/clear` is stale.
    ///
    /// The rule is a comparison and not a lookup, and the two directions of it are what this
    /// pins: a name whose `nameSince` predates the move is the one the CLI carried across the
    /// `/clear` and must be dropped, and one stamped after it is a rename the user made on the
    /// conversation they are actually on and must survive. A pane that has never diverged has
    /// no cutoff at all, so its name is never questioned.
    #[test]
    fn a_cleared_pane_dates_the_conversation_it_moved_onto() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let session = project(&ws, id).expect("exists").primary_session;

        assert!(
            claude_name_cutoffs(&ws).is_empty(),
            "a pane still on the id it was spawned under questions nothing"
        );

        let cleared = SessionId::new();
        note_conversation(&mut ws, session, cleared, 1_700);
        let cutoffs = claude_name_cutoffs(&ws);
        assert_eq!(cutoffs.get(&cleared.to_string()), Some(&1_700));
        assert_eq!(cutoffs.len(), 1, "{cutoffs:?}");

        // What the caller does with it, spelled out here because the comparison is the rule
        // and the boundary is the half that is easy to get backwards. `nameSince == cutoff`
        // is the CLI writing both in the same millisecond as it starts the new conversation,
        // which is the carried-over name, not a rename.
        let cutoff = cutoffs[&cleared.to_string()];
        assert!(1_699 <= cutoff, "a name from before the /clear is stale");
        assert!(1_700 <= cutoff, "and one from the same instant is too");
        assert!(1_701 > cutoff, "a rename after the /clear survives");

        // A second `/clear` re-dates it; the first cutoff must not linger.
        let again = SessionId::new();
        note_conversation(&mut ws, session, again, 2_400);
        let cutoffs = claude_name_cutoffs(&ws);
        assert_eq!(cutoffs.get(&again.to_string()), Some(&2_400));
        assert!(
            !cutoffs.contains_key(&cleared.to_string()),
            "the conversation the pane has left is nobody's cutoff"
        );
    }

    /// And every other pane binds only itself.
    ///
    /// A mirror, a fork or a second Claude pane must not silently become the console: they are
    /// bound by the same command on the same path, and the difference is `PaneRole::Primary`.
    #[test]
    fn binding_a_secondary_pane_leaves_the_primary_session_alone() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("a console exists");
        let second = split_console(&mut ws, id);
        let primary = project(&ws, id).expect("exists").primary_session;

        bind_session(&mut ws, id, console, second, SessionId::new()).expect("the pane binds");

        assert_eq!(project(&ws, id).expect("exists").primary_session, primary);
    }

    #[test]
    fn a_detached_pane_can_still_bind_a_session_it_respawned() {
        // The blocker this fallback exists for. A torn-out window is where a user watches a
        // long turn, so it is where a double Ctrl+C and a restart happen — and `detach_pane`
        // has already moved the pane out of `tab.tree.panes`, so a tab-only lookup answered
        // `NoSuchPane` and the workspace kept naming the child that died. Re-docking then
        // restored the dead id while the live `claude` belonged to no pane at all.
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("a console exists");
        let extra = split_console(&mut ws, id);

        detach_pane(&mut ws, id, console, extra).expect("the pane detaches");
        assert!(
            !ws.projects[&id].tabs[0].tree.panes.contains_key(&extra),
            "the pane really has left its tab — otherwise this test proves nothing",
        );

        let respawned = SessionId::new();
        bind_session(&mut ws, id, console, extra, respawned).expect("a detached pane binds");
        assert_eq!(
            ws.projects[&id].detached[&extra].session,
            Some(respawned),
            "the detached pane records the session that replaced the one that died",
        );
    }

    #[test]
    fn binding_a_pane_that_is_not_there_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("a console exists");
        let ghost = PaneId::new();
        assert_eq!(
            bind_session(&mut ws, id, console, ghost, SessionId::new()),
            Err(CoreError::NoSuchPane(ghost)),
        );
    }

    #[test]
    fn opening_a_project_without_roots_is_refused() {
        let mut ws = Workspace::default();
        assert_eq!(open_project(&mut ws, vec![], None), Err(CoreError::NoRoots));
        assert!(ws.projects.is_empty());
        assert_eq!(
            ws.rev, 0,
            "a refused mutation does not advance the revision"
        );
    }

    #[test]
    fn an_unnamed_project_takes_its_name_from_the_primary_root() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        assert_eq!(project(&ws, id).expect("exists").name, "cide");

        let named = open_project(
            &mut ws,
            vec![PathBuf::from("/home/dev/work/atlas")],
            Some("Atlas".into()),
        )
        .expect("opens");
        assert_eq!(project(&ws, named).expect("exists").name, "Atlas");
    }

    #[test]
    fn display_paths_abbreviate_the_home_directory() {
        let home = Path::new("/home/dev");
        assert_eq!(
            abbreviate(Path::new("/home/dev/work/cide"), Some(home)),
            "~/work/cide"
        );
        assert_eq!(abbreviate(Path::new("/home/dev"), Some(home)), "~");
        assert_eq!(abbreviate(Path::new("/srv/code"), Some(home)), "/srv/code");
        assert_eq!(abbreviate(Path::new("/srv/code"), None), "/srv/code");
        // An empty $HOME is a prefix of every relative path and must not abbreviate.
        assert_eq!(
            abbreviate(Path::new("work/cide"), Some(Path::new(""))),
            "work/cide"
        );
    }

    #[test]
    fn projects_get_distinct_header_dots() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");
        assert_ne!(
            project(&ws, a).expect("exists").dot,
            project(&ws, b).expect("exists").dot
        );
    }

    #[test]
    fn a_freed_dot_is_reused_before_a_neighbours() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");
        close_project(&mut ws, a, false).expect("closes");
        let c = open(&mut ws, "/home/dev/c");

        assert_ne!(
            project(&ws, b).expect("exists").dot,
            project(&ws, c).expect("exists").dot,
            "a project opened into a freed slot must not repeat its neighbour's dot"
        );
    }

    #[test]
    fn closing_the_console_tab_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");

        assert_eq!(
            close_tab(&mut ws, id, console, false),
            Err(CoreError::TabPinned)
        );
        assert_eq!(project(&ws, id).expect("exists").tabs.len(), 1);
    }

    // --- the unsaved-close guard ---------------------------------------------------
    //
    // These are the tests for the defect that a file tab with unsaved edits closed silently
    // and the edits were gone. The rule lives here, in the domain, rather than in the
    // dialog: the dialog is what a user sees, this is what makes it impossible to skip.

    #[test]
    fn a_dirty_file_tab_does_not_close_without_force() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let tab = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", true);
        let rev = ws.rev;

        let refused = close_tab(&mut ws, id, tab, false);

        // The refusal *names* the file. A caller told only "no" cannot write the dialog,
        // and a dialog that says "1 item" gives the user nothing to decide on.
        let Err(CoreError::UnsavedChanges { tabs }) = refused else {
            panic!("a dirty tab must not close without force, got {refused:?}");
        };
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].tab, tab);
        assert_eq!(tabs[0].project, id);
        assert_eq!(tabs[0].title, "main.rs");
        assert_eq!(
            tabs[0].path,
            PathBuf::from("/home/dev/work/cide/src/main.rs")
        );

        assert_eq!(
            project(&ws, id).expect("exists").tabs.len(),
            2,
            "still open"
        );
        assert_eq!(
            ws.rev, rev,
            "a refused mutation does not advance the revision"
        );
        validate(&ws).expect("nothing was half-done");
    }

    #[test]
    fn a_dirty_file_tab_closes_when_forced() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let tab = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", true);

        close_tab(&mut ws, id, tab, true).expect("force discards the buffer");
        assert_eq!(project(&ws, id).expect("exists").tabs.len(), 1);
        validate(&ws).expect("valid");
    }

    #[test]
    fn a_clean_file_tab_closes_without_force() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let tab = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", false);

        // The other half of the rule, and the half that keeps the dialog worth reading: a
        // confirmation that fires when nothing is at risk is one users dismiss unread.
        close_tab(&mut ws, id, tab, false).expect("nothing is at risk");
        assert_eq!(project(&ws, id).expect("exists").tabs.len(), 1);
    }

    #[test]
    fn saving_a_tab_clears_the_refusal() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let tab = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", true);
        assert!(close_tab(&mut ws, id, tab, false).is_err());

        // What `tab_set_dirty(false)` does after a successful write.
        let TabKind::File { dirty, .. } = &mut tab_mut(&mut ws, id, tab).expect("exists").kind
        else {
            panic!("a file tab");
        };
        *dirty = false;

        close_tab(&mut ws, id, tab, false).expect("a saved tab closes like any other");
    }

    #[test]
    fn a_project_holding_a_dirty_tab_does_not_close_and_names_every_one() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        open_file(&mut ws, id, "/home/dev/work/cide/a.rs", true);
        open_file(&mut ws, id, "/home/dev/work/cide/clean.rs", false);
        open_file(&mut ws, id, "/home/dev/work/cide/b.rs", true);

        // Closing a project closes its tabs, so the same buffers are at stake as in
        // `close_tab`. Guarding only the narrower gesture would leave the wider one open.
        let Err(CoreError::UnsavedChanges { tabs }) = close_project(&mut ws, id, false) else {
            panic!("a project holding unsaved work must not close without force");
        };
        // Strip order, which `open_tab` makes newest-first: b.rs was opened last, so it sits
        // nearest the console and is named first. The order matters because this list is what
        // the close dialog reads out, and a list that did not match the strip would ask about
        // the tabs in an order the user cannot see.
        assert_eq!(
            tabs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["b.rs", "a.rs"],
            "every dirty tab, and only the dirty ones"
        );
        assert!(ws.projects.contains_key(&id), "the project is still open");

        close_project(&mut ws, id, true).expect("force discards them");
        assert!(!ws.projects.contains_key(&id));
        validate(&ws).expect("valid");
    }

    #[test]
    fn unsaved_tabs_answers_for_one_project_or_for_all_of_them() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");
        open_file(&mut ws, a, "/home/dev/a/one.rs", true);
        open_file(&mut ws, b, "/home/dev/b/two.rs", true);
        open_file(&mut ws, b, "/home/dev/b/three.rs", false);

        assert_eq!(
            unsaved_tabs(&ws, Some(a))
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            vec!["one.rs"]
        );
        // The whole-workspace answer, which is what the quit path asks for.
        assert_eq!(
            unsaved_tabs(&ws, None)
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            vec!["one.rs", "two.rs"]
        );
        assert!(unsaved_tabs(&Workspace::default(), None).is_empty());
    }

    #[test]
    fn only_dirty_file_tabs_count_as_unsaved() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        // A console, a full Claude tab and a diff tab. None of them holds a buffer: a diff
        // is answered rather than saved, and a terminal has nothing to write.
        open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "one".into(),
            },
            aux_pane(),
        )
        .expect("opens");
        open_tab(
            &mut ws,
            id,
            TabKind::Diff {
                spec: DiffSpec {
                    title: "main.rs".into(),
                    old_path: PathBuf::from("/home/dev/work/cide/src/main.rs"),
                    new_path: PathBuf::from("/home/dev/work/cide/src/main.rs.new"),
                    origin: DiffOrigin::Git {
                        repo: cide_ipc::RepoId::new(),
                        path: "src/main.rs".into(),
                        side: cide_ipc::git::DiffSide::Combined,
                    },
                },
                preview: false,
            },
            demo_pane(PaneKind::Diff, "main.rs — diff", false),
        )
        .expect("opens");

        assert!(unsaved_tabs(&ws, None).is_empty());
    }

    #[test]
    fn closing_an_editor_pane_refuses_to_discard_its_buffer() {
        // The hole `close_pane` was written for: `close_tab` refused this loss while
        // `pane_close` went straight to `layout::close`, so a user who could not close the
        // tab could close the editor pane inside it and lose exactly as much.
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let tab = open_tab(
            &mut ws,
            id,
            TabKind::File {
                path: PathBuf::from("/home/dev/work/cide/src/main.rs"),
                dirty: true,
            },
            demo_pane(PaneKind::Editor, "main.rs", false),
        )
        .expect("opens");

        // A second pane, so the refusal is about the buffer and not about `LastPane`.
        let t = tab_mut(&mut ws, id, tab).expect("exists");
        let editor = t.tree.focused;
        layout::split(
            &mut t.tree,
            editor,
            Axis::Row,
            Side::After,
            demo_pane(PaneKind::Claude, "claude", false),
        )
        .expect("splits");

        let before = ws.rev;
        let err = close_pane(&mut ws, id, tab, editor, false).expect_err("refuses");
        assert!(
            matches!(err, CoreError::UnsavedChanges { ref tabs } if tabs.len() == 1),
            "expected UnsavedChanges naming the file, got {err:?}"
        );
        assert_eq!(ws.rev, before, "a refusal must not move rev");
        assert!(
            tab_mut(&mut ws, id, tab)
                .expect("exists")
                .tree
                .panes
                .contains_key(&editor),
            "the pane must survive a refusal"
        );

        close_pane(&mut ws, id, tab, editor, true).expect("force discards");
        assert!(
            !tab_mut(&mut ws, id, tab)
                .expect("exists")
                .tree
                .panes
                .contains_key(&editor)
        );
    }

    #[test]
    fn closing_a_terminal_pane_beside_a_dirty_editor_is_not_refused() {
        // The guard must not fire when nothing is at risk. Closing the Claude pane in a file
        // tab discards no buffer, and a confirmation there is one the user learns to dismiss.
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let tab = open_tab(
            &mut ws,
            id,
            TabKind::File {
                path: PathBuf::from("/home/dev/work/cide/src/main.rs"),
                dirty: true,
            },
            demo_pane(PaneKind::Editor, "main.rs", false),
        )
        .expect("opens");

        let t = tab_mut(&mut ws, id, tab).expect("exists");
        let editor = t.tree.focused;
        let claude = layout::split(
            &mut t.tree,
            editor,
            Axis::Row,
            Side::After,
            demo_pane(PaneKind::Claude, "claude", false),
        )
        .expect("splits");

        close_pane(&mut ws, id, tab, claude, false).expect("no buffer is at risk here");
    }

    #[test]
    fn unsaved_in_tab_reports_an_unknown_tab_rather_than_answering_none() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let ghost = TabId::new();

        // Failing open here would be the whole bug back again: a caller reading "no such
        // tab" as "nothing at risk" closes over a buffer it never looked at.
        assert_eq!(
            unsaved_in_tab(&ws, id, ghost),
            Err(CoreError::NoSuchTab(ghost))
        );
    }

    #[test]
    fn the_pinned_console_is_refused_as_pinned_not_as_unsaved() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", true);

        // Two refusals can apply to one project; the console's own reason is the one that
        // has to come back, or the frontend offers to discard edits that were never at risk.
        assert_eq!(
            close_tab(&mut ws, id, console, false),
            Err(CoreError::TabPinned)
        );
    }

    /// Every new tab lands at index 1, so the strip reads newest-first from a fixed anchor.
    ///
    /// The user-facing rule is "the file I just opened is the leftmost file tab". The console
    /// is not a file tab and never moves, which is why "leftmost" means index 1 rather than 0 —
    /// and why this asserts the console is still at 0 after every open rather than only at the
    /// end: a rule expressed as `insert(1, ..)` is one typo away from displacing the pinned tab,
    /// and `validate` is the only thing that would have caught it.
    ///
    /// Mixed kinds on purpose. The placement is a property of opening a tab, not of the tab's
    /// kind: a file, a diff and a Claude tab opened in that order must interleave by *when*,
    /// because that is the only thing a user can read off the strip.
    #[test]
    fn a_new_tab_opens_immediately_right_of_the_pinned_console() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = project(&ws, id).expect("exists").tabs[0].id;

        let first = open_file(&mut ws, id, "/home/dev/work/cide/a.rs", false);
        let second = full_tab(&mut ws, id, "two");
        let third = open_file(&mut ws, id, "/home/dev/work/cide/b.rs", false);

        let tabs: Vec<TabId> = project(&ws, id)
            .expect("exists")
            .tabs
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(
            tabs,
            vec![console, third, second, first],
            "newest first, immediately right of the console"
        );
        assert!(
            matches!(
                project(&ws, id).expect("exists").tabs[0].kind,
                TabKind::ClaudeHome
            ),
            "and the pinned console is still the tab nothing may displace"
        );
        assert_eq!(
            project(&ws, id).expect("exists").active_tab,
            third,
            "opening a tab still activates it"
        );
        validate(&ws).expect("still valid");
    }

    /// A project that has somehow lost its console does not take the process down.
    ///
    /// `Vec::insert` panics past the end and this crate is linked into a binary built with
    /// `panic = "abort"`, so the difference between `insert(1, ..)` and a clamped index is the
    /// difference between a refused mutation and a dead app. The workspace below is illegal —
    /// `validate` rejects it, and `WorkspaceState::update` would roll the whole thing back — but
    /// it has to *reach* the validator to be rejected.
    #[test]
    fn opening_a_tab_in_a_project_with_no_console_does_not_panic() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        project_mut(&mut ws, id).expect("exists").tabs.clear();

        let tab = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "one".into(),
            },
            aux_pane(),
        )
        .expect("the insert does not panic");
        assert_eq!(
            project(&ws, id).expect("exists").tabs[0].id,
            tab,
            "it lands at 0 because there was nothing to sit behind"
        );
        assert!(
            validate(&ws).is_err(),
            "and the validator is what refuses the state, as it always was"
        );
    }

    #[test]
    fn opening_a_second_console_tab_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");

        assert_eq!(
            open_tab(&mut ws, id, TabKind::ClaudeHome, aux_pane()),
            Err(CoreError::TabPinned)
        );
        assert_eq!(project(&ws, id).expect("exists").tabs.len(), 1);
    }

    /// A closable full tab, named, so the MRU tests below read as a sequence of gestures
    /// rather than as six copies of the same twelve-line literal.
    fn full_tab(ws: &mut Workspace, id: ProjectId, title: &str) -> TabId {
        open_tab(
            ws,
            id,
            TabKind::ClaudeFull {
                title: title.into(),
            },
            aux_pane(),
        )
        .expect("opens")
    }

    /// The order `tab_mru` holds, for assertions that care about the whole of it.
    fn mru(ws: &Workspace, id: ProjectId) -> Vec<TabId> {
        project(ws, id).expect("exists").tab_mru.clone()
    }

    /// The headline of the feature: closing lands you where you came *from*, not next door.
    ///
    /// The arrangement is chosen so the two rules disagree. `first` is the left neighbour of
    /// `third` after `second` is skipped over, so a test that opened three tabs and closed the
    /// last would pass under either rule — opening a tab activates it, which makes the focus
    /// order the reverse of the strip until something moves. The extra `activate_tab` is what
    /// pulls them apart.
    #[test]
    fn closing_the_active_tab_activates_the_most_recently_used_survivor() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let first = full_tab(&mut ws, id, "one");
        let second = full_tab(&mut ws, id, "two");
        let third = full_tab(&mut ws, id, "three");

        // Read `first` again, then come back to `third` and close it.
        activate_tab(&mut ws, id, first).expect("activates");
        activate_tab(&mut ws, id, third).expect("activates");
        close_tab(&mut ws, id, third, false).expect("a full tab closes");

        let p = project(&ws, id).expect("exists");
        assert_eq!(p.active_tab, first, "the tab the user came from");
        assert_ne!(
            p.active_tab, second,
            "and not the tab to its left, which is what the rule used to answer"
        );
        assert!(
            !p.tab_mru.contains(&third),
            "and the closed tab is out of the order"
        );
        validate(&ws).expect("still valid");
    }

    /// The fallback, and the case every existing `workspace.json` starts in.
    ///
    /// `tab_mru` is trimmed by hand to exactly what `repair_tab_mru` produces for a file
    /// written before the field existed: the active tab and nothing behind it. The successor
    /// then has to come from somewhere else, and the somewhere else is the old left-neighbour
    /// rule — which is why that code is still in `close_tab` and must stay.
    #[test]
    fn closing_the_active_tab_falls_back_to_the_left_neighbour_with_no_history() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let first = full_tab(&mut ws, id, "one");
        let second = full_tab(&mut ws, id, "two");

        // The strip is `[console, second, first]` — `open_tab` inserts at 1 — so `first` is the
        // one whose left neighbour is another *closable* tab. Closing the tab next to the
        // console would fall back to the console and pass under any rule, which is the shape
        // this test exists to avoid.
        activate_tab(&mut ws, id, first).expect("exists");
        project_mut(&mut ws, id).expect("exists").tab_mru = vec![first];
        validate(&ws).expect("a one-entry order is a legal one");

        close_tab(&mut ws, id, first, false).expect("a full tab closes");
        assert_eq!(project(&ws, id).expect("exists").active_tab, second);
        validate(&ws).expect("still valid");
    }

    /// The pinned console is a legitimate successor, and `close_tab`'s guarantee rests on it.
    ///
    /// Excluding `tabs[0]` from the order was considered and lost for the reason
    /// `commands::table`'s note on `tab.switcher.next` gives: "from the file I was reading back
    /// to the conversation about it" is the whole value of the gesture. It also happens to be
    /// what makes the fallback total — `tabs[0]` cannot be closed, so there is always at least
    /// one survivor to land on.
    #[test]
    fn the_pinned_console_can_be_the_successor() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = project(&ws, id).expect("exists").tabs[0].id;
        let first = full_tab(&mut ws, id, "one");
        let second = full_tab(&mut ws, id, "two");

        // Read the conversation, then open the file again from it, then close the file.
        activate_tab(&mut ws, id, console).expect("activates");
        activate_tab(&mut ws, id, second).expect("activates");
        close_tab(&mut ws, id, second, false).expect("closes");

        let p = project(&ws, id).expect("exists");
        assert_eq!(p.active_tab, console);
        assert_eq!(
            p.tab_mru,
            vec![console, first],
            "and the order behind it survived intact"
        );
    }

    #[test]
    fn closing_an_inactive_tab_leaves_the_active_one_alone() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let doomed = full_tab(&mut ws, id, "one");
        let kept = full_tab(&mut ws, id, "two");

        close_tab(&mut ws, id, doomed, false).expect("closes");
        assert_eq!(project(&ws, id).expect("exists").active_tab, kept);
    }

    /// The ghost: an inactive tab must leave the order even though it does not move `active_tab`.
    ///
    /// This is the failure a webview-supplied successor could not have had and a Rust-owned
    /// order can, so it is the one worth a test of its own. Drop the id only in the
    /// active-close branch and nothing is visibly wrong — until the *next* close reads the
    /// order, finds the tab that went away two gestures ago, and activates it.
    ///
    /// `validate` would catch the state, but only after the mutation that produced it, and
    /// `WorkspaceState::update` answers that by rolling the whole close back: the user's second
    /// `×` would silently do nothing. The assertion on the order is therefore the real one and
    /// the assertion on the second close is the symptom.
    #[test]
    fn closing_an_inactive_tab_drops_it_from_the_focus_order() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = project(&ws, id).expect("exists").tabs[0].id;
        let doomed = full_tab(&mut ws, id, "one");
        let kept = full_tab(&mut ws, id, "two");

        close_tab(&mut ws, id, doomed, false).expect("closes");
        assert_eq!(mru(&ws, id), vec![kept, console]);
        validate(&ws).expect("still valid");

        close_tab(&mut ws, id, kept, false).expect("closes");
        assert_eq!(
            project(&ws, id).expect("exists").active_tab,
            console,
            "the successor is a tab that still exists"
        );
    }

    /// "Close others" fires N closes in a row, and each has to see the last one's order.
    ///
    /// `menuModel.ts` builds them as concurrent `closeTab` calls; they serialise on
    /// `WorkspaceState::update`'s lock, so this is what the domain sees. Nothing special is
    /// done for it — the point of the test is that nothing needs to be.
    #[test]
    fn closing_every_other_tab_in_turn_never_lands_on_a_closed_one() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = project(&ws, id).expect("exists").tabs[0].id;
        let kept = full_tab(&mut ws, id, "keep");
        let others = ["a", "b", "c"].map(|t| full_tab(&mut ws, id, t));

        activate_tab(&mut ws, id, kept).expect("activates");
        for other in others {
            close_tab(&mut ws, id, other, false).expect("closes");
            let p = project(&ws, id).expect("exists");
            assert!(
                p.tabs.iter().any(|t| t.id == p.active_tab),
                "the active tab exists after every close in the run"
            );
        }
        assert_eq!(mru(&ws, id), vec![kept, console]);
        validate(&ws).expect("still valid");
    }

    /// Re-activating the tab that is already active is still a no-op for `rev`.
    ///
    /// `activate_tab` gained a second `changed` clause for the repair case, and the clause is
    /// one `!` away from bumping on every click in the tab strip — which would broadcast the
    /// whole tree to every window for a gesture that changed nothing.
    #[test]
    fn re_activating_the_active_tab_changes_nothing() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let tab = full_tab(&mut ws, id, "one");

        let before = ws.rev;
        activate_tab(&mut ws, id, tab).expect("activates");
        assert_eq!(ws.rev, before, "no bump, so no broadcast");
    }

    /// The header-tab twin of the test above: clicking the project that is already at the
    /// front must not bump. `activate_project` bumped unconditionally until the no-op
    /// suppression in `WorkspaceState::update` made an honest answer matter — a gratuitous
    /// bump there would be undone by the suppressor, but a mutator that reports `ws.rev`
    /// from inside its closure would then hand the caller a revision that never broadcasts.
    #[test]
    fn re_activating_the_active_project_changes_nothing() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");

        let before = ws.rev;
        activate_project(&mut ws, id);
        assert_eq!(ws.rev, before, "no bump, so no broadcast");

        // And a real switch still moves rev, which is what a second window follows.
        // (Opening a second project does not activate it — `rebuild_windows` keeps the
        // previously active one — so switching *to* it is the genuine change here.)
        let other = open(&mut ws, "/home/dev/work/other");
        let before = ws.rev;
        activate_project(&mut ws, other);
        assert_eq!(
            ws.rev,
            before + 1,
            "a real switch is a change and is broadcast"
        );
    }

    /// …and does repair the order when it is the one thing out of step.
    ///
    /// The shape a restored workspace is in for exactly one activation. Hand-built rather than
    /// round-tripped through `persist`, so this test fails if the clause is removed even in a
    /// build where `repair_tab_mru` still runs on load.
    #[test]
    fn activating_the_active_tab_repairs_a_focus_order_that_lost_its_head() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let first = full_tab(&mut ws, id, "one");
        let second = full_tab(&mut ws, id, "two");

        project_mut(&mut ws, id).expect("exists").tab_mru = vec![first];
        let before = ws.rev;
        activate_tab(&mut ws, id, second).expect("activates");

        assert_eq!(mru(&ws, id), vec![second, first]);
        assert_eq!(ws.rev, before + 1, "a repair is a change and is broadcast");
        validate(&ws).expect("still valid");
    }

    /// What a `workspace.json` from before the field turns into.
    ///
    /// Three repairs in one fixture, because they interact: a dead id and a duplicate both have
    /// to go *before* `active_tab` is put at the head, or the head is inserted in front of a
    /// second copy of itself and `validate` refuses the result.
    #[test]
    fn repairing_a_focus_order_drops_dead_ids_and_duplicates_and_leads_with_the_active_tab() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = project(&ws, id).expect("exists").tabs[0].id;
        let first = full_tab(&mut ws, id, "one");
        let second = full_tab(&mut ws, id, "two");

        let ghost = TabId::new();
        let p = project_mut(&mut ws, id).expect("exists");
        p.active_tab = second;
        p.tab_mru = vec![first, ghost, console, first, second];

        repair_tab_mru(&mut ws);
        assert_eq!(mru(&ws, id), vec![second, first, console]);
        validate(&ws).expect("the repair is enough to satisfy the validator");
    }

    /// The reopen path: a tab comes back where it was, as the tree it was, and can go again.
    ///
    /// The pane ids are asserted equal rather than merely present, and that is the load-bearing
    /// half: the frontend's `paneHosts` map is keyed by pane id and a tab close does not clear
    /// it, so a reopened `ClaudeFull` tab re-adopts its own parked terminal — scrollback,
    /// renderer and live session — instead of mounting a fresh one. Remint them and that becomes
    /// a silent regression with no failing test anywhere.
    #[test]
    fn a_reinserted_tab_lands_at_its_old_index_with_its_own_panes() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let _first = full_tab(&mut ws, id, "one");
        let doomed = full_tab(&mut ws, id, "two");
        let _third = full_tab(&mut ws, id, "three");

        let (index, kind, tree) = {
            let p = project(&ws, id).expect("exists");
            let at = p
                .tabs
                .iter()
                .position(|t| t.id == doomed)
                .expect("in the strip");
            (at, p.tabs[at].kind.clone(), p.tabs[at].tree.clone())
        };
        let panes: Vec<PaneId> = tree.panes.keys().copied().collect();
        let sessions: Vec<Option<SessionId>> = tree.panes.values().map(|p| p.session).collect();

        close_tab(&mut ws, id, doomed, false).expect("closes");
        let back = reinsert_tab(&mut ws, id, index, kind, tree).expect("reopens");

        let p = project(&ws, id).expect("exists");
        assert_eq!(
            p.tabs.iter().position(|t| t.id == back),
            Some(index),
            "back in the strip position it was closed from"
        );
        assert_ne!(back, doomed, "with a fresh tab id — the old one is spent");
        assert_eq!(
            p.active_tab, back,
            "and active, because a reopen is an arrival"
        );
        let restored = &p.tabs[index].tree;
        assert_eq!(restored.panes.keys().copied().collect::<Vec<_>>(), panes);
        assert_eq!(
            restored
                .panes
                .values()
                .map(|p| p.session)
                .collect::<Vec<_>>(),
            sessions,
            "session bindings included, or a reopened Claude tab spawns a second conversation"
        );
        validate(&ws).expect("still valid");
    }

    /// A pane id may be live in exactly one place, and `reinsert_tab` is a new way to break it.
    ///
    /// The refusal is not decoration: `validate` rejects a workspace with a pane in two tabs, so
    /// without this check `WorkspaceState::update` would roll the whole reopen back and log a
    /// line the user never sees. Reachable in practice by pressing Ctrl+Shift+T twice against a
    /// stack that somehow held the record twice — which is precisely what a `pop` that peeked
    /// instead of removing would produce.
    #[test]
    fn reinserting_a_tab_whose_panes_are_already_live_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let live = full_tab(&mut ws, id, "one");
        let tree = self::tab(&ws, id, live).expect("exists").tree.clone();

        let refusal = reinsert_tab(
            &mut ws,
            id,
            1,
            TabKind::ClaudeFull {
                title: "one".into(),
            },
            tree,
        );
        assert!(
            matches!(refusal, Err(CoreError::Invariant(_))),
            "got {refusal:?}"
        );
        validate(&ws).expect("and the workspace was not touched");
    }

    /// Index 0 is the console's and a record made when the strip was longer must still land.
    ///
    /// Both directions in one test because they are one clamp: an unclamped `insert` would
    /// either put a tab in front of the pinned console — which `validate` refuses outright — or
    /// panic on an index past the end.
    #[test]
    fn a_reinserted_tab_never_displaces_the_console_and_never_runs_off_the_end() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");

        let low = reinsert_tab(
            &mut ws,
            id,
            0,
            TabKind::Settings {
                section: SettingsSection::default(),
            },
            layout::new_tree(aux_pane()),
        )
        .expect("reopens");
        assert_eq!(
            project(&ws, id)
                .expect("exists")
                .tabs
                .iter()
                .position(|t| t.id == low),
            Some(1),
            "clamped past the pinned console rather than in front of it"
        );

        let high = reinsert_tab(
            &mut ws,
            id,
            99,
            TabKind::ClaudeFull {
                title: "far".into(),
            },
            layout::new_tree(aux_pane()),
        )
        .expect("reopens");
        let p = project(&ws, id).expect("exists");
        assert_eq!(
            p.tabs.iter().position(|t| t.id == high),
            Some(p.tabs.len() - 1),
            "and clamped to the end rather than panicking"
        );
        validate(&ws).expect("still valid");
    }

    /// A second console cannot arrive this way either. `open_tab` refuses it and so must this:
    /// the pinning rule is enforced by *index*, so a `ClaudeHome` anywhere else is a tab that
    /// reports itself unclosable while sitting outside the guard.
    #[test]
    fn reinserting_a_console_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        assert_eq!(
            reinsert_tab(
                &mut ws,
                id,
                1,
                TabKind::ClaudeHome,
                layout::new_tree(aux_pane())
            ),
            Err(CoreError::TabPinned)
        );
    }

    /// The empty order, which is what every existing file has, and the one that matters.
    ///
    /// `WorkspaceState::load` discards the whole workspace when `validate` fails, so without
    /// this repair the first launch after the upgrade would greet the user with no projects.
    #[test]
    fn repairing_an_empty_focus_order_leaves_the_active_tab_and_nothing_invented() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let _first = full_tab(&mut ws, id, "one");
        let second = full_tab(&mut ws, id, "two");

        project_mut(&mut ws, id).expect("exists").tab_mru.clear();
        assert!(
            validate(&ws).is_err(),
            "an empty order is exactly the state the validator refuses"
        );

        repair_tab_mru(&mut ws);
        assert_eq!(
            mru(&ws, id),
            vec![second],
            "strip order is not use order; a history nobody lived is worse than none"
        );
        validate(&ws).expect("valid");
    }

    #[test]
    fn closing_a_tab_drops_the_windows_detached_from_it() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let doomed = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "one".into(),
            },
            aux_pane(),
        )
        .expect("opens");
        let detached = WindowLabel::detached_tab();
        ws.windows.insert(
            detached.clone(),
            WindowRole::DetachedTab {
                project: id,
                tab: doomed,
            },
        );
        // Detached through the real operation rather than by hand: `validate` now requires
        // the project to be holding the pane, which only `detach_pane` arranges.
        let console = console_tab(&ws, id).expect("exists");
        let extra = split_console(&mut ws, id);
        let stray = detach_pane(&mut ws, id, console, extra).expect("detaches");

        close_tab(&mut ws, id, doomed, false).expect("closes");
        assert!(!ws.windows.contains_key(&detached));
        assert!(
            ws.windows.contains_key(&stray),
            "a window detached from another tab is untouched"
        );
        validate(&ws).expect("no window points at a tab that is gone");
    }

    #[test]
    fn opening_an_already_open_path_activates_it_rather_than_duplicating() {
        let mut ws = Workspace::default();
        let first = open(&mut ws, "/home/dev/work/cide");
        let other = open(&mut ws, "/home/dev/work/atlas");

        let again = open(&mut ws, "/home/dev/work/cide");

        assert_eq!(again, first, "the same path is the same project");
        assert_eq!(ws.projects.len(), 2, "no duplicate was created");
        // And it is the one now in front, because "open this path" and "show me this
        // project" are one gesture.
        let Some(WindowRole::Shell { active, .. }) = ws.windows.values().next() else {
            panic!("a shell exists");
        };
        assert_eq!(*active, Some(first));
        assert_ne!(*active, Some(other));
        validate(&ws).expect("valid");
    }

    #[test]
    fn a_detached_pane_keeps_its_identity_and_session() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let extra = split_console(&mut ws, id);
        let before = tab(&ws, id, console).expect("exists").tree.panes[&extra].clone();

        let label = detach_pane(&mut ws, id, console, extra).expect("detaches");

        // Out of the tree, still in the project, unchanged — including its session binding,
        // which is what lets the new window attach to the same running child.
        assert!(
            !tab(&ws, id, console)
                .expect("exists")
                .tree
                .panes
                .contains_key(&extra)
        );
        assert_eq!(project(&ws, id).expect("exists").detached[&extra], before);
        assert!(matches!(
            ws.windows.get(&label),
            Some(WindowRole::DetachedPane { pane, .. }) if *pane == extra
        ));
        validate(&ws).expect("valid");
    }

    #[test]
    fn redocking_returns_the_pane_and_closes_its_window() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let extra = split_console(&mut ws, id);
        let label = detach_pane(&mut ws, id, console, extra).expect("detaches");

        redock_pane(&mut ws, &label).expect("redocks");

        assert!(
            tab(&ws, id, console)
                .expect("exists")
                .tree
                .panes
                .contains_key(&extra)
        );
        assert!(project(&ws, id).expect("exists").detached.is_empty());
        assert!(!ws.windows.contains_key(&label));
        validate(&ws).expect("valid");
    }

    #[test]
    fn detaching_a_tab_keeps_it_in_the_project_and_moves_the_shell_off_it() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let file = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", false);
        assert_eq!(project(&ws, id).expect("exists").active_tab, file);

        let label = detach_tab(&mut ws, id, file).expect("detaches");

        // Unlike a pane, the tab stays where it was: `unsaved_tabs`, `plan_restore` and the
        // awaiting arithmetic all walk `p.tabs`, and the window role alone says it is drawn
        // elsewhere.
        let p = project(&ws, id).expect("exists");
        assert!(p.tabs.iter().any(|t| t.id == file));
        assert!(matches!(
            ws.windows.get(&label),
            Some(WindowRole::DetachedTab { tab, .. }) if *tab == file
        ));
        // The shell stops drawing it: the active tab moves to a survivor and the focus order
        // stops offering it, exactly as if the tab had closed.
        assert_eq!(p.active_tab, console);
        assert!(!p.tab_mru.contains(&file));
        validate(&ws).expect("valid");
    }

    #[test]
    fn detaching_the_pinned_console_tab_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        assert_eq!(detach_tab(&mut ws, id, console), Err(CoreError::TabPinned));
    }

    /// A second detach of the same tab answers the first window rather than minting a rival:
    /// two windows over one tab would be two live editors over one buffer.
    #[test]
    fn detaching_a_tab_twice_answers_the_same_window_and_moves_nothing() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let file = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", false);

        let first = detach_tab(&mut ws, id, file).expect("detaches");
        let rev = ws.rev;
        assert_eq!(detach_tab(&mut ws, id, file), Ok(first));
        assert_eq!(ws.rev, rev, "an idempotent answer is not a mutation");
    }

    /// The guard the whole arrangement leans on: while a tab is out, no activation may make
    /// the shell draw it — success with no movement, because the callers that reach this are
    /// reveal flows that go on to raise the tab's own window.
    #[test]
    fn a_detached_tab_cannot_become_the_shells_active_tab() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let file = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", false);
        detach_tab(&mut ws, id, file).expect("detaches");
        let rev = ws.rev;

        activate_tab(&mut ws, id, file).expect("answered with success, not a refusal");

        let p = project(&ws, id).expect("exists");
        assert_eq!(p.active_tab, console);
        assert!(!p.tab_mru.contains(&file));
        assert_eq!(ws.rev, rev);
    }

    #[test]
    fn redocking_a_tab_drops_its_window_and_brings_the_tab_back_in_front() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let file = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", false);
        let label = detach_tab(&mut ws, id, file).expect("detaches");

        redock_tab(&mut ws, &label).expect("redocks");

        assert!(!ws.windows.contains_key(&label));
        let p = project(&ws, id).expect("exists");
        // Active again, not merely present: "put it back" means back where the user can see
        // it, or the window closing reads as the file going with it.
        assert_eq!(p.active_tab, file);
        assert_eq!(p.tab_mru.first(), Some(&file));
        validate(&ws).expect("valid");
    }

    /// The positional fallback is the arm nothing else screens: with an empty focus order —
    /// a workspace written before `tab_mru` existed — closing a tab walks the strip leftward,
    /// and the walk must step over a tab some other window is drawing.
    #[test]
    fn closing_a_tab_never_hands_the_shell_a_detached_successor() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let a = open_file(&mut ws, id, "/home/dev/work/cide/src/a.rs", false);
        let b = open_file(&mut ws, id, "/home/dev/work/cide/src/b.rs", false);
        detach_tab(&mut ws, id, a).expect("detaches");
        activate_tab(&mut ws, id, b).expect("activates");
        project_mut(&mut ws, id).expect("exists").tab_mru.clear();

        close_tab(&mut ws, id, b, false).expect("closes");

        let p = project(&ws, id).expect("exists");
        assert_eq!(
            p.active_tab, console,
            "the left neighbour was torn out, so the walk continues to the console"
        );
        validate(&ws).expect("valid");
    }

    /// Ctrl+W inside the torn-out window itself: the close prunes the window's role, which
    /// is what lets the app destroy the now-empty window instead of stranding it.
    #[test]
    fn closing_a_detached_tab_drops_its_window_role() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let file = open_file(&mut ws, id, "/home/dev/work/cide/src/main.rs", false);
        let label = detach_tab(&mut ws, id, file).expect("detaches");

        close_tab(&mut ws, id, file, false).expect("closes");

        assert!(!ws.windows.contains_key(&label));
        assert!(
            !project(&ws, id)
                .expect("exists")
                .tabs
                .iter()
                .any(|t| t.id == file)
        );
        validate(&ws).expect("valid");
    }

    /// The criterion: "re-dock restores its tree position". Detach, change nothing, re-dock —
    /// and the tab is the tree it was, ratio included.
    ///
    /// The ratio is the half that costs something. A pane that was 70/30 coming back 50/50
    /// resizes the terminal inside it, and a resized `claude` TUI repaints its entire
    /// transcript; so the console is deliberately dragged off centre before the detach, and
    /// the whole `root` is compared rather than "is the pane back in this tab".
    #[test]
    fn redocking_an_unchanged_tab_restores_the_exact_position_and_ratio() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let extra = split_console(&mut ws, id);

        // A third pane, so the detached one is torn out of a nested split rather than the
        // root: a re-dock that merely re-split the tab at top level would pass on two panes.
        let (deep, before) = {
            let t = tab_mut(&mut ws, id, console).expect("exists");
            let deep = layout::split(&mut t.tree, extra, Axis::Col, Side::After, aux_pane())
                .expect("splits");
            // Both dividers off 0.5, which is what a fresh split writes — a restore that lost
            // the ratio would otherwise still match whichever one had never been dragged.
            for (split, ratio) in collect_split_ids_of(&t.tree.root)
                .into_iter()
                .zip([0.71_f32, 0.29])
            {
                layout::set_ratio(&mut t.tree, split, ratio).expect("a live divider moves");
            }
            (deep, t.tree.clone())
        };

        let label = detach_pane(&mut ws, id, console, deep).expect("detaches");
        assert!(
            project(&ws, id)
                .expect("exists")
                .dock_anchors
                .contains_key(&deep),
            "detaching recorded where the pane was"
        );
        redock_pane(&mut ws, &label).expect("redocks");

        let after = &tab(&ws, id, console).expect("exists").tree;
        assert_eq!(
            after.root, before.root,
            "the tree came back a different shape or with a different ratio"
        );
        assert_eq!(after.focused, deep, "the pane just put back holds focus");
        assert!(
            project(&ws, id).expect("exists").dock_anchors.is_empty(),
            "the anchor is spent, not left behind for a pane that is no longer detached"
        );
        validate(&ws).expect("valid");
    }

    /// The other half: the position the anchor names can genuinely stop existing, and then
    /// falling back to the focus-adjacent placement is the right answer rather than an error.
    #[test]
    fn redocking_falls_back_to_focus_when_the_anchor_has_closed() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let primary = tab(&ws, id, console).expect("exists").tree.focused;
        let extra = split_console(&mut ws, id);
        let deep = {
            let t = tab_mut(&mut ws, id, console).expect("exists");
            layout::split(&mut t.tree, extra, Axis::Col, Side::After, aux_pane()).expect("splits")
        };

        let label = detach_pane(&mut ws, id, console, deep).expect("detaches");
        let anchor = project(&ws, id).expect("exists").dock_anchors[&deep].clone();
        // `deep` split against `extra`; closing it is what takes the anchor's sibling away.
        close_pane(&mut ws, id, console, extra, false).expect("closes");

        redock_pane(&mut ws, &label).expect("redocks anyway");

        let t = tab(&ws, id, console).expect("exists");
        assert!(
            t.tree.panes.contains_key(&deep),
            "a stale anchor must not cost the user the pane"
        );
        assert!(
            !collect_split_ids_of(&t.tree.root).contains(&anchor.split),
            "the recorded divider is not resurrected — that position is gone"
        );
        // Focus-adjacent: the only pane left was the primary, so the pane comes back beside it.
        assert_eq!(layout::leaves(&t.tree.root), vec![primary, deep]);
        validate(&ws).expect("valid");
    }

    /// Closing a tab takes the windows detached *from* it with it, so the pane it was holding
    /// is left in `detached` with nothing that can reach it.
    ///
    /// This used to guard its body with `if ws.windows.contains_key(&label)`, which is never
    /// true — `close_tab` retains only windows whose home tab is not the one closing — so the
    /// test asserted nothing about re-docking and could not fail for its own name. The state
    /// is stated as an assertion instead, and the re-dock it was reaching for is covered by
    /// `a_pane_redocks_into_the_console_when_its_home_tab_is_gone` below, which builds the
    /// one shape that actually gets there.
    #[test]
    fn closing_a_tab_strands_the_pane_that_was_detached_from_it() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let home = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "one".into(),
            },
            aux_pane(),
        )
        .expect("opens");
        // Widen that tab so one of its panes can leave.
        let t = tab_mut(&mut ws, id, home).expect("exists");
        let target = t.tree.focused;
        let extra =
            layout::split(&mut t.tree, target, Axis::Row, Side::After, aux_pane()).expect("splits");
        let label = detach_pane(&mut ws, id, home, extra).expect("detaches");

        close_tab(&mut ws, id, home, false).expect("closes");

        assert!(
            !ws.windows.contains_key(&label),
            "the detached window went with the tab it names as home"
        );
        assert!(
            project(&ws, id)
                .expect("exists")
                .detached
                .contains_key(&extra),
            "the pane itself survives — it holds a live session, so it is not destroyed here"
        );
        assert_eq!(
            redock_pane(&mut ws, &label),
            Err(CoreError::Invariant(format!(
                "window {label} is not a detached pane"
            ))),
            "with the window gone there is no gesture left that reaches the pane"
        );
        validate(&ws).expect("valid");
    }

    /// The console fallback in [`redock_pane`], and the claim its comment makes about anchors.
    ///
    /// No gesture produces this state — `close_tab` prunes the window along with the tab —
    /// but `ws.windows` is persisted, so a `workspace.json` hand-edited, half-written or
    /// carried over from a build that pruned differently can name a home tab that is gone.
    /// The branch exists for exactly that, and reaching it needs the state built directly.
    ///
    /// What is being checked is the reasoning the branch relies on: node ids are unique
    /// across the workspace, so the anchor from the vanished tab matches nothing in the
    /// console and the fallback is taken *without* a separate check for having landed
    /// somewhere else. If that ever stopped holding, a re-dock would rebuild a divider from
    /// another tab inside the console.
    #[test]
    fn a_pane_redocks_into_the_console_when_its_home_tab_is_gone() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let home = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "one".into(),
            },
            aux_pane(),
        )
        .expect("opens");
        let t = tab_mut(&mut ws, id, home).expect("exists");
        let target = t.tree.focused;
        let extra =
            layout::split(&mut t.tree, target, Axis::Row, Side::After, aux_pane()).expect("splits");
        let label = detach_pane(&mut ws, id, home, extra).expect("detaches");
        let anchor = project(&ws, id).expect("exists").dock_anchors[&extra].clone();

        let console_before = tab(&ws, id, console).expect("exists").tree.clone();
        // The tab goes away while the window entry stays: the file-shaped state, not one
        // `close_tab` can produce.
        let p = project_mut(&mut ws, id).expect("exists");
        p.tabs.retain(|t| t.id != home);
        // Through `set_active` and a `retain`, because that is what the *file* looks like:
        // `persist::load` runs `repair_tab_mru` on the way in, so a workspace that arrives with
        // a tab missing arrives with the focus order already free of it. Assigning `active_tab`
        // alone would build a state no loaded file can be in, and `validate` at the foot of
        // this test would rightly refuse it.
        p.tab_mru.retain(|t| *t != home);
        set_active(p, console);

        redock_pane(&mut ws, &label).expect("redocks into the console");

        let after = &tab(&ws, id, console).expect("exists").tree;
        assert!(
            after.panes.contains_key(&extra),
            "the pane landed in the console rather than being lost with its tab"
        );
        assert!(
            !collect_split_ids_of(&after.root).contains(&anchor.split),
            "the anchor named a divider in a tab that is gone; it must not be rebuilt here"
        );
        // Focus-adjacent, beside the console pane that held focus — the pre-anchor behaviour,
        // which is the right answer once the recorded position has stopped existing.
        assert_eq!(
            layout::leaves(&after.root),
            {
                let mut want = layout::leaves(&console_before.root);
                want.push(extra);
                want
            },
            "the console gained exactly the re-docked pane, at the end"
        );
        assert!(
            project(&ws, id).expect("exists").dock_anchors.is_empty(),
            "the anchor is spent whether or not it was usable"
        );
        validate(&ws).expect("valid");
    }

    /// The anchor map is a side map on `detached`, and nothing in the type system ties them
    /// together. An anchor for a pane that is back in a tab is not merely untidy: it names a
    /// sibling that is live again, so it would be *restorable*, and a future re-dock reading
    /// it would rebuild a split for a pane that never left.
    #[test]
    fn validate_rejects_an_anchor_for_a_pane_that_is_not_detached() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let extra = split_console(&mut ws, id);
        let label = detach_pane(&mut ws, id, console, extra).expect("detaches");
        let anchor = project(&ws, id).expect("exists").dock_anchors[&extra].clone();
        redock_pane(&mut ws, &label).expect("redocks");
        validate(&ws).expect("a spent anchor is removed with the pane it described");

        project_mut(&mut ws, id)
            .expect("exists")
            .dock_anchors
            .insert(extra, anchor);

        let Err(CoreError::Invariant(msg)) = validate(&ws) else {
            panic!("an anchor outliving its detachment must be reported");
        };
        assert!(msg.contains(&extra.to_string()));
    }

    #[test]
    fn detaching_the_only_pane_of_a_tab_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let only = tab(&ws, id, console).expect("exists").tree.focused;

        // Detaching would leave an empty tab, which the tree cannot represent. The console's
        // pane is Primary, so it reports that rather than the generic last-pane refusal.
        assert_eq!(
            detach_pane(&mut ws, id, console, only),
            Err(CoreError::PanePrimary)
        );
        validate(&ws).expect("unchanged and still valid");
    }

    #[test]
    fn opening_a_tab_in_an_unknown_project_reports_the_project() {
        let mut ws = Workspace::default();
        let ghost = ProjectId::new();

        assert_eq!(
            open_tab(&mut ws, ghost, TabKind::ClaudeHome, aux_pane()),
            Err(CoreError::NoSuchProject(ghost))
        );
    }

    #[test]
    fn reordering_refuses_to_move_the_console_tab() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "one".into(),
            },
            aux_pane(),
        )
        .expect("opens");

        assert_eq!(reorder_tab(&mut ws, id, 0, 1), Err(CoreError::TabPinned));
        assert_eq!(reorder_tab(&mut ws, id, 1, 0), Err(CoreError::TabPinned));
        assert!(matches!(
            project(&ws, id).expect("exists").tabs[0].kind,
            TabKind::ClaudeHome
        ));
    }

    #[test]
    fn reordering_a_tab_out_of_range_reports_the_index() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        assert_eq!(
            reorder_tab(&mut ws, id, 1, 0),
            Err(CoreError::IndexOutOfRange { index: 1, len: 1 })
        );
    }

    /// [`revision_chain`]'s three rules, and the diamond that is the reason for the third. (M18)
    #[test]
    fn a_revision_chain_keeps_its_order_drops_its_own_revision_and_never_repeats_one() {
        let chain = |rev: &str, from: &[&str]| {
            revision_chain(
                rev,
                &from.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
            )
        };

        // Order is untouched: newest last, exactly as the field is documented and as the strip
        // draws it. This is the ordinary case and it must be the identity.
        assert_eq!(chain("ccc", &["aaa", "bbb"]), vec!["aaa", "bbb"]);
        assert_eq!(chain("aaa", &[]), Vec::<String>::new());

        // The tab's own revision is not part of the walk that led to it. Reachable the moment a
        // user walks back and then forward again through a crumb.
        assert_eq!(chain("bbb", &["aaa", "bbb"]), vec!["aaa"]);

        // The diamond. `bbb` is reached down one parent of the merge and again down the other;
        // without the dedupe this chain grows by one entry per lap, on disk.
        assert_eq!(
            chain("ddd", &["aaa", "bbb", "ccc", "bbb", "aaa"]),
            vec!["aaa", "bbb", "ccc"],
            "and the EARLIEST occurrence is the one kept, so the crumbs stay in the order the \
             user walked them rather than being reshuffled behind them"
        );

        // Idempotent: a chain that has already been normalised is its own answer, which is what
        // makes it safe to run on the `from` of a tab that is being extended.
        let once = chain("ddd", &["aaa", "bbb", "ccc", "bbb"]);
        let twice = revision_chain("ddd", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn reordering_tabs_moves_the_tab_and_nothing_else() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let a = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull { title: "a".into() },
            aux_pane(),
        )
        .expect("opens");
        let b = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull { title: "b".into() },
            aux_pane(),
        )
        .expect("opens");

        // `open_tab` inserts at 1, so the strip starts as `[console, b, a]`; moving index 2 to
        // index 1 swaps them back.
        assert_eq!(
            {
                let tabs = &project(&ws, id).expect("exists").tabs;
                (tabs[1].id, tabs[2].id)
            },
            (b, a),
            "newest first, before anything is dragged"
        );

        reorder_tab(&mut ws, id, 2, 1).expect("a full tab moves");
        let tabs = &project(&ws, id).expect("exists").tabs;
        assert_eq!((tabs[1].id, tabs[2].id), (a, b));
        validate(&ws).expect("still valid");
    }

    #[test]
    fn projects_reorder_within_the_header() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");

        reorder_project(&mut ws, 1, 0).expect("in range");
        assert_eq!(ws.projects.keys().copied().collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(
            reorder_project(&mut ws, 0, 7),
            Err(CoreError::IndexOutOfRange { index: 7, len: 2 })
        );
        validate(&ws).expect("the stacked window followed the reorder");
    }

    #[test]
    fn removing_the_last_root_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");

        assert_eq!(
            remove_root(&mut ws, id, Path::new("/home/dev/work/cide")),
            Err(CoreError::NoRoots)
        );
        assert_eq!(project(&ws, id).expect("exists").roots.len(), 1);
    }

    #[test]
    fn removing_the_primary_root_recomputes_the_display_path() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        add_root(&mut ws, id, PathBuf::from("/srv/vendor")).expect("adds");

        remove_root(&mut ws, id, Path::new("/home/dev/work/cide")).expect("removes");
        let p = project(&ws, id).expect("exists");
        assert_eq!(p.roots.len(), 1);
        assert_eq!(p.display_path, display_path_of(Path::new("/srv/vendor")));
        validate(&ws).expect("display path and root agree");
    }

    #[test]
    fn adding_a_root_twice_leaves_one_copy() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");

        add_root(&mut ws, id, PathBuf::from("/srv/vendor")).expect("adds");
        let rev = ws.rev;
        add_root(&mut ws, id, PathBuf::from("/srv/vendor")).expect("is a no-op");

        assert_eq!(project(&ws, id).expect("exists").roots.len(), 2);
        assert_eq!(ws.rev, rev, "a no-op does not advance the revision");
    }

    #[test]
    fn removing_an_absent_root_changes_nothing() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        add_root(&mut ws, id, PathBuf::from("/srv/vendor")).expect("adds");
        let rev = ws.rev;

        remove_root(&mut ws, id, Path::new("/nowhere")).expect("is a no-op");
        assert_eq!(project(&ws, id).expect("exists").roots.len(), 2);
        assert_eq!(ws.rev, rev);
    }

    #[test]
    fn a_root_takes_its_label_from_its_final_component() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        add_root(&mut ws, id, PathBuf::from("/srv/vendor/libs")).expect("adds");
        assert_eq!(project(&ws, id).expect("exists").roots[1].label, "libs");
    }

    #[test]
    fn stacked_mode_puts_every_project_in_one_shell_window() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");

        assert_eq!(ws.windows.len(), 1);
        let Some(WindowRole::Shell { projects, active }) = ws.windows.values().next() else {
            panic!("stacked mode builds one shell window");
        };
        assert_eq!(projects, &vec![a, b]);
        assert_eq!(*active, Some(a));
        validate(&ws).expect("valid");
    }

    #[test]
    fn per_project_mode_gives_each_project_its_own_window() {
        let mut ws = Workspace::default();
        open(&mut ws, "/home/dev/a");
        open(&mut ws, "/home/dev/b");

        set_window_mode(&mut ws, WindowMode::PerProject).expect("mode flips");
        assert_eq!(ws.windows.len(), 2);
        for role in ws.windows.values() {
            let WindowRole::Shell { projects, .. } = role else {
                panic!("only shells here");
            };
            assert_eq!(projects.len(), 1);
        }
        validate(&ws).expect("valid");
    }

    #[test]
    fn flipping_the_window_mode_leaves_every_project_untouched() {
        let mut ws = demo_workspace();
        let before = serde_json::to_string(&ws.projects).expect("serialises");

        set_window_mode(&mut ws, WindowMode::PerProject).expect("flips");
        let flipped = serde_json::to_string(&ws.projects).expect("serialises");
        set_window_mode(&mut ws, WindowMode::Stacked).expect("flips back");
        let after = serde_json::to_string(&ws.projects).expect("serialises");

        assert_eq!(before, flipped, "no project changes when the mode changes");
        assert_eq!(before, after);
        validate(&ws).expect("valid after a round trip");
    }

    /// M5's stated check: three projects, six live sessions, flip the mode, zero deaths.
    ///
    /// The guarantee has two halves and only one of them is here. This half is that the flip
    /// is a *window* operation: it rewrites `ws.windows` and touches no project, tab, pane or
    /// session binding, so every `SessionId` a pane held before the flip it still holds
    /// after. The other half — that destroying an OS window does not kill the child behind
    /// it — is structural rather than testable at this layer: sessions are owned by a
    /// process-global registry, and `windows::destroy` closes a webview and nothing else.
    /// That is the whole reason the registry exists.
    #[test]
    fn flipping_the_mode_with_live_sessions_loses_none_of_them() {
        let mut ws = Workspace::default();
        let mut expected: Vec<(PaneId, SessionId)> = Vec::new();

        for path in ["/home/dev/a", "/home/dev/b", "/home/dev/c"] {
            let project = open(&mut ws, path);
            let console = console_tab(&ws, project).expect("every project has a console");

            // Two panes per project, both bound: the console's own pane and a split.
            let second = split_console(&mut ws, project);
            let t = tab_mut(&mut ws, project, console).expect("exists");
            let ids: Vec<PaneId> = t.tree.panes.keys().copied().collect();
            assert_eq!(ids.len(), 2, "the console should now hold two panes");

            for pane in ids {
                let session = SessionId::new();
                t.tree
                    .panes
                    .get_mut(&pane)
                    .expect("just enumerated")
                    .session = Some(session);
                expected.push((pane, session));
            }
            let _ = second;
        }
        assert_eq!(expected.len(), 6, "three projects, six live sessions");

        let bindings = |ws: &Workspace| -> Vec<(PaneId, SessionId)> {
            let mut found: Vec<_> = ws
                .projects
                .values()
                .flat_map(|p| p.tabs.iter())
                .flat_map(|t| t.tree.panes.iter())
                .filter_map(|(id, pane)| pane.session.map(|s| (*id, s)))
                .collect();
            found.sort();
            found
        };

        let mut before = expected.clone();
        before.sort();
        assert_eq!(bindings(&ws), before, "setup did not bind what it meant to");

        set_window_mode(&mut ws, WindowMode::PerProject).expect("flips");
        assert_eq!(ws.windows.len(), 3, "one window per project");
        assert_eq!(
            bindings(&ws),
            before,
            "a flip to PerProject moved a session"
        );
        validate(&ws).expect("valid in PerProject");

        set_window_mode(&mut ws, WindowMode::Stacked).expect("flips back");
        assert_eq!(ws.windows.len(), 1, "back to a single shell");
        assert_eq!(
            bindings(&ws),
            before,
            "a flip back to Stacked moved a session"
        );
        validate(&ws).expect("valid in Stacked");
    }

    #[test]
    fn a_detached_window_survives_a_mode_flip() {
        // Detached windows are orthogonal to the mode — they show one pane, not a project
        // mapping — so a flip must carry them across untouched. Dropping one would strand a
        // live session with a window role naming a pane nothing renders.
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/home/dev/a");
        let console = console_tab(&ws, project).expect("exists");
        let pane = split_console(&mut ws, project);
        let label = detach_pane(&mut ws, project, console, pane).expect("detaches");

        assert!(ws.windows.contains_key(&label));
        set_window_mode(&mut ws, WindowMode::PerProject).expect("flips");
        assert!(
            ws.windows.contains_key(&label),
            "the detached window was dropped by a mode flip"
        );
        assert!(
            ws.projects[&project].detached.contains_key(&pane),
            "the detached pane lost its holding entry"
        );
        validate(&ws).expect("valid");
    }

    #[test]
    fn stacked_mode_keeps_the_active_project_it_already_had() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");

        // Nothing in this module activates a project, so the setting is made directly, as
        // `cide-app` does when the user switches header tab.
        let label = ws.windows.keys().next().cloned().expect("one shell");
        ws.windows.insert(
            label,
            WindowRole::Shell {
                projects: vec![a, b],
                active: Some(b),
            },
        );

        open(&mut ws, "/home/dev/c");
        let Some(WindowRole::Shell { active, .. }) = ws.windows.values().next() else {
            panic!("one shell window");
        };
        assert_eq!(
            *active,
            Some(b),
            "opening a project does not steal the focus"
        );
    }

    #[test]
    fn per_project_mode_has_no_memory_of_which_project_was_focused() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");

        set_window_mode(&mut ws, WindowMode::PerProject).expect("flips");
        // Each per-project shell is active on its own project, so flipping back can only
        // pick the first. Cross-window focus memory is deferred; this pins the behaviour
        // so that adding it is a deliberate change rather than a silent one.
        set_window_mode(&mut ws, WindowMode::Stacked).expect("flips back");
        let Some(WindowRole::Shell { active, .. }) = ws.windows.values().next() else {
            panic!("one shell window");
        };
        assert_eq!(*active, Some(a));
        assert_ne!(*active, Some(b));
    }

    #[test]
    fn setting_the_current_window_mode_is_a_no_op() {
        let mut ws = Workspace::default();
        open(&mut ws, "/home/dev/a");
        let labels: Vec<_> = ws.windows.keys().cloned().collect();
        let rev = ws.rev;

        set_window_mode(&mut ws, WindowMode::Stacked).expect("already stacked");
        assert_eq!(ws.windows.keys().cloned().collect::<Vec<_>>(), labels);
        assert_eq!(ws.rev, rev);
    }

    #[test]
    fn the_stacked_window_keeps_its_label_across_a_project_opening() {
        let mut ws = Workspace::default();
        open(&mut ws, "/home/dev/a");
        let label = ws.windows.keys().next().cloned().expect("one window");

        open(&mut ws, "/home/dev/b");
        assert_eq!(ws.windows.keys().next(), Some(&label));
    }

    #[test]
    fn closing_a_project_drops_its_detached_windows() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");
        let detached = WindowLabel::detached_tab();
        ws.windows.insert(
            detached.clone(),
            WindowRole::DetachedTab {
                project: a,
                tab: console_tab(&ws, a).expect("exists"),
            },
        );

        close_project(&mut ws, a, false).expect("closes");
        assert!(!ws.windows.contains_key(&detached));
        assert!(ws.projects.contains_key(&b));
        validate(&ws).expect("valid");
    }

    #[test]
    fn closing_a_project_keeps_another_projects_detached_window() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let b = open(&mut ws, "/home/dev/b");
        let detached = WindowLabel::detached_tab();
        ws.windows.insert(
            detached.clone(),
            WindowRole::DetachedTab {
                project: b,
                tab: console_tab(&ws, b).expect("exists"),
            },
        );

        close_project(&mut ws, a, false).expect("closes");
        assert!(ws.windows.contains_key(&detached));
        validate(&ws).expect("valid");
    }

    #[test]
    fn closing_the_last_project_keeps_an_empty_shell() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");

        close_project(&mut ws, a, false).expect("closes");

        // Dropping the window would leave the app with nothing on screen. What the user
        // should see is the empty frame with a `+` in its header, which is a shell holding
        // no projects — the same state as first launch.
        let Some(WindowRole::Shell { projects, active }) = ws.windows.values().next() else {
            panic!("the shell window should survive its last project");
        };
        assert!(projects.is_empty());
        assert_eq!(*active, None);
        validate(&ws).expect("an empty workspace is valid");
    }

    #[test]
    fn a_shell_holding_projects_must_have_one_active() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/home/dev/a");
        let label = ws.windows.keys().next().expect("a shell exists").clone();
        ws.windows.insert(
            label,
            WindowRole::Shell {
                projects: vec![a],
                active: None,
            },
        );

        assert!(matches!(validate(&ws), Err(CoreError::Invariant(_))));
    }

    #[test]
    fn unknown_ids_are_reported_as_such() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let ghost_project = ProjectId::new();
        let ghost_tab = TabId::new();

        assert_eq!(
            project(&ws, ghost_project),
            Err(CoreError::NoSuchProject(ghost_project))
        );
        assert_eq!(
            close_project(&mut ws, ghost_project, false),
            Err(CoreError::NoSuchProject(ghost_project))
        );
        assert_eq!(
            tab(&ws, id, ghost_tab).err(),
            Some(CoreError::NoSuchTab(ghost_tab))
        );
        assert_eq!(
            activate_tab(&mut ws, id, ghost_tab),
            Err(CoreError::NoSuchTab(ghost_tab))
        );
        assert_eq!(
            close_tab(&mut ws, id, ghost_tab, false),
            Err(CoreError::NoSuchTab(ghost_tab))
        );
    }

    #[test]
    fn activating_a_tab_makes_it_current() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let full = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull { title: "a".into() },
            aux_pane(),
        )
        .expect("opens");

        activate_tab(&mut ws, id, console).expect("activates");
        assert_eq!(project(&ws, id).expect("exists").active_tab, console);
        activate_tab(&mut ws, id, full).expect("activates");
        assert_eq!(project(&ws, id).expect("exists").active_tab, full);
    }

    #[test]
    fn every_successful_mutation_advances_the_revision() {
        let mut ws = Workspace::default();
        assert_eq!(ws.rev, 0);

        let id = open(&mut ws, "/home/dev/work/cide");
        assert_eq!(ws.rev, 1);
        let full = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull { title: "a".into() },
            aux_pane(),
        )
        .expect("opens");
        assert_eq!(ws.rev, 2);
        close_tab(&mut ws, id, full, false).expect("closes");
        assert_eq!(ws.rev, 3);
        assert_eq!(bump(&mut ws), 4);
    }

    #[test]
    fn find_pane_names_the_project_and_tab_that_own_it() {
        let ws = demo_workspace();
        let (id, p) = ws.projects.first().expect("a demo project");
        let t = p.tabs.last().expect("a tab");
        let pane = *t.tree.panes.keys().next().expect("a pane");

        assert_eq!(find_pane(&ws, pane), Some((*id, t.id)));
        assert_eq!(find_pane(&ws, PaneId::new()), None);
    }

    #[test]
    fn the_demo_workspace_has_three_projects_and_twelve_panes() {
        let ws = demo_workspace();
        assert_eq!(ws.projects.len(), 3);
        assert_eq!(pane_count(&ws), 12);

        let first = ws.projects.values().next().expect("a project");
        assert_eq!(first.roots.len(), 2, "the first project is multi-root");
        assert!(
            depth(&first.tabs[0].tree.root) >= 4,
            "the console nests three splits deep"
        );
        assert!(
            first
                .tabs
                .iter()
                .any(|t| matches!(t.kind, TabKind::ClaudeFull { .. }))
        );
        assert!(
            first
                .tabs
                .iter()
                .any(|t| matches!(t.kind, TabKind::File { .. }))
        );
    }

    #[test]
    fn the_demo_workspace_satisfies_every_invariant() {
        validate(&demo_workspace()).expect("the fixture is valid");
    }

    #[test]
    fn validate_rejects_a_project_whose_first_tab_is_not_the_console() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull { title: "a".into() },
            aux_pane(),
        )
        .expect("opens");
        project_mut(&mut ws, id).expect("exists").tabs.swap(0, 1);

        assert_eq!(validate(&ws), Err(CoreError::TabPinned));
    }

    #[test]
    fn validate_rejects_a_dangling_active_tab() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let ghost = TabId::new();
        project_mut(&mut ws, id).expect("exists").active_tab = ghost;

        assert_eq!(validate(&ws), Err(CoreError::NoSuchTab(ghost)));
    }

    #[test]
    fn validate_rejects_a_rootless_project() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        project_mut(&mut ws, id).expect("exists").roots.clear();

        assert_eq!(validate(&ws), Err(CoreError::NoRoots));
    }

    #[test]
    fn a_stale_display_path_does_not_make_a_workspace_invalid() {
        // It is a rendering of `roots[0]` through `$HOME`, so a file written on another
        // machine or read under another user legitimately carries a different string.
        // Rejecting it would discard the user's whole layout over a cosmetic mismatch.
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        project_mut(&mut ws, id).expect("exists").display_path = "~someone-else/work/cide".into();

        assert_eq!(validate(&ws), Ok(()));
    }

    #[test]
    fn refresh_display_paths_recomputes_from_the_primary_root() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let expected = project(&ws, id).expect("exists").display_path.clone();
        project_mut(&mut ws, id).expect("exists").display_path = "~someone-else/work/cide".into();

        refresh_display_paths(&mut ws);

        assert_eq!(project(&ws, id).expect("exists").display_path, expected);
    }

    #[test]
    fn opening_a_tab_with_a_live_pane_id_is_refused() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = project(&ws, id).expect("exists").tabs[0].clone();
        let (&live, _) = console.tree.panes.first().expect("the console has a pane");

        let mut clash = aux_pane();
        clash.id = live;

        // Left to run, this builds a workspace that `validate` then calls corrupt.
        assert!(matches!(
            open_tab(
                &mut ws,
                id,
                TabKind::ClaudeFull { title: "x".into() },
                clash
            ),
            Err(CoreError::Invariant(_))
        ));
        assert_eq!(validate(&ws), Ok(()));
    }

    #[test]
    fn validate_rejects_a_window_on_a_project_that_is_gone() {
        let mut ws = Workspace::default();
        open(&mut ws, "/home/dev/work/cide");
        let ghost = ProjectId::new();
        let label = ws.windows.keys().next().cloned().expect("one window");
        ws.windows.insert(
            label,
            WindowRole::Shell {
                projects: vec![ghost],
                active: Some(ghost),
            },
        );

        assert_eq!(validate(&ws), Err(CoreError::NoSuchProject(ghost)));
    }

    #[test]
    fn validate_rejects_a_stacked_workspace_with_a_window_per_project() {
        let mut ws = Workspace::default();
        open(&mut ws, "/home/dev/a");
        open(&mut ws, "/home/dev/b");
        set_window_mode(&mut ws, WindowMode::PerProject).expect("flips");
        // The mode is the only thing put back, so the windows now contradict it.
        ws.settings.window_mode = WindowMode::Stacked;

        assert!(matches!(validate(&ws), Err(CoreError::Invariant(_))));
    }

    #[test]
    fn validate_rejects_a_pane_that_appears_in_two_tabs() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let console = console_tab(&ws, id).expect("exists");
        let stolen = tab(&ws, id, console).expect("exists").tree.clone();

        let full = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull { title: "a".into() },
            aux_pane(),
        )
        .expect("opens");
        tab_mut(&mut ws, id, full).expect("exists").tree = stolen;

        assert!(matches!(validate(&ws), Err(CoreError::Invariant(_))));
    }

    /// A git diff tab over one repo-relative path, kept or preview.
    fn open_diff(
        ws: &mut Workspace,
        project: ProjectId,
        path: &str,
        preview: bool,
    ) -> (TabId, DiffSpec) {
        let spec = git_spec(path);
        let id = open_tab(
            ws,
            project,
            TabKind::Diff {
                spec: spec.clone(),
                preview,
            },
            demo_pane(PaneKind::Diff, &spec.title, false),
        )
        .expect("a diff tab opens");
        (id, spec)
    }

    /// The shape `cmd::file::git_diff_spec` produces, without depending on the app crate.
    fn git_spec(path: &str) -> DiffSpec {
        DiffSpec {
            title: format!("{path} — diff"),
            old_path: PathBuf::from(path),
            new_path: PathBuf::from(path),
            origin: DiffOrigin::Git {
                repo: cide_ipc::RepoId::new(),
                path: path.to_owned(),
                side: cide_ipc::git::DiffSide::Combined,
            },
        }
    }

    fn kind_of(ws: &Workspace, project: ProjectId, id: TabId) -> TabKind {
        tab(ws, project, id).expect("exists").kind.clone()
    }

    /// A file tab's path as a string, which is how every consumer in the webview reads it.
    fn path_of(ws: &Workspace, project: ProjectId, id: TabId) -> String {
        let TabKind::File { path, .. } = kind_of(ws, project, id) else {
            panic!("not a file tab");
        };
        path.to_string_lossy().into_owned()
    }

    /// The operation the user asked for, at the level that owns it: the tab changes, the
    /// strip does not grow.
    #[test]
    fn retargeting_a_diff_tab_changes_its_spec_and_creates_no_tab() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let (id, _) = open_diff(&mut ws, project, "src/a.rs", true);
        let before = super::project(&ws, project).expect("exists").tabs.len();
        let rev = ws.rev;

        let wanted = git_spec("src/b.rs");
        retarget_diff(&mut ws, project, id, wanted.clone()).expect("retargets");

        assert_eq!(
            super::project(&ws, project).expect("exists").tabs.len(),
            before,
            "no new tab"
        );
        assert_eq!(
            kind_of(&ws, project, id),
            TabKind::Diff {
                spec: wanted,
                preview: true,
            },
            "same tab, pointed somewhere else, still the scratch slot"
        );
        assert!(
            ws.rev > rev,
            "a retarget is news; every window has to redraw"
        );
    }

    /// The pane header is written from the spec at open time, so a retarget that skipped it
    /// would leave "a.rs — diff" over the diff of `b.rs`.
    #[test]
    fn retargeting_renames_the_diff_panes_and_leaves_the_tree_alone() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let (id, _) = open_diff(&mut ws, project, "src/a.rs", true);
        let tree_before = tab(&ws, project, id).expect("exists").tree.clone();

        retarget_diff(&mut ws, project, id, git_spec("src/b.rs")).expect("retargets");

        let t = tab(&ws, project, id).expect("exists");
        assert_eq!(t.kind.title(), "src/b.rs — diff");
        assert!(
            t.tree
                .panes
                .values()
                .all(|p| p.kind != PaneKind::Diff || p.title == "src/b.rs — diff")
        );
        assert_eq!(
            t.tree.panes.keys().collect::<Vec<_>>(),
            tree_before.panes.keys().collect::<Vec<_>>(),
            "the pane ids are the addresses every later command uses; they do not move"
        );
    }

    /// A diff pane torn into its own window is renamed too.
    ///
    /// `detach_pane` takes the pane *out* of the tab's tree and parks it in
    /// `project.detached`, so the rename loop over `tree.panes` cannot see it — and
    /// `DetachedPaneWindow` draws its header from exactly that parked copy. A diff pulled out
    /// into a window and then clicked past would otherwise keep the title it was torn out on
    /// for ever, which is the same stale header the in-tree rename exists to prevent.
    ///
    /// A diff pane is `PaneRole::Auxiliary`, and `PaneTitleBar` offers Detach on every pane
    /// that is not `Primary` — so this is a gesture the product actually has, not a state
    /// reachable only from a test.
    #[test]
    fn retargeting_renames_a_diff_pane_that_was_torn_into_its_own_window() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let (id, _) = open_diff(&mut ws, project, "src/a.rs", true);

        // A second pane, because `take_pane` refuses to detach a tab's last one.
        let torn = {
            let t = tab_mut(&mut ws, project, id).expect("exists");
            let target = t.tree.focused;
            layout::split(
                &mut t.tree,
                target,
                Axis::Row,
                Side::After,
                demo_pane(PaneKind::Diff, "src/a.rs — diff", false),
            )
            .expect("splits")
        };
        detach_pane(&mut ws, project, id, torn).expect("detaches");

        retarget_diff(&mut ws, project, id, git_spec("src/b.rs")).expect("retargets");

        assert_eq!(
            super::project(&ws, project).expect("exists").detached[&torn].title,
            "src/b.rs — diff",
            "the window torn out of this tab names the file the tab now shows",
        );
        assert!(
            tab(&ws, project, id)
                .expect("exists")
                .tree
                .panes
                .values()
                .all(|p| p.kind != PaneKind::Diff || p.title == "src/b.rs — diff"),
            "and the panes still in the tree are renamed as before",
        );
    }

    /// Clicking the row whose diff is already up must not cost a broadcast: the panel calls
    /// this on the first and last click of every walk down a changelist.
    #[test]
    fn retargeting_a_tab_onto_what_it_already_shows_is_not_news() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let (id, spec) = open_diff(&mut ws, project, "src/a.rs", true);
        let rev = ws.rev;

        retarget_diff(&mut ws, project, id, spec).expect("retargets");

        assert_eq!(ws.rev, rev);
    }

    /// A Claude diff tab is the visible half of a blocked agent turn. Re-pointing it would
    /// leave the CLI waiting on a question nobody can see any more.
    #[test]
    fn a_claude_diff_tab_refuses_to_be_retargeted() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let spec = DiffSpec {
            title: "main.rs".into(),
            old_path: PathBuf::from("/repo/src/main.rs"),
            new_path: PathBuf::from("/repo/src/main.rs"),
            origin: DiffOrigin::ClaudeMcp {
                request_id: "req-1".into(),
            },
        };
        let id = open_tab(
            &mut ws,
            project,
            TabKind::Diff {
                spec: spec.clone(),
                preview: false,
            },
            demo_pane(PaneKind::Diff, "main.rs", false),
        )
        .expect("opens");
        let rev = ws.rev;

        let refused = retarget_diff(&mut ws, project, id, git_spec("src/other.rs"));

        assert!(matches!(refused, Err(CoreError::Invariant(_))));
        assert_eq!(
            kind_of(&ws, project, id),
            TabKind::Diff {
                spec,
                preview: false
            }
        );
        assert_eq!(ws.rev, rev, "a refusal leaves the revision alone");
    }

    /// There is nothing to retarget about a terminal.
    #[test]
    fn a_tab_that_is_not_a_diff_refuses_both_halves() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let file = open_file(&mut ws, project, "/repo/src/main.rs", false);

        assert!(matches!(
            retarget_diff(&mut ws, project, file, git_spec("src/a.rs")),
            Err(CoreError::Invariant(_))
        ));
        assert!(matches!(
            promote_diff(&mut ws, project, file),
            Err(CoreError::Invariant(_))
        ));
    }

    /// A diff tab cannot be dirty today — only a `File` tab can — but retargeting *is* a
    /// discard, so it asks the same question `close_tab` asks rather than assuming the
    /// answer. Pinned here so the guard survives a diff pane that one day holds an edit.
    #[test]
    fn retargeting_asks_the_same_unsaved_question_close_tab_asks() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let (id, _) = open_diff(&mut ws, project, "src/a.rs", true);

        assert!(unsaved_in_tab(&ws, project, id).expect("exists").is_none());
        retarget_diff(&mut ws, project, id, git_spec("src/b.rs")).expect("nothing to lose");
    }

    /// At most one scratch slot, and promotion empties it.
    #[test]
    fn the_preview_slot_holds_one_tab_and_promotion_frees_it() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let working = PreviewSlot::Working;
        assert_eq!(
            preview_diff_tab(&ws, project, working).expect("exists"),
            None
        );

        let (kept, _) = open_diff(&mut ws, project, "src/keep.rs", false);
        assert_eq!(
            preview_diff_tab(&ws, project, working).expect("exists"),
            None
        );

        let (preview, _) = open_diff(&mut ws, project, "src/scratch.rs", true);
        assert_eq!(
            preview_diff_tab(&ws, project, working).expect("exists"),
            Some(preview)
        );

        let rev = ws.rev;
        promote_diff(&mut ws, project, kept).expect("already kept");
        assert_eq!(
            ws.rev, rev,
            "promoting a kept tab is a no-op, not a broadcast"
        );

        promote_diff(&mut ws, project, preview).expect("promotes");
        assert_eq!(
            preview_diff_tab(&ws, project, working).expect("exists"),
            None
        );
        assert!(ws.rev > rev);
    }

    /// The shape `cmd::file::revision_diff_spec` produces, without depending on the app crate.
    fn revision_spec(path: &str, oid: &str) -> DiffSpec {
        DiffSpec {
            title: format!("{path} @ {oid}"),
            old_path: PathBuf::from(path),
            new_path: PathBuf::from(path),
            origin: DiffOrigin::GitRevision {
                repo: cide_ipc::RepoId::new(),
                path: path.to_owned(),
                new: cide_ipc::history::RevSide::Commit {
                    oid: oid.to_owned(),
                },
                old: cide_ipc::history::RevSide::FirstParent,
            },
        }
    }

    /// Two slots, and the git panel's scratch tab is not the tool window's.
    ///
    /// The failure this pins is the one that motivated [`PreviewSlot`]: with a single slot, a
    /// click in the log's file list retargets the tab the user was staging from, and the
    /// half-made selection in it goes with it.
    #[test]
    fn a_revision_preview_and_a_working_preview_are_different_tabs() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let (working, _) = open_diff(&mut ws, project, "src/a.rs", true);
        let spec = revision_spec("src/b.rs", "a1b2c3d");
        let revision = open_tab(
            &mut ws,
            project,
            TabKind::Diff {
                spec: spec.clone(),
                preview: true,
            },
            demo_pane(PaneKind::Diff, &spec.title, false),
        )
        .expect("opens");

        assert_eq!(
            preview_diff_tab(&ws, project, PreviewSlot::Working).expect("exists"),
            Some(working)
        );
        assert_eq!(
            preview_diff_tab(&ws, project, PreviewSlot::Revision).expect("exists"),
            Some(revision)
        );
    }

    /// The fourth refusal: a revision tab is read-only and a working-tree tab has a Stage
    /// footer, so neither may be re-pointed at the other's kind of diff.
    #[test]
    fn a_diff_tab_refuses_to_be_retargeted_across_families() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let (working, working_spec) = open_diff(&mut ws, project, "src/a.rs", true);
        let rev = ws.rev;

        let refused = retarget_diff(&mut ws, project, working, revision_spec("src/b.rs", "dead"));
        assert!(matches!(refused, Err(CoreError::Invariant(_))));
        assert_eq!(
            kind_of(&ws, project, working),
            TabKind::Diff {
                spec: working_spec,
                preview: true
            },
            "a refusal changes nothing about the tab"
        );
        assert_eq!(ws.rev, rev, "a refusal leaves the revision alone");

        // And the other direction, which is the one that would put a Stage button over two
        // commits.
        let spec = revision_spec("src/c.rs", "a1b2c3d");
        let revision = open_tab(
            &mut ws,
            project,
            TabKind::Diff {
                spec: spec.clone(),
                preview: true,
            },
            demo_pane(PaneKind::Diff, &spec.title, false),
        )
        .expect("opens");
        assert!(matches!(
            retarget_diff(&mut ws, project, revision, git_spec("src/d.rs")),
            Err(CoreError::Invariant(_))
        ));
        assert_eq!(
            kind_of(&ws, project, revision),
            TabKind::Diff {
                spec,
                preview: true
            }
        );

        // Within a family it still works, which is the half that must not be broken by the
        // check above.
        retarget_diff(
            &mut ws,
            project,
            revision,
            revision_spec("src/e.rs", "beef123"),
        )
        .expect("same family, so it retargets");
    }

    /*
     * Renaming a file the user has open. (M25)
     *
     * The tab is the thing that has to move: `cide_core::document::write` canonicalizes before it
     * writes, so a tab left on the old name is a buffer that cannot be saved at all — which is
     * the report this came from, and it is invisible from inside `fs_rename`, whose own answer
     * (the file moved) was correct.
     */
    #[test]
    fn renaming_an_open_file_moves_its_tab_and_its_pane_header() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let id = open_file(&mut ws, project, "/repo/src/old.rs", true);
        let before = ws.rev;

        retarget_paths(
            &mut ws,
            &[(
                PathBuf::from("/repo/src/old.rs"),
                PathBuf::from("/repo/src/new.rs"),
            )],
        );

        assert_eq!(
            kind_of(&ws, project, id),
            TabKind::File {
                path: PathBuf::from("/repo/src/new.rs"),
                // Still dirty. The rename moved the file, not the buffer: the edits are in the
                // editor and are still unsaved, and clearing this would offer to close the tab
                // without a word about them.
                dirty: true,
            },
        );
        assert!(
            tab(&ws, project, id)
                .expect("exists")
                .tree
                .panes
                .values()
                .all(|p| p.title == "new.rs"),
            "the pane header names the file the buffer is now over",
        );
        assert!(ws.rev > before, "every window has to be told");
    }

    /// Renaming a *folder* takes every tab under it. The prefix case is the one that gets
    /// forgotten, and it fails in exactly the same way as the file case.
    #[test]
    fn renaming_a_folder_moves_the_tabs_beneath_it() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        let deep = open_file(&mut ws, project, "/repo/src/net/http.rs", false);
        let sibling = open_file(&mut ws, project, "/repo/src/main.rs", false);
        // The near miss: a sibling whose path *starts with the same characters* and is not
        // inside the folder. `strip_prefix` matches whole components, which is what keeps it out.
        let decoy = open_file(&mut ws, project, "/repo/src/network.md", false);

        retarget_paths(
            &mut ws,
            &[(
                PathBuf::from("/repo/src/net"),
                PathBuf::from("/repo/src/io"),
            )],
        );

        assert_eq!(path_of(&ws, project, deep), "/repo/src/io/http.rs");
        assert_eq!(path_of(&ws, project, sibling), "/repo/src/main.rs");
        assert_eq!(path_of(&ws, project, decoy), "/repo/src/network.md");
    }

    /// The same file open in two projects moves in both. The path is absolute and the file moved
    /// on disk; the project that happened to issue the rename has nothing to do with it.
    #[test]
    fn a_rename_reaches_every_project_holding_the_file() {
        let mut ws = Workspace::default();
        let one = open(&mut ws, "/repo");
        let two = open(&mut ws, "/other");
        let here = open_file(&mut ws, one, "/repo/src/old.rs", false);
        let there = open_file(&mut ws, two, "/repo/src/old.rs", false);

        retarget_paths(
            &mut ws,
            &[(
                PathBuf::from("/repo/src/old.rs"),
                PathBuf::from("/repo/src/new.rs"),
            )],
        );

        assert_eq!(path_of(&ws, one, here), "/repo/src/new.rs");
        assert_eq!(path_of(&ws, two, there), "/repo/src/new.rs");
    }

    /// The common rename is of a file nobody has open, and it must not cost a broadcast: the
    /// caller only emits when this says something moved.
    #[test]
    fn renaming_a_file_no_tab_names_is_not_news() {
        let mut ws = Workspace::default();
        let project = open(&mut ws, "/repo");
        open_file(&mut ws, project, "/repo/src/main.rs", false);
        let before = ws.rev;

        retarget_paths(
            &mut ws,
            &[(
                PathBuf::from("/repo/README.md"),
                PathBuf::from("/repo/READ.md"),
            )],
        );
        assert_eq!(ws.rev, before, "no tab moved, so no mirror is stale");
    }

    /// A retargeted path is compared as a *string* by everything in the webview that reads it,
    /// so the exact case must not come back with `to.join("")`'s trailing separator.
    #[test]
    fn a_moved_path_is_spelled_the_way_the_frontend_will_compare_it() {
        assert_eq!(
            moved_path(
                Path::new("/repo/src/old.rs"),
                Path::new("/repo/src/old.rs"),
                Path::new("/repo/src/new.rs"),
            )
            .expect("the file itself moved")
            .to_string_lossy(),
            "/repo/src/new.rs",
        );
        assert_eq!(
            moved_path(
                Path::new("/repo/src/net/http.rs"),
                Path::new("/repo/src/net"),
                Path::new("/repo/src/io"),
            )
            .expect("a folder above it moved")
            .to_string_lossy(),
            "/repo/src/io/http.rs",
        );
        assert_eq!(
            moved_path(
                Path::new("/repo/src/network.md"),
                Path::new("/repo/src/net"),
                Path::new("/repo/src/io"),
            ),
            None,
            "a name that merely starts the same is not inside the folder",
        );
    }
}
