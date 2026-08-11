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
use crate::{Axis, PaneKind, PaneRole, Side};

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
    /// Where each entry of `detached` sat in its tab's tree, so re-docking can put it back
    /// exactly rather than merely nearby.
    ///
    /// A **second map keyed the same way** rather than a field on the `detached` value: that
    /// map's value type is the wire shape the detached-pane window renders (`App.tsx` reads
    /// `project.detached[pane]` and expects a `Pane`), and wrapping it in a record would make
    /// every reader of a torn-out pane reach through a field that only re-docking cares
    /// about. The two are inserted and removed together in `cide-core::workspace`, and
    /// `validate` refuses an anchor whose pane is not detached.
    ///
    /// Sparse by design: an entry is absent for a pane detached by an older build, and a
    /// present entry can still go stale — the sibling it names may have been closed while the
    /// pane was out. Re-docking checks before it trusts one.
    #[serde(default)]
    pub dock_anchors: IndexMap<PaneId, DockAnchor>,
    /// Immortal while the project is open. For a Claude pane this id *is* the value passed
    /// to `claude --session-id`, so restoring a workspace needs no extra bookkeeping to map
    /// a pane back to its conversation.
    pub primary_session: SessionId,
}

/// A project the user opened once, remembered after it closed.
///
/// **Not part of [`Workspace`]**, and that is the whole point of the type. `workspace.json`
/// holds what is *open*; closing a project removes it from there, which is exactly the moment
/// a "recent projects" list has to start knowing about it. So this is persisted on its own, in
/// `recent.json` beside it — see `cide_core::persist::recent_path`.
///
/// Identity is [`Self::path`], the primary root. A project can hold several roots, but the
/// gesture being remembered is "the folder I opened", and reopening the primary root is what
/// `open_project` deduplicates against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentProject {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub name: String,
    /// Display form of [`Self::path`], e.g. `~/work/cide`.
    ///
    /// Recomputed on read rather than trusted from the file, for the same reason
    /// `refresh_display_paths` exists: the `$HOME` that wrote the entry is not necessarily the
    /// one reading it.
    pub display_path: String,
    /// Milliseconds since the Unix epoch, at the last open. Sorts the list.
    pub opened_at: u64,
}

/// A [`RecentProject`] as the frontend sees it: the record, plus whether it is still there.
///
/// `exists` is deliberately **not** a field of `RecentProject`, because it is not a property
/// of the record — it is a fact about the filesystem at the moment the list was asked for, and
/// a directory can be deleted, renamed or unmounted a second later. Persisting it would write
/// an answer that is stale before it is read; the alternative that lost was a
/// `#[serde(skip)] exists` on the record itself, which reads as though the file stored it.
///
/// A missing directory is still listed. The user's mental model is "the projects I worked on",
/// and silently dropping one is indistinguishable from the app forgetting it — so the entry
/// stays, the UI disables it with the reason, and nothing pretends to open a folder that is
/// gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentEntry {
    pub project: RecentProject,
    pub exists: bool,
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
        /// A **preview** tab: the one slot a single click in the git panel is allowed to
        /// re-point at another file. VS Code's preview tab, and taken deliberately.
        ///
        /// > *"Only when diff is already opened one click should change current diff to
        /// > selected file."*
        ///
        /// "Change *the* current diff" needs an answer to "which one", and the answer has to
        /// survive several diff tabs being open at once. Two candidates lost. *The active
        /// tab* means a diff the user opened on purpose and then walked away from gets
        /// silently re-pointed the moment they come back to it — the click that was supposed
        /// to be cheap eats the tab they were comparing against. *The most recently focused
        /// one* needs a focus history that `Workspace` does not record and that nothing else
        /// would ever read.
        ///
        /// So the slot is marked instead of inferred, and the mark is set by the gesture
        /// that made the tab: a double-click (`tab_open_diff`) means "open this properly"
        /// and produces `preview: false`; a single click (`tab_retarget_diff`) reuses the
        /// preview tab, or creates one when there is none. That is the symmetry the panel
        /// already had — double-click has meant "open properly" since the click rules landed
        /// — and it makes the count the user complained about fall from thirty to one.
        ///
        /// Always `false` for a [`DiffOrigin::ClaudeMcp`] tab, and
        /// `cide_core::workspace::retarget_diff` refuses one outright: that tab is holding an
        /// agent turn open on a promise about *one* file, and re-pointing it would leave the
        /// CLI blocked on a diff nobody can answer.
        ///
        /// `#[serde(default)]` because this variant is older than the field — unlike
        /// [`DiffOrigin::Git`]'s fetch key, `TabKind::Diff` has been reachable through
        /// Claude's `openDiff` since M7, so `workspace.json` files carrying the field-less
        /// form exist. Defaulting to `false` reads them as what they are: tabs nobody
        /// designated as scratch.
        #[serde(default)]
        preview: bool,
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
            Self::Diff { spec, .. } => spec.title.clone(),
            Self::Settings { .. } => "Settings".into(),
        }
    }
}

/// What a diff tab is *about*. Never what it contains.
///
/// This struct is persisted: it lives in `TabKind::Diff`, inside `Workspace`, which
/// `cide-core::persist` debounces to `workspace.json`. So it carries identity and nothing
/// else — no original text, no proposal, no patch. That is the same rule that keeps
/// `new_file_contents` out of a Claude diff (the pane calls `claude_diff_content` when it
/// mounts) and it is why [`DiffOrigin::Git`] carries a *fetch key* rather than a diff: the
/// pane calls `git_diff_file` when it mounts.
///
/// Two reasons, and the second is the one that bites. A saved layout would hold megabytes of
/// file text that nobody reads back; and, worse, it would hold a *stale* copy — the working
/// tree moves while the tab is open, so a diff written to disk at 10:00 and restored at
/// 14:00 describes a file that no longer exists in that form. A key re-reads; a copy lies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffSpec {
    pub title: String,
    /// The left-hand side's path.
    ///
    /// Absolute for a `ClaudeMcp` diff, which names files on disk. **Repo-relative** for a
    /// `Git` one, because that is how git spells a path and how the fetch key in
    /// [`DiffOrigin::Git`] spells it — a `PathBuf` here is the display label for a pair of
    /// paths, not a handle anything opens.
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
    ///
    /// The three fields are the **fetch key** the pane re-reads the diff with, and they are
    /// exactly the arguments of `git_diff_file`. Same shape as `ClaudeMcp`'s `request_id`,
    /// for the same reason: see [`DiffSpec`] for why no diff text may live in here.
    ///
    /// They arrived after the variant did, and adding them is *not* a schema break in
    /// practice: until `tab_open_diff` there was no command that could produce a `Git` origin
    /// at all — the only constructors were two fixtures in `cide-core`'s tests — so no
    /// `workspace.json` in existence carries the old field-less form for the new one to fail
    /// on. Had one existed, `CURRENT_SCHEMA` would have had to move and `persist::migrate`
    /// would have needed an arm; both are cheaper to note here than to discover from a
    /// quarantined workspace.
    Git {
        /// Which repository the path belongs to. A `RepoId` is a uuid derived from the
        /// canonical work tree, **not** a path — `cmd/git.rs::repo_root` resolves it, and a
        /// path passed where an id belongs is a `NoSuchRepo` on every button in the panel.
        repo: RepoId,
        /// Repo-relative and slash-separated, the way `cide_ipc::git` spells every path.
        path: String,
        /// Which pair of trees the tab was opened on.
        ///
        /// Load-bearing rather than decorative: `cide_git::stage::stage` re-derives the diff
        /// with `Unstaged`, `unstage` with `Staged`, and `commit` with `Combined`. A
        /// [`crate::git::Selection`] names *positions in a diff*, so one made against the
        /// wrong side is refused by the `rev` check — and would name different lines if the
        /// check were skipped. The pane may switch sides while it is open; this is where it
        /// starts, which is all a saved layout has any business remembering.
        side: crate::git::DiffSide,
    },
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

/// Enough of a pane's old position to rebuild the split that detaching collapsed.
///
/// Detaching a pane removes its leaf and folds the parent split into whatever was on the
/// other side. Everything that split knew — its axis, its divider position, which half the
/// pane occupied — is gone with it, and a re-dock that does not remember it can only guess.
/// The guess is visible: a pane that was 70/30 coming back as 50/50 resizes a terminal, and
/// a reflowed `claude` TUI redraws its whole transcript.
///
/// What is stored is the *sibling node*, not a path from the root. A path indexes positions
/// and every position shifts when any other pane in the tab is closed or split; a node id
/// either still exists or plainly does not, which is exactly the question re-docking needs
/// answered. Storing the sibling also handles the case a pane id alone cannot: the pane may
/// have been split against a whole subtree, and re-entering "beside" some leaf inside that
/// subtree would nest it one level too deep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DockAnchor {
    /// The node left behind when the split collapsed. The pane goes back beside *this*.
    pub sibling: DockSibling,
    /// The id of the collapsed split, reinstated rather than minted afresh so that anything
    /// keyed on the divider — a drag in flight, a ratio the frontend is animating — refers to
    /// the same divider it did before the detach.
    pub split: SplitId,
    pub axis: Axis,
    /// Which half of the split the detached pane occupied, in the same sense as the `side`
    /// passed to a split: `Before` means it was child `a`.
    pub side: Side,
    /// The collapsed split's ratio — the fraction that went to child `a`, so it is read
    /// together with `side` rather than being "the pane's share".
    pub ratio: f32,
}

/// One node of a pane tree, named by whichever id it carries.
///
/// A leaf is its pane and an interior node is its split; both id spaces are UUIDs minted
/// once, so a lookup answers "still there" or "gone" with no ambiguity in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DockSibling {
    Pane { pane: PaneId },
    Split { split: SplitId },
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `workspace.json` written before the preview flag existed still loads.
    ///
    /// `TabKind::Diff` has been reachable through Claude's `openDiff` since M7, so saved
    /// layouts carrying the field-less form are real — unlike [`DiffOrigin::Git`]'s fetch
    /// key, which had no constructor outside tests when it arrived. Without the
    /// `#[serde(default)]` this pins, `persist::load` would reject those workspaces and the
    /// user would come back to a window that had forgotten its tabs.
    #[test]
    fn a_diff_tab_saved_before_the_preview_flag_still_loads() {
        let json = r#"{
            "kind": "diff",
            "spec": {
                "title": "main.rs \u2014 diff",
                "oldPath": "src/main.rs",
                "newPath": "src/main.rs",
                "origin": { "kind": "claudeMcp", "requestId": "req-1" }
            }
        }"#;

        let kind: TabKind = serde_json::from_str(json).expect("an older diff tab still parses");

        assert!(
            matches!(kind, TabKind::Diff { preview: false, .. }),
            "a tab nobody designated as scratch is not one"
        );
    }
}
