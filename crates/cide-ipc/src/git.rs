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

/// An absolute path resolved to the repository that contains it. (M18)
///
/// Absolute in, repo-relative out. Every surface that starts from a *file* — blame on the open
/// editor, "show history for this file" from the tree, a revision diff opened from a terminal
/// link — has an absolute path in hand and needs the pair this carries before it can ask
/// `cide-git` anything, because every other path on this wire is repo-relative.
///
/// # Why the arithmetic is in Rust
///
/// The same reason [`TreeStatusMap`] gives for going the other way, and it is the reason that
/// matters more here because this direction is the one that silently produces a *wrong* answer
/// rather than an untagged row. A prefix comparison in TypeScript is wrong for a symlinked root
/// (the path the user opened and the path git canonicalised are different strings that name one
/// directory), wrong for a nested submodule (the innermost repository owns the path, and the
/// superproject's root is also a prefix of it), and wrong for a path that is inside no
/// repository at all — which TypeScript would answer by producing a relative path against
/// whichever root sorted first. Rust knows the project's roots, the work trees above them and
/// the submodules below them, so it can answer the question or refuse it with
/// [`GitError::NotARepository`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RepoPath {
    /// The innermost repository containing the path — the submodule, never its superproject.
    pub repo: RepoId,
    /// Repo-relative and slash-separated, the way every other path in this module is spelled.
    pub path: String,
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
    /// The commit the caller believes it is amending. Only meaningful with `amend`. (M18)
    ///
    /// `None` keeps the behaviour the commit panel's Amend checkbox has always had — amend
    /// whatever HEAD is — which is right there, because that checkbox is *about* HEAD and cannot
    /// name anything else. The **log** can: its menu offers Amend on one row, and between the
    /// menu opening and the confirm landing a `git commit` in a bash pane can move HEAD out from
    /// under it. So the oid travels with the request and is checked in `cide_git::commit`,
    /// against the repository, rather than against a list drawn a second ago.
    ///
    /// A mismatch is [`GitError::NotHead`] and never a rewrite: amending anything but HEAD is an
    /// interactive rebase, and cide has no conflict-resolution surface to finish one — the same
    /// argument `branch::pull` makes for being fast-forward-only.
    ///
    /// `#[serde(default)]` so every caller that predates this field, and every `workspace.json`
    /// that never carried it, still deserialises.
    #[serde(default)]
    pub amend_of: Option<String>,
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

// --- branches ---------------------------------------------------------------------------

/// One branch as the selector draws it.
///
/// Local and remote-tracking branches share this shape because the popup treats them the
/// same way: both are things you can check out, branch from, and read a tip line for. The
/// difference is [`Self::remote`], and it is what decides whether *Rename* and *Delete* are
/// offered — those write `refs/heads/`, and a remote-tracking ref is a cache of somebody
/// else's repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BranchRef {
    /// Short name: `main`, or `origin/main` for a remote-tracking branch.
    pub name: String,
    pub remote: bool,
    /// This is `HEAD`. At most one entry in a [`BranchList`] has it.
    pub current: bool,
    /// `origin/main`, for a local branch that has one.
    pub upstream: Option<String>,
    /// Commits on this branch that its upstream does not have, and vice versa. Both zero
    /// for a remote-tracking branch, which is nobody's downstream.
    pub ahead: u32,
    pub behind: u32,
    /// Abbreviated tip oid, 8 hex digits.
    pub tip: String,
    /// Summary line of the tip commit — the second line of a branch row.
    pub subject: String,
    /// Committer time of the tip, unix seconds. The popup sorts by it, because the branch
    /// you want next is nearly always one of the handful you touched last.
    pub committed: i64,
}

/// Every branch of one repository, plus what `HEAD` is doing.
///
/// One of these per repository, because a project can hold several roots and each root its
/// submodules — and in a superproject the branch you mean is genuinely ambiguous. The
/// selector names the repository whenever there is more than one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BranchList {
    pub repo: RepoInfo,
    /// The same [`BranchInfo`] the status bar reads, so the widget and the popup cannot
    /// disagree about what is checked out.
    pub head: BranchInfo,
    /// Local branches, most recently committed first.
    pub local: Vec<BranchRef>,
    /// Remote-tracking branches, same order. `origin/HEAD` is dropped: it is a symbolic ref
    /// that duplicates whichever branch it points at.
    pub remote: Vec<BranchRef>,
}

/// What a checkout should do about local changes that stand in its way.
///
/// There is deliberately **no `Force`**. Discarding uncommitted work is the one thing this
/// crate exists to never do by accident, and a dropdown in a status bar is the last place it
/// should be one click away; `git_rollback` is the command that destroys work and it says so.
/// Everything here either refuses or preserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CheckoutMode {
    /// Refuse with [`GitError::CheckoutWouldOverwrite`], which names the files. The default,
    /// and the only mode the first click ever uses.
    Refuse,
    /// `git stash -u`, then switch. The changes stay in the stash for the user to pop —
    /// IDEA's *Stash*.
    Stash,
    /// Stash, switch, then pop. IDEA's *Smart checkout*: the changes come along.
    ///
    /// The pop can conflict, because the files it touches are exactly the ones that differ
    /// between the two branches. That is reported in [`CheckoutOutcome::restore_failed`]
    /// rather than swallowed, and the stash is still there either way.
    StashAndRestore,
}

/// What a checkout actually did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckoutOutcome {
    /// The local branch that is now checked out.
    pub branch: String,
    /// A local branch was created to track a remote one — `origin/feature` → `feature`.
    pub created_from_remote: Option<String>,
    /// The stash entry that was made, when the mode asked for one.
    pub stashed: Option<String>,
    /// The stash was made, the switch succeeded, and the changes could **not** be put back.
    /// They are in `git stash list`; this is git's own message for why.
    pub restore_failed: Option<String>,
}

/// One commit a pull brought down.
///
/// Deliberately not a `Commit` DTO with a full author record and a body: this exists to be
/// read in a toast that is four lines high. The short oid is what a user pastes into
/// `git show`, the summary is what they recognise the change by, and the author is what tells
/// them whether it is theirs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PulledCommit {
    /// Eight hex characters, the same width [`crate::git::BranchRef::tip`] uses.
    pub short_oid: String,
    /// The first line of the message, with no trailing newline.
    pub summary: String,
    /// The author's name. Not the email — the toast has one line per commit.
    pub author: String,
}

/// Result of a fetch or a fast-forward pull.
///
/// # Why this carries more than `advanced`
///
/// It used to carry `remote`, `shelled_out`, `output` and `advanced`, and the only sentence
/// anything could build from that was *"Fast-forwarded 7 commits from origin"*. That answers
/// "did it work"; it does not answer "what did I just take", which is the question a person
/// asks after pulling into a tree they are about to build. The three answers a pull can give
/// — *nothing came down*, *this came down*, and *I refused because you have diverged* — must
/// be told apart at a glance, and the middle one is the one that needs contents.
///
/// The fields below the transport half are therefore the pull's own report: which branch
/// moved, from which commit to which, what the diff between them totals, and the commits
/// themselves. A plain fetch fills in none of it, because a fetch moves no working tree —
/// see [`FetchOutcome::fetched`], which says so in one place instead of at every call site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FetchOutcome {
    pub remote: String,
    /// True when the `git` binary did it because a credential helper is configured — the
    /// same rule [`PushOutcome::shelled_out`] records for pushes.
    pub shelled_out: bool,
    /// git's own text. `Everything up-to-date`, or the transport's complaint, lives here.
    ///
    /// **Untrusted on the binary route.** `git fetch`'s stderr carries the server's `remote:`
    /// sideband lines verbatim, which are written by whoever runs that server. It is shown
    /// and nothing else: never parsed into an action, never used to decide what the app will
    /// do next. The frontend collapses it to one line and caps it (`branchModel::oneLine`).
    pub output: String,
    /// How many commits the fast-forward moved `HEAD`. Zero for a plain fetch, and for a
    /// pull that had nothing to take.
    pub advanced: u32,
    /// The branch the pull moved. Empty for a fetch, which moves none.
    ///
    /// Filled in even when `advanced` is zero, so the frontend can say *"main is already up
    /// to date"* rather than the anonymous *"already up to date"* a fetch gets.
    pub branch: String,
    /// Where that branch was before, short. Empty for a fetch.
    pub old_oid: String,
    /// Where it is now, short. Equal to `old_oid` when nothing came down.
    pub new_oid: String,
    /// `git diff --shortstat` between the two, over the whole fast-forward.
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
    /// The commits taken, newest first, **capped in Rust**.
    ///
    /// Capped here and not in the frontend on purpose: a 400-commit pull after a fortnight
    /// away must not put 400 rows on the IPC wire to have 390 of them dropped by a `slice`
    /// in a component.
    pub commits: Vec<PulledCommit>,
    /// How many more there were beyond `commits`. Zero when the list is complete.
    pub more_commits: u32,
}

impl FetchOutcome {
    /// The answer a plain fetch gives: the transport half filled in, the pull half empty.
    ///
    /// A fetch writes remote-tracking refs and moves nothing else, so `branch`, both OIDs,
    /// the three diff totals and the commit list are genuinely vacant rather than merely
    /// unset — and they are vacant in one place here instead of being spelled out zero by
    /// zero at each of the two routes in `cide_git::branch::fetch_with`.
    pub fn fetched(remote: String, shelled_out: bool, output: String) -> Self {
        Self {
            remote,
            shelled_out,
            output,
            advanced: 0,
            branch: String::new(),
            old_oid: String::new(),
            new_oid: String::new(),
            files_changed: 0,
            insertions: 0,
            deletions: 0,
            commits: Vec::new(),
            more_commits: 0,
        }
    }
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

    // --- branches ---
    /// No branch of that name, local or remote-tracking.
    NoSuchBranch {
        name: String,
    },
    /// Creating a branch over one that exists. Never forced: two people's `feature/login`
    /// are two different things and silently moving the ref loses one of them.
    BranchExists {
        name: String,
    },
    /// `git check-ref-format` would reject it — a space, `..`, a trailing `.lock`.
    InvalidBranchName {
        name: String,
    },
    /// Deleting a branch that `HEAD` cannot reach — `git branch -d`'s refusal.
    ///
    /// **It does not mean the commits are on no other branch**, and no message built from this
    /// variant may say so: the test behind it is "is this branch's tip an ancestor of where I
    /// am standing", so a branch already merged into `release` while you sit on `main` lands
    /// here with nothing whatsoever at stake. Saying "deleting it loses them" there is a false
    /// statement attached to a red button, which is how a user learns to stop reading them.
    /// The honest phrasing is git's own: not fully merged into the current branch.
    BranchNotMerged {
        name: String,
    },
    /// Deleting the branch that is checked out. git refuses this too, and the useful answer
    /// is "switch first" rather than an error code.
    BranchIsCurrent {
        name: String,
    },
    /// The switch would overwrite local changes. **The whole point of this variant is
    /// `paths`**: "checkout failed" is unactionable, and the files that are in the way are
    /// the ones the user has to decide about. Only paths that genuinely block are listed —
    /// a modified file that is identical on both branches comes along and is not here.
    CheckoutWouldOverwrite {
        branch: String,
        paths: Vec<String>,
    },
    /// A pull that is not a fast-forward. cide does not merge or rebase for you — there is
    /// no conflict-resolution surface yet — so it says how far the two have diverged and
    /// stops.
    NotFastForward {
        branch: String,
        ahead: u32,
        behind: u32,
    },
    /// The branch has no upstream, so there is nothing to pull from.
    NoUpstream {
        branch: String,
    },
    /// `HEAD` is not on a branch, so there is nothing to fast-forward.
    ///
    /// Split out of [`Self::NoUpstream`], which used to swallow it: a detached `HEAD` was
    /// told *"a1b2c3d4 has no upstream branch to pull from"*, which names a commit as though
    /// it were a branch and suggests the fix is `--set-upstream`. It is not; the fix is to
    /// check out a branch. `head` is the short oid, so the sentence can name where you are.
    DetachedHead {
        head: String,
    },
    /// No remote by that name is configured.
    ///
    /// Raised before a route is chosen, because both routes answer this badly on their own:
    /// libgit2 says `Config: remote 'origin' does not exist` and the `git` binary says
    /// `'origin' does not appear to be a git repository`. Both are true; neither is the
    /// sentence for a repository that simply has no remote yet.
    NoRemote {
        name: String,
    },
    /// `git fetch` failed. Same shape as [`Self::Push`] and for the same reason: the
    /// transport's own text is what the user needs.
    Fetch {
        output: String,
    },

    /// `git push` failed. `output` is the transport's own text, which is what the user
    /// actually needs.
    Push {
        output: String,
    },

    // --- history: the Git tool window (M18) ---
    //
    // These live in `GitError` and not in a `HistoryError` beside `cide_ipc::history`, even
    // though every type they are raised by does. The enum is **one closed list on purpose**: a
    // panel that has to catch two error unions from the same repository will get the second
    // one's arms wrong, and it will get them wrong silently, because an unmatched tag falls into
    // whatever the catch-all prints. Every refusal in the Git tool window is a git refusal, and
    // the frontend already has exactly one place that turns one of these into a sentence.
    /// The path is inside a repository but git has never heard of it, so there is no history
    /// and no blame to show. Distinct from [`Self::NoSuchChange`], which is about a path git
    /// *does* track that simply has nothing pending.
    NotTracked {
        path: String,
    },
    /// A revspec resolved to nothing. `rev` is what was asked for, verbatim, because the user
    /// typed it and the sentence has to quote it back.
    NoSuchRevision {
        rev: String,
    },
    /// A revspec resolved to an object that is not in this repository, or an oid that is not
    /// present. Split from [`Self::NoSuchRevision`] because "I cannot parse that" and "that
    /// commit is not here" send the user to different places — the second is what a shallow
    /// clone and a pruned branch both produce.
    NoSuchCommit {
        rev: String,
    },
    /// The blob is past the size cap. Both numbers, because "too large" without them is a
    /// refusal the user cannot act on and cannot tell apart from a bug.
    FileTooLarge {
        path: String,
        bytes: u64,
        limit: u64,
    },
    /// The operation requires the commit to be `HEAD` and it is not — `HEAD` moved between the
    /// log row being drawn and the button being pressed, which on a repository someone else is
    /// pushing to is a matter of seconds. Carries both so the message can say which is which
    /// rather than "stale".
    NotHead {
        oid: String,
        head: String,
    },
    /// A revert or cherry-pick would conflict. **`paths` is the whole point**: cide has no
    /// conflict-resolution surface yet, so it refuses rather than leaving the tree half-applied
    /// with no way to finish — and a refusal that does not name the files is one the user cannot
    /// act on.
    ReplayWouldConflict {
        op: crate::history::ReplayOp,
        oid: String,
        paths: Vec<String>,
    },
    /// Reverting or cherry-picking a merge without saying which parent is the mainline.
    ///
    /// Carries the parents themselves rather than only their count, because the question the
    /// user is being asked is *which side do you want to keep*, and that is unanswerable from a
    /// number. [`PulledCommit`] is already the four-line summary shape a chooser needs.
    MergeNeedsMainline {
        oid: String,
        parents: Vec<PulledCommit>,
    },
    /// A mainline was given for a commit that is not a merge — git's own refusal, kept because
    /// silently ignoring the argument would make a mis-wired button look like it worked.
    NotAMerge {
        oid: String,
    },
    /// The replay would produce no change: the patch is already applied (for a cherry-pick) or
    /// already reverted. Not an error in git's sense, but it must not look like success — an
    /// empty commit created here is a commit the user has to explain in review.
    EmptyReplay {
        op: crate::history::ReplayOp,
        oid: String,
    },
    /// A tag of that name exists. Never forced implicitly: a tag is a published promise about
    /// which commit a release is, and moving one silently is how two people build different
    /// `v1.2.0`s. `oid` is where it points now, so the confirmation can say what would be lost.
    TagExists {
        name: String,
        oid: String,
    },
    /// `git check-ref-format` would reject it. The tag-name twin of
    /// [`Self::InvalidBranchName`].
    InvalidTagName {
        name: String,
    },
    /// A revspec that does not parse. `detail` is libgit2's own complaint, which names the
    /// character it stopped at and is the only part of this a user can act on.
    BadRevspec {
        spec: String,
        detail: String,
    },
    /// A revspec that parses and resolves to something that is not a commit — a tree, a blob, an
    /// annotated tag pointing at a blob. `kind` is git's own word for what it found, so the
    /// message can say *`v1.2.0^{tree}` is a tree* rather than "not a commit".
    NotACommit {
        spec: String,
        kind: String,
    },
    /// An abbreviated oid that matches more than one object. Its own variant rather than a
    /// [`Self::BadRevspec`] because the fix is specific and mechanical: type more characters.
    AmbiguousRev {
        spec: String,
    },
    /// A [`crate::history::LogResume`] token this build cannot use — a version it does not know,
    /// a fingerprint from a different query, or a frontier naming commits that a rewrite or a
    /// `gc` has removed.
    ///
    /// Carries nothing on purpose. Every field a caller might want here is either already in the
    /// token they still hold or is an internal detail of the walker, and the only correct
    /// response is the same in all three cases: restart from [`crate::history::LogCursor::
    /// Newest`]. A variant that invited a caller to branch on *why* would be inviting a retry
    /// loop with the same dead token.
    StaleLogCursor,

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

/// The English for a [`crate::history::ReplayOp`], for the two arms of [`GitError`]'s
/// `Display` that name one.
///
/// Here rather than as a method on `ReplayOp`, because `cide-ipc` is wire shapes and the only
/// prose in it is this `Display` impl — a `fn label()` hanging off the enum would read as a
/// general-purpose display name and get reached for by a frontend that should be choosing its
/// own words in its own language file.
fn replay_verb(op: crate::history::ReplayOp) -> &'static str {
    match op {
        crate::history::ReplayOp::Revert => "reverting",
        crate::history::ReplayOp::CherryPick => "cherry-picking",
    }
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
            Self::NoSuchBranch { name } => write!(f, "no such branch: {name}"),
            Self::BranchExists { name } => write!(f, "a branch named {name} already exists"),
            Self::InvalidBranchName { name } => write!(f, "{name} is not a valid branch name"),
            Self::BranchNotMerged { name } => {
                write!(f, "{name} is not fully merged into the current branch")
            }
            Self::BranchIsCurrent { name } => write!(f, "{name} is the branch you are on"),
            Self::CheckoutWouldOverwrite { branch, paths } => write!(
                f,
                "switching to {branch} would overwrite {} local change(s): {}",
                paths.len(),
                paths.join(", ")
            ),
            Self::NotFastForward {
                branch,
                ahead,
                behind,
            } => write!(
                f,
                "{branch} is {ahead} ahead and {behind} behind its upstream, so this is not a fast-forward"
            ),
            Self::NoUpstream { branch } => write!(f, "{branch} has no upstream branch"),
            Self::DetachedHead { head } => write!(f, "HEAD is detached at {head}"),
            Self::NoRemote { name } => write!(f, "no remote named {name}"),
            Self::Fetch { output } => write!(f, "fetch failed: {output}"),
            Self::Push { output } => write!(f, "push failed: {output}"),
            Self::NotTracked { path } => write!(f, "{path} is not tracked by git"),
            Self::NoSuchRevision { rev } => write!(f, "no such revision: {rev}"),
            Self::NoSuchCommit { rev } => write!(f, "no such commit: {rev}"),
            Self::FileTooLarge { path, bytes, limit } => write!(
                f,
                "{path} is {bytes} bytes, over the {limit} byte limit for this view"
            ),
            Self::NotHead { oid, head } => {
                write!(f, "{oid} is no longer HEAD; HEAD is now {head}")
            }
            Self::ReplayWouldConflict { op, oid, paths } => write!(
                f,
                "{} of {oid} would conflict in {} path(s): {}",
                replay_verb(*op),
                paths.len(),
                paths.join(", ")
            ),
            Self::MergeNeedsMainline { oid, parents } => write!(
                f,
                "{oid} is a merge of {} parents; say which one to keep",
                parents.len()
            ),
            Self::NotAMerge { oid } => write!(f, "{oid} is not a merge commit"),
            Self::EmptyReplay { op, oid } => {
                write!(f, "{} of {oid} would change nothing", replay_verb(*op))
            }
            Self::TagExists { name, oid } => {
                write!(f, "the tag {name} already points at {oid}")
            }
            Self::InvalidTagName { name } => write!(f, "{name} is not a valid tag name"),
            Self::BadRevspec { spec, detail } => write!(f, "{spec} is not a revision: {detail}"),
            Self::NotACommit { spec, kind } => write!(f, "{spec} is a {kind}, not a commit"),
            Self::AmbiguousRev { spec } => {
                write!(f, "{spec} matches more than one object")
            }
            Self::StaleLogCursor => f.write_str("this log page cannot be continued"),
            Self::Io { detail } | Self::Git { detail } => f.write_str(detail),
            Self::Sidecar { path, detail } => write!(f, "{path}: {detail}"),
        }
    }
}

impl std::error::Error for GitError {}
