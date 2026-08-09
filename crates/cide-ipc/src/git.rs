//! Git wire types: multi-root status, changelists, hunk/line selections, shelf and stash.
//!
//! Two vocabulary decisions run through everything here and are worth stating once.
//!
//! **Paths are repo-relative, slash-separated strings**, not `PathBuf`. A `ChangesTree` for
//! a monorepo carries thousands of them, the frontend only ever concatenates and compares
//! them, and git itself stores them exactly this way — so the wire form is the storage form
//! and no conversion can disagree with libgit2 about what a path is.
//!
//! **A [`Selection`] names positions in a [`FileDiff`], not content.** The frontend checks
//! boxes against the diff it was given; Rust re-derives the diff and applies the selection
//! to it. That means a selection is only meaningful against the diff it came from, which is
//! why [`FileDiff::rev`] exists — a stale selection is rejected rather than applied to
//! lines that have since moved.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::RepoId;

// --- repositories -----------------------------------------------------------------------

/// One git repository visible inside a project.
///
/// A project may hold several roots, and each root may contain submodules; both arrive here
/// as separate `RepoInfo`s. Submodules are not opaque entries — they are repositories with
/// their own changes, which is the only way a monorepo user can see what is actually dirty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RepoInfo {
    /// Derived from the canonical work-tree path, so it is stable across restarts and
    /// across the two processes that might compute it.
    pub id: RepoId,
    #[ts(type = "string")]
    pub root: PathBuf,
    /// The last path component, or the submodule's name — what the tree row shows.
    pub name: String,
    /// The containing repository, when this one is a submodule.
    pub parent: Option<RepoId>,
    pub is_submodule: bool,
}

/// Head, upstream and the divergence counts the status bar shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BranchInfo {
    /// Branch short name, or an abbreviated oid when detached.
    pub head: String,
    pub detached: bool,
    /// `origin/main`, when the branch has one.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// `merge`, `rebase`, `cherry-pick`, `revert`, `bisect` — whatever `.git` says is
    /// half-finished. The panel refuses to commit changelists while one is running.
    pub operation: Option<String>,
    /// No commits yet: `HEAD` points at an unborn branch.
    pub unborn: bool,
}

// --- status -----------------------------------------------------------------------------

/// What happened to one file on one side of the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FileState {
    Unmodified,
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChange,
    Untracked,
    Ignored,
    Conflicted,
}

/// One changed path.
///
/// Carries *both* sides — `index` is HEAD→index, `worktree` is index→workdir — because the
/// tri-state checkbox in the commit tree is a function of the pair, and collapsing them to
/// one status is what makes a partially-staged file render as a lie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChangeEntry {
    pub path: String,
    /// The pre-rename path, when git found a rename.
    pub orig_path: Option<String>,
    pub index: FileState,
    pub worktree: FileState,
    /// True when the index differs from HEAD for this path.
    pub staged: bool,
    /// libgit2 called at least one side binary. Hunk and line staging are refused.
    pub binary: bool,
    /// A gitlink. Its own changes appear under its own `RepoInfo`, not here.
    pub submodule: bool,
    /// The changelist that owns this path. Untracked and ignored entries carry the
    /// changelist they *would* join if staged, which is always the active one.
    pub changelist: String,
}

/// One changelist and the changes currently in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChangelistView {
    pub id: String,
    pub name: String,
    pub comment: String,
    /// Exactly one changelist per repo is active; new changes land in it.
    pub active: bool,
    pub changes: Vec<ChangeEntry>,
}

/// Everything the commit tool window draws for one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RepoChanges {
    pub repo: RepoInfo,
    pub branch: BranchInfo,
    /// The default list is first; the rest keep their creation order.
    pub changelists: Vec<ChangelistView>,
    pub unversioned: Vec<ChangeEntry>,
    pub ignored: Vec<ChangeEntry>,
    pub conflicts: Vec<ChangeEntry>,
    /// The index no longer matches what cide last wrote to it — someone ran `git add` in a
    /// pane. Drives the non-blocking "staging changed outside cide" bar; committing while
    /// this is true needs an explicit override.
    pub index_changed_externally: bool,
    /// IDEA's "use Git staging area instead" toggle. In this mode the index is the truth
    /// and cide never rebuilds it from a changelist.
    pub use_staging_area: bool,
}

/// The whole tree, one entry per repository, roots before their submodules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChangesTree {
    pub repos: Vec<RepoChanges>,
}

// --- the file tree's status column --------------------------------------------------------

/// The status the file tree draws in its one-letter tag column.
///
/// Deliberately *not* [`FileState`]. The commit panel needs both sides of the index because a
/// tri-state checkbox is a function of the pair; the file tree has one 9px column and has to
/// answer "what happened to this path" with a single letter. Two types rather than one lossy
/// shared one, because collapsing the pair is exactly the bug `ChangeEntry` exists to avoid
/// and re-deriving that collapse in TypeScript would put the rule somewhere it cannot be
/// tested against a real repository.
///
/// The variants are the mock's — `M` blue, `A` green, `D` faint and struck through — plus
/// the two the tree can hold that the mock does not draw. Every one of them is reachable;
/// see `cide_git::tree_status` for what produces each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TreeStatus {
    /// Tracked and unchanged. Never travels in a [`TreeStatusMap`] — absence means this, and
    /// a 100k-file repository would otherwise ship 100k entries saying nothing.
    Clean,
    /// Changed against HEAD, or conflicted. Also what a directory gets when something under
    /// it changed.
    Modified,
    /// New in the index.
    Added,
    /// Deleted on one side while the path is still on disk — `git rm --cached`. A path
    /// deleted from the working tree has no row to tag, so this is the case that makes the
    /// mock's `D` reachable at all.
    Deleted,
    /// Present on disk, unknown to git.
    Untracked,
    /// Ignored by git but still walked by `cide-fs`. Reachable because the walk deliberately
    /// does not consult `.gitignore` files *above* a project root (see `cide_fs::filter`),
    /// so a root opened inside an ignored directory shows rows git considers ignored.
    Ignored,
}

/// Per-path git status for the file tree, keyed by the same absolute path a [`TreeRow`]
/// carries.
///
/// [`TreeRow`]: crate::TreeRow
///
/// # Why absolute paths and not repo-relative ones
///
/// Every other path on this wire is repo-relative, because that is how git stores them. This
/// one is not, and the reason is that its only consumer is the file tree, whose rows are
/// keyed by absolute path. Doing the rebasing in Rust — where the project's roots, the work
/// trees above them and the submodules below them are all known — keeps the frontend's lookup
/// a plain map hit. The alternative puts prefix arithmetic over three coordinate systems into
/// TypeScript, where it cannot be tested against a real repository and would silently produce
/// an untagged tree the first time a root was a symlink.
///
/// # Size
///
/// Bounded by the number of *changed* paths plus their ancestor directories, plus the paths
/// git reports as ignored — not by the size of the repository: a 100k-file checkout with four
/// edits ships a handful of entries, because an untracked or ignored *directory* travels as
/// one entry and is not recursed into. The pathological cases — a whole-tree `sed -i`, or an
/// ignore glob like `*.o` over an in-tree build — are capped, and [`Self::truncated`] says so
/// rather than letting the tail read as clean.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TreeStatusMap {
    /// Absolute path → status. Paths absent from the map are [`TreeStatus::Clean`], except
    /// under a directory that is itself `untracked` or `ignored`, whose descendants inherit
    /// it — see `ui/src/sidebar/treeStatus.ts`.
    pub statuses: std::collections::BTreeMap<String, TreeStatus>,
    /// The cap was hit and the map is a prefix in path order. The entries present are still
    /// correct; paths after the cut are simply untagged.
    pub truncated: bool,
}

// --- diffs ------------------------------------------------------------------------------

/// Which pair of trees a diff compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DiffSide {
    /// HEAD → index. What a commit would contain in staging-area mode.
    Staged,
    /// index → working tree. What `stage` operates on.
    Unstaged,
    /// HEAD → working tree. What the changelist panel shows and what `commit` selects from
    /// in changelist mode.
    Combined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LineOrigin {
    Context,
    Addition,
    Deletion,
}

/// One line of a hunk.
///
/// `no_newline` is the `\ No newline at end of file` marker, folded onto the line it
/// annotates rather than modelled as a line of its own. Keeping it as a separate row would
/// put a phantom entry in the middle of the indices a [`LineRef`] uses, and every off-by-one
/// there is a mis-staged line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffLineView {
    pub origin: LineOrigin,
    /// Without the leading `+`/`-`/` ` and without the trailing newline.
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffHunkView {
    /// Index into [`FileDiff::hunks`]. Named explicitly so a [`LineRef`] is readable in a
    /// log line without the surrounding array.
    pub index: u32,
    /// The `@@ -a,b +c,d @@ section` line as libgit2 rendered it.
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLineView>,
}

/// A per-file diff, and the only thing a [`Selection`] is meaningful against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub side: DiffSide,
    pub status: FileState,
    pub binary: bool,
    /// Unix mode bits, e.g. 33188 (0o100644). Zero when the side does not exist.
    pub old_mode: u32,
    pub new_mode: u32,
    pub hunks: Vec<DiffHunkView>,
    /// Hash of the exact patch text this diff was derived from.
    ///
    /// A selection carries it back, and staging refuses a mismatch. Without it, a user who
    /// checks three lines while Claude is mid-edit stages three *different* lines — the
    /// panel's selection would be applied to a diff it never saw.
    pub rev: String,
    /// False when the file cannot be partially staged at all — binary, a submodule, a
    /// symlink or a deletion. The UI hides the per-hunk affordances rather than offering a
    /// control that will be refused.
    pub partial_ok: bool,
}

// --- selections -------------------------------------------------------------------------

/// One line of one hunk of a [`FileDiff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct LineRef {
    pub hunk: u32,
    /// Index into [`DiffHunkView::lines`].
    pub line: u32,
}

/// How much of one file an operation touches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum Selection {
    /// The whole file, through the index API — the same thing `git add` does. Everything
    /// that patch synthesis gets wrong (binary, modes, submodules, symlinks, deletions)
    /// takes this path and is therefore exact by construction.
    Whole,
    /// Every line of the named hunks.
    Hunks { hunks: Vec<u32> },
    /// Individual lines. Context lines in the list are ignored; only additions and
    /// deletions can be selected.
    Lines { lines: Vec<LineRef> },
}

/// A file and the part of it an operation applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct PathSelection {
    pub path: String,
    pub selection: Selection,
    /// [`FileDiff::rev`] of the diff the selection was made against. `None` skips the
    /// staleness check, which is only correct for `Whole`.
    pub rev: Option<String>,
}

impl PathSelection {
    /// The whole file, with no staleness check — the common case for a checkbox on a row.
    pub fn whole(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            selection: Selection::Whole,
            rev: None,
        }
    }
}

// --- commit -----------------------------------------------------------------------------

/// What `git.commit` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct CommitRequest {
    pub message: String,
    pub amend: bool,
    /// Which changelist to commit. `None` means the active one.
    pub changelist: Option<String>,
    /// Restrict the commit to these paths and parts. `None` commits every change in the
    /// changelist whole.
    pub selections: Option<Vec<PathSelection>>,
    /// Commit even though the index changed outside cide — the "overwrite" half of the
    /// guard bar. Never defaulted to true anywhere.
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitOutcome {
    pub oid: String,
    pub summary: String,
    pub files: u32,
}

/// Result of `git.push`, including whatever the transport said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PushOutcome {
    pub remote: String,
    pub refspec: String,
    /// True when the push went through the `git` binary because a credential helper is
    /// configured — libgit2 cannot drive an interactive helper.
    pub shelled_out: bool,
    /// stderr of the `git push`, or libgit2's progress text. This is where "Everything
    /// up-to-date" and the remote's own messages live, and users read them.
    pub output: String,
}

// --- shelf and stash --------------------------------------------------------------------

/// One shelved patch. Ours, not git's — see `cide-git::shelf` for why they are separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShelfEntry {
    pub id: String,
    pub name: String,
    /// Unix seconds.
    pub created: i64,
    pub files: Vec<String>,
}

/// One entry of `git stash list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StashEntry {
    pub index: u32,
    pub message: String,
    pub oid: String,
}

// --- errors -----------------------------------------------------------------------------

/// Why a partial selection was refused.
///
/// These are the cases where synthesizing a unified diff is *silently* wrong rather than
/// loudly wrong, which is why each one is a distinct tag: the frontend has to be able to
/// explain the refusal, and "patch failed" explains nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PartialRefusal {
    /// libgit2 called a side binary. There are no lines to select.
    Binary,
    /// A gitlink. The "content" is a commit id; a hunk of it means nothing.
    Submodule,
    /// A symlink, or a regular↔symlink type change. The blob is a target path.
    TypeChange,
    /// The file is being deleted. A patch header that says `deleted file mode` cannot also
    /// keep lines, so a partial delete has to go through the index instead.
    Deletion,
    /// A rename. The rename headers and the content hunks are one indivisible delta.
    Rename,
    /// The selection would put a `\ No newline at end of file` marker somewhere other than
    /// the end of the patch, which is not a diff any tool can read.
    NoNewlineOrdering,
    /// Nothing selectable was selected — every named line was context, or the list was
    /// empty.
    Empty,
}

/// Errors crossing the IPC boundary from `cide-git`.
///
/// Every variant is a tag the frontend branches on. `IndexChangedExternally` in particular
/// is not an error the user should see as a failure — it is the guard bar, and the UI needs
/// to tell it apart from a genuine libgit2 failure without matching on prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    content = "detail"
)]
#[ts(export)]
pub enum GitError {
    /// The path is not inside a git repository.
    NotARepository {
        path: String,
    },
    /// A repo id that no root in this project resolves to.
    NoSuchRepo {
        repo: RepoId,
    },
    /// A bare repository has no working tree, so none of this applies.
    Bare {
        path: String,
    },
    /// `git.status` was asked about a project id nothing is open under.
    NoSuchProject {
        project: String,
    },

    /// The index differs from the one cide last wrote. Carries both fingerprints so a bug
    /// report can say whether they were ever equal.
    IndexChangedExternally {
        expected: String,
        actual: String,
    },

    /// A [`PathSelection`] named a diff that no longer exists.
    StaleSelection {
        path: String,
    },
    /// Partial staging is not possible for this file. See [`PartialRefusal`].
    PartialRefused {
        path: String,
        reason: PartialRefusal,
    },
    /// The path has no changes on the side the operation looks at.
    NoSuchChange {
        path: String,
    },

    /// libgit2 or `git apply` rejected the patch. `patch` is the exact text we synthesized,
    /// because a rejection here is the one failure mode that needs the input to diagnose.
    PatchRejected {
        detail: String,
        patch: String,
    },

    /// The working tree has unresolved conflicts.
    Conflicted {
        paths: Vec<String>,
    },
    /// A merge, rebase or cherry-pick is in progress.
    OperationInProgress {
        operation: String,
    },
    /// The commit would be empty.
    NothingToCommit,
    /// `amend` with no commit to amend.
    Unborn,

    NoSuchChangelist {
        id: String,
    },
    /// The default changelist cannot be deleted or renamed away.
    DefaultChangelist,
    /// A changelist name already in use in this repo.
    DuplicateChangelist {
        name: String,
    },
    NoSuchShelf {
        id: String,
    },
    NoSuchStash {
        index: u32,
    },

    /// `git push` failed. `output` is the transport's own text, which is what the user
    /// actually needs.
    Push {
        output: String,
    },

    Io {
        detail: String,
    },
    /// libgit2's message, with its class, e.g. `index: ...`.
    Git {
        detail: String,
    },
    /// The changelists sidecar could not be read or written.
    Sidecar {
        path: String,
        detail: String,
    },
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotARepository { path } => write!(f, "{path} is not inside a git repository"),
            Self::NoSuchRepo { repo } => write!(f, "no such repository: {repo}"),
            Self::Bare { path } => write!(f, "{path} is a bare repository"),
            Self::NoSuchProject { project } => write!(f, "no such project: {project}"),
            Self::IndexChangedExternally { .. } => {
                f.write_str("the git index changed outside cide")
            }
            Self::StaleSelection { path } => {
                write!(f, "the diff for {path} moved under the selection")
            }
            Self::PartialRefused { path, reason } => {
                write!(f, "{path} cannot be partially staged: {reason:?}")
            }
            Self::NoSuchChange { path } => write!(f, "{path} has no changes to apply"),
            Self::PatchRejected { detail, .. } => write!(f, "patch rejected: {detail}"),
            Self::Conflicted { paths } => {
                write!(f, "{} path(s) are unresolved", paths.len())
            }
            Self::OperationInProgress { operation } => write!(f, "a {operation} is in progress"),
            Self::NothingToCommit => f.write_str("nothing to commit"),
            Self::Unborn => f.write_str("this branch has no commits yet"),
            Self::NoSuchChangelist { id } => write!(f, "no such changelist: {id}"),
            Self::DefaultChangelist => f.write_str("the default changelist cannot be removed"),
            Self::DuplicateChangelist { name } => write!(f, "a changelist named {name} exists"),
            Self::NoSuchShelf { id } => write!(f, "no such shelved change: {id}"),
            Self::NoSuchStash { index } => write!(f, "no such stash entry: {index}"),
            Self::Push { output } => write!(f, "push failed: {output}"),
            Self::Io { detail } | Self::Git { detail } => f.write_str(detail),
            Self::Sidecar { path, detail } => write!(f, "{path}: {detail}"),
        }
    }
}

impl std::error::Error for GitError {}
