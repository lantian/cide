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
    /// # 1 → 2: the proxy scope
    ///
    /// Bumped when [`crate::ProxyScope`] arrived, and **not** because serde would have failed
    /// without it — `ProxySettings` carries `#[serde(default)]`, so a schema-1 document
    /// deserialises perfectly well and gets `ProxyScope::default()`, which is by construction
    /// exactly what schema 1 meant.
    ///
    /// The bump is for the day after. `Default` is the answer for a *fresh install*, and the
    /// whole point of the scope is that somebody will eventually want a fresh install to
    /// default to something narrower. On that day, a defaulted field silently re-scopes every
    /// existing user's live proxy configuration — a corporate laptop's `git push` moves onto
    /// cide's proxy, or off it, because of a constant edited for an unrelated reason, with no
    /// diff on anyone's disk to explain it. `persist::migrate` therefore *writes* the scope
    /// into the document, so what an upgraded user has is a fact on disk rather than a
    /// consequence of a constant.
    ///
    /// This is the first migration this ladder has ever run, and it is what
    /// `serde_json`'s `preserve_order` feature was turned on for: without it a migrated
    /// document goes through a `serde_json::Value` whose object keys are sorted, and
    /// `projects` insertion order **is** the header tab order.
    /// # 2 → 3: autosave
    ///
    /// Bumped when `EditorSettings::autosave` arrived, and again **not** because serde would
    /// have failed without it: the field defaults to `true`, so a schema-2 document loads and
    /// gets exactly the behaviour a fresh install gets.
    ///
    /// The bump is because of what the field *does*. Autosave writes the user's files on a
    /// timer, and a defaulted field means every existing workspace's answer to "may cide do
    /// that" is an inference from a constant in this build rather than a statement on the
    /// user's disk. If the default is ever narrowed — and "off by default, opt in" is the
    /// obvious first request for a feature like this — a defaulted field would silently turn
    /// autosave *off* for everyone who had been relying on it, with nothing in their
    /// `workspace.json` to explain it. `persist::v2_to_v3` therefore writes `true` into every
    /// upgraded document, so a later change to the default is a change to new installs only.
    ///
    /// This is the same argument 1 → 2 makes about the proxy scope, applied to a setting whose
    /// blast radius is the contents of files rather than an environment variable.
    /// # 3 → 4: ignored files become visible by default
    ///
    /// The odd one out, and worth reading before adding a fifth: this is the only rung that
    /// **overwrites a value already on disk**. Every other one uses `or_insert_with`, and
    /// [`crate::settings::ExplorerSettings`] carries `#[serde(default)]`, so a document that
    /// predates the field would take the new default with no migration at all.
    ///
    /// The problem is the documents that do *not* predate it. `show_ignored_files` shipped as
    /// `false` and every workspace written by that build has the word `false` in it — not
    /// because anyone chose it, but because a constant in that build said so. Leaving those
    /// alone would mean the people who already have the feature are exactly the people it stays
    /// switched off for, including the person who asked for it.
    ///
    /// Overwriting a user's answer is the thing this ladder exists to prevent, so the exception
    /// is bounded rather than general: it rewrites `false` and only `false`, on a field that
    /// existed for a matter of hours before this correction, in a build that never presented it
    /// with any other default. A `true` is left alone because it agrees, and a user who wants
    /// the cheap walk back now turns it off against a default that is finally the one the
    /// feature was asked for — a statement on their disk, which is what rungs 1 → 2 and 2 → 3
    /// are both about.
    pub const CURRENT_SCHEMA: u32 = 5;
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
    /// This project's tabs in most-recently-*active* order, `tab_mru[0]` being [`Self::active_tab`].
    ///
    /// # Why the focus history is domain state and `mru` (projects) is not
    ///
    /// It was in the webview until M15, derived per snapshot from `active_tab` and cached under
    /// `localStorage`'s `cide.tabMru`; `ui/src/store/workspace.ts` carried a long note admitting
    /// the argument for keeping it there was weak, because `active_tab` is already a workspace
    /// field and a history behind it is less of an outlier than a history of *window* activations.
    ///
    /// What settled it is that **`close_tab` has to pick a successor and `close_tab` is here.**
    /// A tab closing is the one moment the order is consulted rather than merely walked, and two
    /// of its call sites have no webview to ask: `cide_app::ide`'s withdrawn-diff path closes a
    /// tab straight from the MCP server's thread, and the quit ladder closes projects wholesale.
    /// A successor computed in a renderer and passed down would therefore have to exist twice,
    /// and the second copy — the one in Rust — would be the left-neighbour rule this replaces.
    ///
    /// The two objections the frontend note raised both evaporate for this field. *Broadcast
    /// cost*: it moves only inside `activate_tab`, `open_tab` and `close_tab`, each of which
    /// already bumps `rev` and broadcasts, so it costs no additional event. *Migration*:
    /// `#[serde(default)]` reads every existing `workspace.json` as an empty order, which
    /// `close_tab` handles by falling back to the left neighbour and which
    /// `persist::load`'s repair fills in from `active_tab` on the way through — so
    /// `CURRENT_SCHEMA` does not move.
    ///
    /// Holds **live tab ids only, without duplicates**, and `cide_core::workspace::validate`
    /// enforces both. It may legitimately be *shorter* than `tabs`: a workspace restored from an
    /// older build starts with one entry, and `reconcile` in `keys/switcher.ts` appends the rest
    /// in strip order when the switcher walks it.
    ///
    /// The pinned console is in here like any other tab, deliberately — see
    /// `cide_core::commands`'s note on `tab.switcher.next`: "from the file I was reading back to
    /// the conversation about it" is the whole value of the gesture.
    #[serde(default)]
    pub tab_mru: Vec<TabId>,
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
    /// The console's conversation. For a Claude pane this id *is* the value passed to
    /// `claude --session-id`, so restoring a workspace needs no extra bookkeeping to map a pane
    /// back to its conversation.
    ///
    /// It **follows the console's primary pane** — minted with the project and rewritten by
    /// `cide_core::workspace::bind_session` whenever that pane binds a different session, which
    /// is what a restart does. This used to say "immortal while the project is open", and it
    /// was: nothing wrote it after `open_project`, so the first time the console respawned (a
    /// restore whose transcript had gone, and now any restart) this field went on naming a
    /// conversation no pane held. `lifecycle::entry_for` reads it to decide whether the console
    /// comes back live on the next launch, so a stale value costs the user a Resume splash on
    /// the one pane that should never need one.
    pub primary_session: SessionId,
    /// The Git tool window across the bottom of this project: whether it is open, how tall, and
    /// which histories it has tabs for. (M18)
    ///
    /// # Why `CURRENT_SCHEMA` does not move for this
    ///
    /// Modelled on the note [`Self::tab_mru`] carries, and the argument is the same one three
    /// times over.
    ///
    /// `#[serde(default)]` reads every existing `workspace.json` unchanged — a document written
    /// before this field gets [`ToolWindowState::default()`], which is a closed panel, which is
    /// exactly what those documents meant.
    ///
    /// The two bumps that *did* happen (1 → 2 and 2 → 3, and 3 → 4 for a different reason again)
    /// were not about serde either; both were about a **defaulted field whose default might
    /// later move**, where the movement would silently re-scope a live proxy or stop cide
    /// writing the user's files. Neither hazard exists here. If the tool window's default height
    /// changes, an existing user's stored height wins because it is on their disk, and if the
    /// default `open` ever flips, the worst outcome is a panel that appears once and that the
    /// user closes with one click. A panel is not a proxy scope and is not autosave.
    ///
    /// And a bump is not free in the other direction: it makes an older build's read of a newer
    /// document *certainly* fatal — `persist::load` quarantines a schema it does not know —
    /// where an unbumped document with one unknown key is read perfectly by every build that
    /// predates the key. Downgrading across a bump costs the user their whole layout; downgrading
    /// across this field costs them nothing at all.
    #[serde(default)]
    pub tool_window: ToolWindowState,
}

/// The shortest the tool window may be stored as, in CSS pixels.
///
/// Below this the tab bar and one row of log do not both fit, so the panel is present, costing
/// its own chrome, and showing nothing.
pub const TOOL_WINDOW_MIN_HEIGHT: u16 = 120;

/// The tallest, in CSS pixels.
///
/// The *stored* ceiling, generous on purpose, and it exists for the same reason
/// [`crate::settings::SIDEBAR_MAX_WIDTH`] does: to stop a corrupt or hand-edited
/// `workspace.json` producing a panel taller than any monitor. What actually fits depends on the
/// window, which Rust cannot see, so the frontend applies a second ceiling against the live
/// viewport before it paints. A 900px panel restored into a 700px window is therefore safe — it
/// is clamped on the way to the DOM and the stored value survives for the next time the window
/// is tall enough to honour it.
pub const TOOL_WINDOW_MAX_HEIGHT: u16 = 900;

/// The Git tool window's own state, per project. (M18)
///
/// Per project and not per window, deliberately. The panel is a view of *this project's*
/// repositories, and the same project shown in two windows (which `Stacked` mode makes ordinary)
/// is one set of open histories; keying it by window would mean a user who detaches a tab finds
/// the panel they were reading empty, and a user who reads a history in two windows has to open
/// it twice.
///
/// It sits in [`Project`] rather than in [`crate::settings::Settings`] for the reason the whole
/// workspace tree exists: this is *what is open*, not *how the app behaves*. A settings field
/// would be shared across every project, so opening a second project would inherit the first's
/// history tabs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolWindowState {
    /// Closed by default. A tool window that opens itself on first launch is a strip of chrome
    /// the user has to dismiss before they have asked anything of it.
    pub open: bool,
    /// Height in CSS pixels, clamped to [`TOOL_WINDOW_MIN_HEIGHT`]..=[`TOOL_WINDOW_MAX_HEIGHT`]
    /// by [`Self::clamped`].
    pub height: u16,
    /// The open Log tabs, in strip order.
    pub history: Vec<HistoryTab>,
    /// Which of them is showing. `None` exactly when `history` is empty — the same real,
    /// non-degenerate empty state [`WindowRole::Shell`]'s `active` models, and for the same
    /// reason: a bare [`HistoryTabId`] would force an empty panel to name a tab that does not
    /// exist.
    pub active: Option<HistoryTabId>,
    /// The commit list's share of the Log tab, in **per mille** (0–1000). 450 is the default.
    ///
    /// # Why an integer and not an `f32`
    ///
    /// Two reasons, and the second is the one that would have been discovered later.
    ///
    /// A float in a *persisted* document does not round-trip exactly through `serde_json`: a
    /// value written as `0.45` comes back as the nearest `f32`, is re-serialised, and the
    /// document's bytes change on a save where nothing moved. That is noise in a file the user
    /// can diff, and it defeats any equality check on a workspace snapshot.
    ///
    /// And an `f32` bars `Eq` from every type that transitively contains it. Nothing breaks
    /// *today* — [`Workspace`], [`Project`] and [`PaneTree`] already derive only `PartialEq`,
    /// because [`LayoutNode::Split`]'s `ratio` is an `f32` — but "already poisoned" is a
    /// property of one field in one place, and it is not a licence for the next one. A panel
    /// divider is a poor reason to make it two.
    ///
    /// Per mille rather than percent because the panel is up to 900px tall and a hundred steps
    /// over that is a 9px jump, which is plainly visible while dragging.
    pub log_split: u16,
    /// Whether the details pane groups its changed files into directories, or lists the paths
    /// flat. `true` — grouped — by default.
    ///
    /// # Why it is stored, and why here
    ///
    /// A display toggle that forgets is a control the user has to press again every session, so
    /// the only question is where it lives. It is on the tool window and not in [`Settings`]
    /// because it is the same class of thing as `log_split` beside it: a fact about how one
    /// panel of one project is arranged, not a preference about the application. `Settings` is
    /// global and rides every `workspace-changed` to every window, which is the right shape for
    /// a font and the wrong one for a panel's own furniture.
    ///
    /// Grouped by default because a commit's files share a prefix far more often than not — one
    /// feature, one directory — and a flat list of thirty paths that all begin
    /// `crates/cide-git/src/` spends most of its width saying the same thing. The flat reading
    /// is what a *rename* wants, since `old → new` across directories is unreadable when the two
    /// halves are in different subtrees, which is exactly why the choice is offered rather than
    /// decided.
    #[serde(default = "files_as_tree_default")]
    pub files_as_tree: bool,
}

/// Grouped. A free function because `#[serde(default = …)]` names a path, not an expression, and
/// `bool::default()` is `false` — the opposite of what this field wants.
fn files_as_tree_default() -> bool {
    true
}

impl Default for ToolWindowState {
    fn default() -> Self {
        Self {
            open: false,
            height: 260,
            history: Vec::new(),
            active: None,
            log_split: 450,
            files_as_tree: files_as_tree_default(),
        }
    }
}

impl ToolWindowState {
    /// The height brought inside [`TOOL_WINDOW_MIN_HEIGHT`]..=[`TOOL_WINDOW_MAX_HEIGHT`] and the
    /// split inside 0..=1000.
    ///
    /// Applied where a patch lands rather than where the workspace is read, mirroring
    /// [`crate::settings::SidebarSettings::clamped`]: clamping on read means the stored file
    /// never converges and is re-corrected on every launch for ever, where clamping on write
    /// fixes it once.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.height = self
            .height
            .clamp(TOOL_WINDOW_MIN_HEIGHT, TOOL_WINDOW_MAX_HEIGHT);
        self.log_split = self.log_split.min(1000);
        // Every tab's own divider too. A hand-edited `workspace.json` can hold any number, and a
        // History tab whose split is 60000 is a pane grid with one column — the same failure
        // `log_split` is clamped against, once per tab.
        for tab in &mut self.history {
            tab.split = tab.split.map(|s| s.min(1000));
        }
        self
    }
}

/// One tab of the tool window's Log surface. (M18)
///
/// The whole tab and not a `(RepoId, String)` pair, because two tabs on the same file are the
/// ordinary case — see [`HistoryTabId`]'s own note — so the identity has to be minted rather
/// than derived from the subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryTab {
    /// This tab's divider, in per mille, or `None` for the kind's default.
    ///
    /// # Per tab, not per project (M21)
    ///
    /// `ToolWindowState::log_split` is the Log tab's, and a History tab wants a different number
    /// for a reason that is about the *question* rather than about taste: the Log tab's right
    /// half is a commit's summary and its file list, which is a narrow column beside a wide list,
    /// while a History tab's right half is **one file's diff** — the thing the tab was opened to
    /// read. Fifty-fifty is what that wants, and it is what was asked for.
    ///
    /// Sharing one number would make the two fight: dragging the log wide would squeeze every
    /// history diff, and widening a diff would squeeze the log. `None` rather than a stored 500
    /// so a tab that has never been dragged follows the default if it ever changes, which is the
    /// same reason `log_split` is a field and not a literal at the call site.
    #[serde(default)]
    pub split: Option<u16>,
    pub id: HistoryTabId,
    pub repo: RepoId,
    /// Repo-relative and slash-separated, the way `cide_ipc::git` spells every path. **Empty
    /// means the whole repository**, which is the commonest tab of all — an `Option<String>`
    /// would put a `None` case into every consumer to express a state that is already the
    /// natural zero of a path filter, and `cide_ipc::history::LogQuery::path` converts the empty
    /// string to `None` at the one seam that cares.
    pub path: String,
    /// What the chip shows: a file name, or the repository's name for a whole-repository tab.
    ///
    /// Stored rather than derived, for the reason [`DiffSpec`] gives about titles generally: the
    /// title is a *label the gesture chose* and it survives a restart, where a derived one would
    /// change the day the derivation is improved and would silently relabel every saved tab.
    pub title: String,
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
///
/// # There is deliberately no `repo` field
///
/// There was one — `repo: Option<RepoId>`, documented as "`None` when the root is not inside a
/// git repository" — and **nothing ever wrote anything but `None` into it**. Git discovery
/// belongs to `cide-git`, which depends on `cide-core` and so cannot be called from the one
/// constructor (`cide_core::workspace::project_root`); writing a discovered id into the
/// workspace *file* was refused for a second and independent reason, that it would disagree
/// with the disk the moment anyone ran `git init`.
///
/// The frontend trusted it anyway. `keys/target.ts::reposOf` collected the field across a
/// project's roots, `keys/context.ts` derived the `repoOpen` context flag from the result, and
/// every git command was gated on that flag — so the whole Git group was filtered out of the
/// command palette on every platform, for every user, for ever, and three of the handlers bailed
/// before acting even when reached another way. Nothing failed anywhere: a field that is always
/// `None` looks exactly like a root that happens not to be in a repository.
///
/// A root also cannot *represent* the answer. A submodule has a `RepoId` and no `ProjectRoot`
/// at all, which is why `cmd/git.rs` resolves every repository through `cide_git::repo::discover`
/// and always did. So the field was not merely unfilled, it was the wrong shape for the
/// question; `git_repos` is the right one, and it asks the disk.
///
/// Removing it needs no schema bump. `serde` ignores unknown fields, so every `workspace.json`
/// already on disk — all of which carry `"repo": null` — still loads, and the next save drops it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectRoot {
    #[ts(type = "string")]
    pub path: PathBuf,
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
    /// A file, by absolute path. **Nothing here says the file is in the project.**
    ///
    /// It never did — `tab_open_file` enforces nothing, and a Ctrl+B into
    /// `~/.cargo/registry/…` has opened an editable tab for as long as go-to-definition has
    /// existed. `cmd::file::terminal_open_path`'s approved out-of-project open makes that
    /// ordinary rather than incidental, so the consequence is written down here rather than
    /// discovered later:
    ///
    /// **A tab approved once comes back on restart without being asked again.** That is a
    /// decision and not an oversight. The approval was for a *file*, the user gave it by name,
    /// and re-opening the same file they approved grants nothing new; asking again every launch
    /// would train exactly the reflexive approval the confirmation exists to prevent. The tab is
    /// also cide's own record, in a file cide writes — a `workspace.json` an attacker can edit
    /// is a machine on which `keymap.json` already runs arbitrary commands.
    ///
    /// There is deliberately **no `external: bool`** beside `dirty`. It would have to earn its
    /// place by changing behaviour, and the two candidates both lost: read-only does not
    /// un-read a file that has already been read into the buffer, and it would leave cide's two
    /// ctrl+clicks disagreeing about the same file; and a marker in the tab strip is already
    /// there in a better form, because `editor/statusReadout.ts`'s `pathTrail` falls back to
    /// the *whole absolute path* for a file no root contains.
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
        /// one* needed a focus history that `Workspace` did not record.
        ///
        /// M15 added one — [`Project::tab_mru`] — and this decision does **not** change with it,
        /// which is worth saying rather than leaving for someone to "fix". The second candidate
        /// lost on two counts and the missing history was only one: re-pointing whichever diff
        /// the user last looked at is still the click eating a tab they opened on purpose, one
        /// step less predictably than the active-tab rule because "last looked at" is invisible.
        /// A marked slot is a fact the gesture set; an inferred one is a guess about intent.
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
    /// A file as it was at some revision, read-only. (M18)
    ///
    /// # Why this is not a [`Self::File`] with a rev on it
    ///
    /// A `File` tab is an editable buffer over a path on disk, and every piece of machinery
    /// hanging off it assumes that: it is `dirty` or it is not, autosave writes it, the LSP
    /// serves completions into it, `openDiff` can retarget it. **None of that is true of a
    /// commit's version of a file**, which cannot be saved anywhere, and a `rev: Option<String>`
    /// on `File` would make every one of those consumers responsible for remembering to check
    /// it. A separate variant makes "you cannot edit this" a fact of the type rather than a rule
    /// each reader has to know.
    ///
    /// # Why the path is a `String` and not a `PathBuf`
    ///
    /// It is **repo-relative**, the way `cide_ipc::git` spells every path — there is no file on
    /// disk for this tab to point at, and an absolute path here would be a claim that there is.
    /// [`Self::File`]'s `PathBuf` is absolute precisely because that tab *is* a file on disk.
    Revision {
        repo: RepoId,
        /// Repo-relative, slash-separated.
        path: String,
        /// The revision to read the blob at, as a full oid — resolved when the tab was opened
        /// rather than kept as the revspec the user typed. `HEAD~3` means a different commit
        /// tomorrow, and a tab restored from `workspace.json` must show the same bytes it showed
        /// when it was saved.
        rev: String,
        /// The revisions this tab was reached *from*, newest last: the trail of "blame the
        /// parent" and "older revision" hops that led here.
        ///
        /// Persisted because it is the only way back. The hops are a walk through history, and a
        /// user four hops deep who restarts cide would otherwise land on a commit with no
        /// explanation of how they got there and no route out but the log. A `Vec` and not a
        /// single `from`, because the gesture is repeatable and each step has to stay
        /// individually reachable.
        from: Vec<String>,
        /// What the tab strip shows — `main.rs @ a1b2c3d`. Stored for the reason [`DiffSpec`]
        /// gives: a derived title changes the day the derivation is improved and relabels every
        /// saved tab.
        title: String,
    },
    /// The three-pane conflict resolver, for one conflicted path. (M20)
    ///
    /// Carries only the **key** — which repository, which path — for `TabKind::Diff`'s reason,
    /// stated at length on [`DiffSpec`]: this enum is persisted inside `Workspace`, which
    /// `cide_core::persist` debounces to `workspace.json`, so a document that travelled in here
    /// would be a document written to disk. The pane calls `git_conflict_read` when it mounts.
    ///
    /// **Restoring one is not reproducible, and that is handled rather than ignored.** Unlike
    /// [`Self::Revision`], which names an immutable commit, this names a path in a *conflict* —
    /// and by the time the workspace is restored the merge may have been finished, aborted, or
    /// finished differently in a terminal. The pane asks and draws "no longer conflicted" if it
    /// is gone; it does not resurrect anything.
    ///
    /// Additive: no `workspace.json` in existence carries this variant, so `CURRENT_SCHEMA` does
    /// not move and `persist::migrate` needs no arm — the same argument
    /// [`DiffOrigin::Git`]'s fields make above.
    Merge {
        repo: RepoId,
        /// Repo-relative and slash-separated, the way `cide_ipc::git` spells every path.
        path: String,
        /// Unsaved edits in the centre pane. Same flag, same guard, as [`Self::File`]'s.
        dirty: bool,
    },
    Settings {
        section: SettingsSection,
    },
    /// One extension's page: its README, what it contributes, and what it asks for. (M22)
    ///
    /// # Why a tab kind and not a file tab on its `README.md`
    ///
    /// Because the file is not in the project. An installed extension lives under
    /// `$XDG_STATE_HOME`, and one that is only *listed* lives in a marketplace clone — both
    /// outside every root, which is exactly what `cmd::file`'s refusals exist to keep out of an
    /// editor. Widening that for this would widen it for everything.
    ///
    /// It also has to draw more than a file: the version, the capabilities the extension asks
    /// for, and the Install button, which are the things a person reading a README is deciding
    /// about.
    ///
    /// # Why `name` is stored
    ///
    /// [`TabKind::title`] is a pure function of the variant — it cannot read a registry — and the
    /// tab strip needs a caption on the frame the tab is restored in, before any snapshot has
    /// arrived. Storing the display name is what keeps a restored tab from reading `sql` in a
    /// strip where everything else is capitalised, and it costs a string that goes stale only if
    /// an extension is renamed.
    Extension {
        marketplace: MarketplaceId,
        extension: ExtensionId,
        /// What the extension calls itself. See the note above.
        name: String,
    },
    /// One OpenSpec change or capability, as a page. (M28)
    ///
    /// # Why a tab kind, when these really are files in the project
    ///
    /// Because neither is *one* file. A change is a directory — a proposal, a design, a task
    /// checklist and a delta spec per capability it touches — and which files those are is
    /// decided by the workflow schema in `openspec/config.yaml`, not by cide. Opening
    /// `proposal.md` was the first thing the panel did and it is the weakest part of the
    /// feature: it answers *show me this change* with one of its five documents and no way to
    /// reach the others, the checklist, the requirement edits, or anything that can be done
    /// about any of it.
    ///
    /// The page also draws things no file contains: whether the change validates, how far its
    /// checklist has got, which task tracks it, and the actions — because those are what a
    /// person opening a change is deciding about. `Extension`'s note above makes the same
    /// argument from the other direction.
    ///
    /// # Why one variant and not two
    ///
    /// A change and a capability are the same page at two moments — a capability is what a
    /// change becomes once it is archived — and every arm that has to handle a tab kind would
    /// otherwise have to handle two of them. The discriminator lives in [`SpecSubject`], where
    /// exhaustiveness is checked once.
    OpenSpec {
        subject: SpecSubject,
    },
}

/// What an [`TabKind::OpenSpec`] tab is looking at. (M28)
///
/// Carries the id and nothing else. The page reads everything it draws through `spec_change` /
/// `spec_spec` — deliberately, because a change's contents move while an agent works on it, and
/// a tab that had cached its own copy would be a second, staler answer than the panel's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SpecSubject {
    /// `openspec/changes/<name>/`.
    Change { change: ChangeName },
    /// `openspec/specs/<id>/spec.md`.
    Capability { spec: SpecId },
}

impl SpecSubject {
    /// What the tab strip prints.
    ///
    /// The bare id, which is what the user typed and what every other tool calls it — a change
    /// is `add-dark-mode` in the directory, in `openspec show`, and in a teammate's editor.
    /// Prefixing it with "Change: " would be cide inventing a name for something that has one.
    pub fn label(&self) -> String {
        match self {
            Self::Change { change } => change.as_str().to_string(),
            Self::Capability { spec } => spec.as_str().to_string(),
        }
    }
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
            Self::Revision { title, .. } => title.clone(),
            Self::Merge { path, .. } => format!(
                "{} — merge",
                path.rsplit('/').next().unwrap_or(path.as_str())
            ),
            Self::Settings { .. } => "Settings".into(),
            Self::Extension { name, .. } => name.clone(),
            Self::OpenSpec { subject } => subject.label(),
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
    /// Opened from the Git tool window: two revisions of one file. (M18)
    ///
    /// A sibling of [`Self::Git`] and not an extension of it, because the two name different
    /// coordinate systems and only one of them is answerable. `Git`'s `side` picks a pair of
    /// trees around the *working tree* — HEAD→index, index→workdir — which is the pair a
    /// selection can be staged from; this one names two frozen revisions, which nothing can be
    /// staged from at all. Folding the two together would mean a `DiffSide` that had to grow a
    /// "neither" case, and every consumer of `Git`'s `side` — `cide_git::stage::stage`,
    /// `unstage`, `commit` — would have to learn to refuse it. A separate variant refuses by
    /// construction.
    ///
    /// Like `Git`, these three fields are the **fetch key** the pane re-reads with, and for the
    /// reason [`DiffSpec`] states: no diff text may be persisted. Unlike `Git`, the read is
    /// reproducible — two commits diff to the same bytes for ever — so a restored tab shows
    /// exactly what it showed when it was saved.
    GitRevision {
        repo: RepoId,
        /// Repo-relative and slash-separated.
        path: String,
        /// The two sides. [`crate::history::RevSide::FirstParent`] is the reason these are a
        /// union rather than a pair of oids: "this commit against its parent" has to still mean
        /// that after a restart, and freezing the parent's oid into the saved tab survives a
        /// restart but not a rebase — the frozen oid then names a commit that is no longer an
        /// ancestor of anything the user can find in their own log.
        new: crate::history::RevSide,
        old: crate::history::RevSide,
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
    /// What the file tree shows: dotfiles, ignored files. (M18)
    ///
    /// Its own section rather than a group under `Editor` ("the code buffer") or
    /// `ProjectsAndWindows` (how projects are spread across OS windows), because it is neither:
    /// it is the explorer's content, it changes what an index costs, and both toggles re-walk
    /// every open project when they move. A setting with that consequence should be found under
    /// a heading that names it rather than at the foot of a list about something else.
    Files,
    /// Which analysers run, and which of their findings are shown. (M12)
    ///
    /// Its own section rather than a group inside `Editor`, which is described as "the code
    /// buffer" and already carries six rows: four severity toggles, one row per analyser, and a
    /// default highlighting level is a screenful, and burying a 1–4 GB indexer's on/off switch
    /// under a font size makes it undiscoverable. Named for IDEA's own tree node — the sidebar
    /// panel keeps "Problems", so the two surfaces are not both called the same thing.
    Inspections,
    /// What the installed extensions are configured with. (M22)
    ///
    /// Its own section rather than a group under something that exists, on the argument `Agents`
    /// makes for itself two variants down: the rows here are not cide's settings at all. They are
    /// declared by third-party manifests, they are stored in `extensions.json` rather than in
    /// `workspace.json`, and none of them rides `SettingsPatch`. A page of somebody else's
    /// controls under a heading about cide's own would be a category error, and it would grow
    /// without bound as extensions are installed.
    ///
    /// Between `Inspections` and `Agents`, which is where it sits in the nav — the two before it
    /// are about what analyses your code, and this is about what else is running inside cide.
    Extensions,
    /// Which models cide's agents may reach, and the ordered pools a run falls down. (M45)
    ///
    /// Its own section on [`Self::Inspections`]' test applied again. `ClaudeSessions` is *how
    /// claude is launched* and this is about a different CLI's providers entirely; `Agents` is an
    /// editor for role *files* and this is a page of genuine global settings riding
    /// [`crate::SettingsPatch`] like every other.
    ///
    /// Immediately before `Agents`, which is where it sits in the nav, because a pool is what a
    /// role's local override names — the screen should read in the order a person works:
    /// configure the models, then the roles that use them.
    ///
    /// It is also the first section that stores a **credential**, which is its own reason to be
    /// findable under a heading that names it rather than buried under a CLI's launch options.
    Models,
    /// The subagent roles this project and this user define, edited as a form. (M18)
    ///
    /// # Why a section, rather than a group under something that exists
    ///
    /// The same test [`Self::Files`] passes, applied to a different candidate. `Editor` is "the
    /// code buffer" and `ClaudeSessions` is the one Claude conversation a project's console
    /// hosts; a role is neither. It is a *definition* — a system prompt plus the switches that
    /// make that prompt safe — that cide will spawn an unattended process from, in a git worktree
    /// of the user's repository. Filing that under a heading about fonts or about a single
    /// interactive pane makes the one screen that decides what an autonomous process may do the
    /// hardest thing on the settings tree to find, which is the argument `Inspections` makes
    /// about a 1–4 GB indexer's switch and it is stronger here.
    ///
    /// # Why it is a settings section at all, when a role is a file in the repository
    ///
    /// Because "I don't want to configure a yaml, I want a UI for that" is the complaint it
    /// answers, and the place a person looks for a UI to configure something is Settings. The
    /// Agents *sidebar panel* is the roster — what roles exist, what is running, what to
    /// dispatch — and it is deliberately read-only about the definitions, for the reason
    /// [`crate::AgentDef::system_prompt`] gives: a row is not a surface anybody opened in order
    /// to edit a committed file. This is the surface they did.
    ///
    /// # What it does *not* mean
    ///
    /// Unlike every other variant here, this section does not draw
    /// [`crate::Settings`] and does not write a [`crate::SettingsPatch`]. It edits two
    /// directories of markdown — `<root>/.cide/agents/` and `$XDG_CONFIG_HOME/cide/agents/` —
    /// through `agents_save`/`agents_delete`, and one of those is per-project. That asymmetry is
    /// real and is argued out in [`crate::OrchestrationConfig`]'s doc; it is admitted here so
    /// that nobody "fixes" it by adding role definitions to `Settings`, which would follow the
    /// user into every repository they open.
    Agents,
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
    /// The conversation the CLI is actually on, when that is no longer [`Self::session`].
    ///
    /// `session` is cide's handle: it keys the registry, it is what the webview addresses a
    /// pane by, and it is stable for the pane's whole life. The CLI's conversation id is not
    /// stable — `--resume <parent>` runs under a fresh one rather than the parent's, and
    /// `/clear` starts another mid-session — and it is the one `~/.claude/projects/*.jsonl`
    /// is named after. Keeping both is what lets a restart resume the conversation the user
    /// was last looking at instead of the one the pane opened with, which is the whole of the
    /// "it always restarts with almost the first session that was in this panel" report: the
    /// parent's transcript is still on disk, so `cide-app`'s `restore_for` kept finding it
    /// and resuming a conversation two or three `/clear`s stale.
    ///
    /// Learned from hook frames, which carry both ids — see `HookFrame::spawned_as`.
    /// `#[serde(default)]` so a `workspace.json` written before this field loads unchanged.
    #[serde(default)]
    pub conversation: Option<SessionId>,
    /// When the CLI moved onto [`Self::conversation`], in milliseconds since the epoch.
    ///
    /// The stamp beside the id, and it exists for one question: *was this name given to the
    /// conversation the pane is on now, or to the one before it?* The CLI holds a `/rename`
    /// name on the **process** — `/clear` mints a new conversation inside the same `claude`
    /// and rewrites `~/.claude/sessions/<pid>.json` with the new `sessionId` and the old
    /// `name` still attached — so a name read back under this id may predate it. Anything not
    /// later than this instant belongs to a conversation the user has already cleared away.
    ///
    /// Wall clock rather than monotonic, because the only thing it is ever compared against is
    /// the CLI's own `nameSince`, which is `Date.now()`. Persisted for the same reason
    /// [`Self::conversation`] is: a restart must not resurrect the stale name.
    /// `cide_core::workspace::claude_name_cutoffs` is the one reader.
    #[serde(default)]
    pub conversation_since: Option<u64>,
    /// The harness conversation this pane was opened onto, when it was opened onto one. (M42)
    ///
    /// Set for a pane the Agents panel opened on a run — a mirror of the run's live child, or
    /// the real harness re-opened on its conversation — and `None` for every pane a user split
    /// themselves. It is what lets the pane act on its own afterwards: the exit bar's *Resume
    /// this conversation* and a restart re-open the same conversation from the same directory,
    /// and a restore after a cide restart does the same instead of spawning a fresh child in the
    /// project root over a worktree transcript it can no longer find.
    ///
    /// **Durable, and not a copy of the run.** The run's row is history that
    /// `AgentRegistry` prunes; the pane may outlive it by weeks. Everything a re-open needs is
    /// in [`crate::HarnessSession`], and nothing here says which run it was. `#[serde(default)]`
    /// so a `workspace.json` written before this field loads unchanged.
    #[serde(default)]
    pub continues: Option<crate::HarnessSession>,
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

/// One structured log line as it was written, for the pane that rendered a summary of it.
///
/// Both forms, because they answer different questions. `pretty` is what the card shows —
/// indented, one field per line, which is the reason somebody opened it. `raw` is what the
/// copy button puts on the clipboard: the bytes the program actually emitted, which is what
/// belongs in a ticket or a grep, and reformatting those on the way out would be quietly
/// changing somebody's evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogLineDetail {
    pub raw: String,
    pub pretty: String,
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
