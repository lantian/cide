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
    SessionId, SettingsSection, Side, Tab, TabId, TabKind, UnsavedTab, WindowLabel, WindowMode,
    WindowRole, Workspace,
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
            detached: IndexMap::new(),
            dock_anchors: IndexMap::new(),
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
pub fn activate_project(ws: &mut Workspace, project: ProjectId) {
    for role in ws.windows.values_mut() {
        if let WindowRole::Shell { projects, active } = role
            && projects.contains(&project)
        {
            *active = Some(project);
        }
    }
    bump(ws);
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
    p.tabs.push(Tab {
        id,
        kind,
        tree: layout::new_tree(first_pane),
    });
    p.active_tab = id;
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

    let p = project_mut(ws, project)?;
    p.tabs.remove(index);
    if p.active_tab == tab {
        // Removal has already shifted the right-hand neighbour into `index`, so `index - 1`
        // is the tab to the left. It always exists: `index` is at least 1 and `tabs[0]` is
        // the console, which cannot be closed.
        if let Some(left) = p.tabs.get(index - 1) {
            p.active_tab = left.id;
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

/// Make `tab` the project's active tab.
pub fn activate_tab(ws: &mut Workspace, project: ProjectId, tab: TabId) -> Result<()> {
    let p = project_mut(ws, project)?;
    index_of_tab(p, tab)?;

    let changed = p.active_tab != tab;
    p.active_tab = tab;
    if changed {
        bump(ws);
    }
    Ok(())
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
/// `repo` stays `None`: git discovery belongs to `cide-git`, and guessing here would make
/// the workspace file disagree with the repository the moment one is initialised.
fn project_root(path: PathBuf) -> ProjectRoot {
    ProjectRoot {
        label: basename(&path),
        path,
        repo: None,
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
                origin: DiffOrigin::Git,
            },
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
        assert_eq!(
            tabs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["a.rs", "b.rs"],
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
                    origin: DiffOrigin::Git,
                },
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

    #[test]
    fn closing_the_active_tab_activates_the_tab_to_its_left() {
        let mut ws = Workspace::default();
        let id = open(&mut ws, "/home/dev/work/cide");
        let first = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "one".into(),
            },
            aux_pane(),
        )
        .expect("opens");
        let second = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "two".into(),
            },
            aux_pane(),
        )
        .expect("opens");

        assert_eq!(project(&ws, id).expect("exists").active_tab, second);
        close_tab(&mut ws, id, second, false).expect("a full tab closes");
        assert_eq!(project(&ws, id).expect("exists").active_tab, first);
        validate(&ws).expect("still valid");
    }

    #[test]
    fn closing_an_inactive_tab_leaves_the_active_one_alone() {
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
        let kept = open_tab(
            &mut ws,
            id,
            TabKind::ClaudeFull {
                title: "two".into(),
            },
            aux_pane(),
        )
        .expect("opens");

        close_tab(&mut ws, id, doomed, false).expect("closes");
        assert_eq!(project(&ws, id).expect("exists").active_tab, kept);
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
        p.active_tab = console;

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

        reorder_tab(&mut ws, id, 2, 1).expect("a full tab moves");
        let tabs = &project(&ws, id).expect("exists").tabs;
        assert_eq!((tabs[1].id, tabs[2].id), (b, a));
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
}
