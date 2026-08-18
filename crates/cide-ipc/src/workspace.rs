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
    pub const CURRENT_SCHEMA: u32 = 4;
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
