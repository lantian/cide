//! The workspace tree: projects, windows, tabs, pane trees, panes.
//!
//! These types are **data only** — every operation on them lives in `cide-core`, as free
//! functions rather than inherent impls, which is what keeps this crate free of logic and
//! linkable without a webview.
//!
//! They serve three roles at once and are deliberately not duplicated per role:
//!
//! * the in-memory domain that `cide-core` mutates,
//! * the on-disk format of `workspace.json`,
//! * the wire format the webview receives.
//!
//! A parallel "snapshot" hierarchy would double the surface for no benefit here: the
//! frontend genuinely needs the whole tree, and any divergence between the three would be
//! a bug rather than a feature.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::*;
use crate::settings::Settings;
use crate::{Axis, PaneKind, PaneRole};

/// The persisted root. Everything durable hangs off this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Workspace {
    /// Bumped by `cide-core::persist` when the on-disk shape changes, so an old file can
    /// be migrated rather than silently misread.
    pub schema_version: u32,
    /// Bumped on every mutation. Every event carries it, so the frontend can discard a
    /// snapshot that arrived out of order.
    pub rev: u64,
    pub settings: Settings,
    /// Insertion order **is** the header tab order.
    pub projects: IndexMap<ProjectId, Project>,
    pub windows: IndexMap<WindowLabel, WindowRole>,
}

impl Workspace {
    pub const CURRENT_SCHEMA: u32 = 1;
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            schema_version: Self::CURRENT_SCHEMA,
            rev: 0,
            settings: Settings::default(),
            projects: IndexMap::new(),
            windows: IndexMap::new(),
        }
    }
}

/// What an OS window is showing.
///
/// The `Stacked` and `PerProject` window modes are nothing more than two different
/// mappings from `ProjectId` to `WindowRole::Shell` over identical state — which is why
/// flipping the setting touches no project, tab, pane or session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum WindowRole {
    /// N projects in `Stacked` mode, exactly one in `PerProject`.
    ///
    /// `active` is `None` exactly when `projects` is empty. That is a real state, not a
    /// degenerate one: it is what the app shows on first launch and after the last project
    /// is closed — an empty frame with a `+` in the header, which is what the design mock
    /// draws. Modelling it as a bare `ProjectId` would force every such window to name a
    /// project that does not exist.
    Shell {
        projects: Vec<ProjectId>,
        active: Option<ProjectId>,
    },
    /// One pane, torn out of its tab into its own window. `home` is where it re-docks.
    DetachedPane {
        project: ProjectId,
        tab: TabId,
        pane: PaneId,
    },
    DetachedTab {
        project: ProjectId,
        tab: TabId,
    },
}

/// One opened project: a header tab, or a whole window in `PerProject` mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// Display form of the primary root, e.g. `~/work/cide`.
    pub display_path: String,
    /// CSS colour for the header tab's dot, e.g. `var(--accent)`.
    pub dot: String,
    /// A project may hold several roots and nested submodules. Never empty; `roots[0]` is
    /// the primary root and supplies `display_path`.
    pub roots: Vec<ProjectRoot>,
    /// `tabs[0]` is **always** the pinned Claude console. Enforced in `cide-core`.
    pub tabs: Vec<Tab>,
    pub active_tab: TabId,
    /// Panes torn out into their own windows.
    ///
    /// A detached pane cannot stay in its tab's `panes` map: the tree invariant is that the
    /// leaf set and the key set are equal, and its leaf is gone. It has not stopped
    /// belonging to the project though — its session is still running, and `WindowRole::
    /// DetachedPane` records where it re-docks — so it waits here rather than being
    /// destroyed and rebuilt, which would lose the binding to its conversation.
    #[serde(default)]
    pub detached: IndexMap<PaneId, Pane>,
    /// Immortal while the project is open. For a Claude pane this id *is* the value passed
    /// to `claude --session-id`, so restoring a workspace needs no extra bookkeeping to map
    /// a pane back to its conversation.
    pub primary_session: SessionId,
}

/// One root directory within a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectRoot {
    #[ts(type = "string")]
    pub path: PathBuf,
    /// `None` when the root is not inside a git repository.
    pub repo: Option<RepoId>,
    /// Shown as the top-level tree row when a project has more than one root.
    pub label: String,
}

/// A workspace tab.
///
/// **Every** tab carries a `PaneTree`, including file tabs. That is what makes "split the
/// editor right" and "promote this pane to a full tab" the same code path instead of two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Tab {
    pub id: TabId,
    pub kind: TabKind,
    pub tree: PaneTree,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum TabKind {
    /// The pinned project console. Always `tabs[0]`; `tab.close` on it is an error.
    ClaudeHome,
    /// A closable full-screen Claude tab. Splitting it creates *new* sessions.
    ClaudeFull {
        title: String,
    },
    File {
        #[ts(type = "string")]
        path: PathBuf,
        dirty: bool,
    },
    Diff {
        spec: DiffSpec,
    },
    Settings {
        section: SettingsSection,
    },
}

impl TabKind {
    /// Whether this tab may be closed. Only the pinned console may not.
    pub fn closable(&self) -> bool {
        !matches!(self, Self::ClaudeHome)
    }

    /// The label shown in the workspace tab strip.
    pub fn title(&self) -> String {
        match self {
            Self::ClaudeHome => "Claude".into(),
            Self::ClaudeFull { title } => title.clone(),
            Self::File { path, .. } => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            Self::Diff { spec } => spec.title.clone(),
            Self::Settings { .. } => "Settings".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffSpec {
    pub title: String,
    #[ts(type = "string")]
    pub old_path: PathBuf,
    #[ts(type = "string")]
    pub new_path: PathBuf,
    pub origin: DiffOrigin,
}

/// Where a diff came from, which decides what happens when the user answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DiffOrigin {
    /// Opened from the git panel. Closing it is just closing a tab.
    Git,
    /// Opened by Claude Code's `openDiff` RPC.
    ///
    /// This one **blocks an agent turn**: the CLI will not proceed until the request is
    /// answered, so every way the user can make this tab go away has to resolve the
    /// pending future. See `cide-ide-mcp::diff_broker`.
    ClaudeMcp { request_id: String },
}

/// How the user answered a diff that Claude Code is blocked on.
///
/// The three outcomes are the protocol's, and the difference between them is what gets
/// written to disk — so this is one of the few wire types where a wrong mapping destroys
/// work rather than degrading a view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DiffAnswer {
    /// Accepted after the user edited it in cide's own diff view.
    ///
    /// Carries the buffer as it stands on screen, which the CLI reads from `content[1].text`
    /// and writes verbatim. This is the outcome that makes the diff pane an editor rather
    /// than a viewer, and the reason it reads its document at click time instead of trusting
    /// the proposal it was handed.
    AcceptedEdited { contents: String },
    /// Accepted exactly as the model proposed it. The file becomes the model's version.
    AcceptedAsIs,
    /// Rejected. The file is not touched.
    ///
    /// Also the answer for every way a diff can disappear without being answered — a closed
    /// tab, pane, window or project — because rejection is recoverable and a write is not.
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SettingsSection {
    #[default]
    Appearance,
    ProjectsAndWindows,
    Keymap,
    ClaudeSessions,
    Editor,
    Git,
    Terminal,
}

/// A tab's pane layout.
///
/// Pane *data* lives in a side map rather than inside the leaves, so looking a pane up by
/// id does not mean walking the tree, and `LayoutNode` stays small enough to clone freely
/// during a rebalance.
///
/// Invariant, asserted in `cide-core` tests: the set of leaf ids equals the key set of
/// `panes`, exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PaneTree {
    pub root: LayoutNode,
    pub focused: PaneId,
    /// Maximizing is **not** a tree mutation — it is a flag the renderer honours by giving
    /// one pane the whole grid area and hiding its siblings. That keeps restore exact and
    /// costs no terminal reflow.
    pub maximized: Option<PaneId>,
    pub panes: IndexMap<PaneId, Pane>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum LayoutNode {
    Leaf {
        pane: PaneId,
    },
    Split {
        id: SplitId,
        axis: Axis,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
        /// Fraction given to `a`, clamped to [`MIN_RATIO`, `MAX_RATIO`].
        ratio: f32,
    },
}

/// Splits never collapse a child to nothing: below this a pane cannot show its title bar.
pub const MIN_RATIO: f32 = 0.1;
pub const MAX_RATIO: f32 = 0.9;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Pane {
    pub id: PaneId,
    pub kind: PaneKind,
    pub role: PaneRole,
    /// `None` for panes with no process, such as a diff view.
    pub session: Option<SessionId>,
    /// e.g. `cide : claude`, `cide : bash`, `cide : claude — diff`.
    pub title: String,
}

/// Which way to move when navigating between panes with the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    /// The split axis this direction moves along.
    pub fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Row,
            Self::Up | Self::Down => Axis::Col,
        }
    }

    /// True when moving towards the second child of a split.
    pub fn is_forward(self) -> bool {
        matches!(self, Self::Right | Self::Down)
    }
}
