//! Wire types for the Git tool window: the commit log and its graph, one commit's contents,
//! a file's revisions, blame, and the commit-level actions. (M19)
//!
//! # Why this is not a section of `git.rs`
//!
//! [`crate::git`] is nine hundred lines and every type in it is organised around one idea,
//! which its own header states first: **a [`crate::git::Selection`] names positions in a
//! [`crate::git::FileDiff`], not content, and a stale one must be refused.** That single rule
//! is why [`crate::git::FileDiff`] carries a `rev`, why [`crate::git::PathSelection`] hands it
//! back, why [`crate::git::PartialRefusal`] enumerates every case where synthesising a patch
//! is *silently* wrong rather than loudly wrong, and why
//! [`crate::git::RepoChanges::index_changed_externally`] is on the wire at all. Every one of
//! those exists because the working tree moves under the user while they are looking at it.
//!
//! **Nothing in this module can move.** History is already frozen: a commit's contents are the
//! same bytes for ever, a diff between two commits cannot go stale, and a blame of a revision
//! is a fact about that revision. Nothing here is selectable, stageable or rev-checkable —
//! there is no `rev` field anywhere below and there must never be one, because there is no
//! failure for it to prevent. A history type living in `git.rs` would sit under a module header
//! promising a staleness contract that does not apply to it, and the first reader to go looking
//! for the check that "every type in this file" has would waste their time discovering that
//! this family is the exception.
//!
//! The precedent is `symbols`/`diagnostics`, split in M12 with the justification written into
//! each header rather than left implicit. The split there was by producer; the split here is by
//! job — `git.rs` is *mutate the working tree*, this is *read what is already committed* — and
//! the two have almost no vocabulary in common. What overlap there is crosses the boundary
//! explicitly and in one direction:
//!
//! * this module imports [`crate::git::FileState`], [`crate::git::DiffHunkView`],
//!   [`crate::git::PathSelection`], [`crate::git::PulledCommit`] and
//!   [`crate::git::ShelfEntry`], because a status letter, a hunk, a shelvable selection and a
//!   four-line commit summary mean exactly the same thing on both sides and a second spelling
//!   of any of them would be a second answer;
//! * [`crate::git::GitError`] grows arms for this module's failures and two of them name
//!   [`ReplayOp`]. That enum stays **one closed list** deliberately: a panel that has to catch
//!   two error unions will get the second one's arms wrong, and every refusal in the Git tool
//!   window is a git refusal.
//!
//! Paths obey `git.rs`'s rule without restating it: **repo-relative, slash-separated strings**,
//! because that is how git stores them and how the rest of this wire spells them. The one
//! exception is [`crate::git::RepoPath`], which is the conversion *into* that coordinate system
//! and says so itself.
//!
//! # Times are unix seconds
//!
//! Every timestamp here is `i64` unix seconds, matching [`crate::git::BranchRef::committed`],
//! and lands on the TypeScript side as `bigint` like every other 64-bit integer on this wire.
//! Signed, because git's own `git_time_t` is and pre-1970 committer dates exist in imported
//! histories; a `u64` would turn one of those into a date in the far future rather than an
//! obviously wrong one in the past.
//!
//! # Everything is paged, and every page says why it stopped
//!
//! A log over a real repository cannot be delivered whole and cannot be delivered blind. Two
//! separate budgets run over every walk — [`LogQuery::limit`], the rows wanted, and
//! [`LogQuery::scan_limit`], the commits the walk may *examine* — and they diverge the moment a
//! filter is on: a path filter over a file touched twice in forty thousand commits fills no page
//! at all until it has walked the repository. So every response carries a [`LogStop`] naming
//! which budget ended it, and the difference between *there is no more history* and *I ran out
//! of budget* is a distinction the panel has to draw, because one of them means the "load more"
//! affordance should disappear and the other means it must not.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git::{DiffHunkView, FileState, PathSelection, PulledCommit, ShelfEntry};
use crate::ids::RepoId;

// --- the walk -----------------------------------------------------------------------------

/// Which repositories one log walks.
///
/// A project can hold several roots and each root its submodules, and "show me the history"
/// means different things in the two cases — so the caller says which, rather than the walker
/// guessing from how many repositories happen to be open.
///
/// # A note that applies to every tagged union in this module
///
/// This type is inbound (it is a field of [`LogQuery`], which carries `deny_unknown_fields`)
/// and yet it has no `deny_unknown_fields` of its own. That is not an oversight: **serde does
/// not support `deny_unknown_fields` on an internally tagged enum**, because the tag has to be
/// read out of the same map the fields come from and the buffering that requires is
/// incompatible with the strict check. [`crate::git::Selection`] lacks it for exactly this
/// reason and always has. The consequence is worth knowing rather than rediscovering: a
/// frontend that misspells a field inside one of these variants gets a `missing field` error if
/// the field was required, and silence if it was optional. Requests therefore keep their
/// optional fields on the *struct* side, where the strict check does apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum LogScope {
    /// One repository. The graph is drawable and every parent edge is real.
    One { repo: RepoId },
    /// Several repositories, interleaved by commit time into one list.
    ///
    /// The rows carry [`CommitRow::repo`] so a reader can tell them apart, and the graph is
    /// switched off with [`GraphOff::Merged`] — there is no parent relation *between* two
    /// repositories, so any line drawn across the boundary would be a claim nothing supports.
    Merged { repos: Vec<RepoId> },
}

/// Where the walk starts from, before any cursor is applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum LogRefs {
    /// Whatever `HEAD` points at, including a detached one. The default view.
    Head,
    /// One branch by short name — `main`, or `origin/main` for a remote-tracking ref.
    Branch { name: String },
    /// Every ref: local branches, remote-tracking branches, tags and `HEAD`.
    ///
    /// This is the mode that makes the graph worth having, and also the one that makes it
    /// expensive: a repository with three hundred remote branches opens three hundred lanes
    /// before the first merge closes any of them, which is what [`LogQuery::graph_lanes`] and
    /// [`GraphRow::overflow`] are for.
    All,
    /// An arbitrary revspec — `v1.2..HEAD`, `origin/main^{}`, a bare oid.
    ///
    /// Resolved by libgit2, so it accepts exactly what `git log` accepts and fails the same
    /// way; a spec that does not parse is [`crate::git::GitError::BadRevspec`] and one that
    /// parses to something that is not a commit is
    /// [`crate::git::GitError::NotACommit`], which are genuinely different mistakes.
    Rev { spec: String },
}

/// Where in the walk this page begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum LogCursor {
    /// The tip. The first page of any view.
    Newest,
    /// Continue from a token the previous page handed back.
    ///
    /// The **only** correct way to page. The obvious alternative — "give me the commits older
    /// than this oid" — is wrong for a merged walk (two repositories have no common ordering to
    /// be "older" in) and wrong for a filtered one (the budget accounting, the followed path
    /// and the open graph lanes all have to survive the gap, and none of them is recoverable
    /// from an oid). See [`LogResume`].
    Resume { token: LogResume },
    /// Start at this commit, **including** it. What "reveal this commit in the log" scrolls to.
    At { oid: String },
    /// Start strictly after this commit.
    ///
    /// Distinct from [`Self::At`] because the two are used for opposite gestures: `At` centres
    /// a row the user asked to see, `After` re-anchors a refetch on a page whose first row is
    /// already drawn and must not be drawn twice.
    After { oid: String },
}

/// Why a walk stopped producing rows.
///
/// The reason a page needs this at all is that "no more rows" has six causes and only one of
/// them means the history is over. Collapsing them leaves the panel unable to decide whether to
/// keep the "load more" affordance, and — worse — unable to tell a shallow clone's floor from
/// the actual root commit, which reads to the user as *this project began here*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LogStop {
    /// The walk reached the root commit. There is nothing more, ever. The only value that
    /// justifies hiding "load more".
    Exhausted,
    /// [`LogQuery::limit`] rows were produced and the walk still has a frontier.
    Page,
    /// [`LogQuery::scan_limit`] commits were examined without filling the page. The page may be
    /// short or empty and there is still history behind it — this is the one a path filter over
    /// a long history produces, and showing it as `Exhausted` would tell the user their file
    /// has no older revisions when it has thousands.
    Budget,
    /// The walk hit the boundary of a shallow clone (`.git/shallow`).
    ///
    /// Deliberately not `Exhausted`. The commits below the graft point are not in this
    /// repository and never were; saying the history ended there is a false statement about the
    /// project, and the fix — `git fetch --unshallow` — is one the panel can actually offer.
    Shallow,
    /// The [`LogResume`] token named commits this repository no longer has, or was minted for a
    /// different query. A rewrite, a prune, a `gc`, or a token pasted between views. The rows
    /// present (if any) are what could still be walked; the caller should restart from
    /// [`LogCursor::Newest`] rather than retry.
    CursorLost,
    /// [`LogRefs`] named a branch or revspec that does not resolve. Zero rows, and not an
    /// error: a view pinned to a branch someone has since deleted should say so in the panel
    /// rather than raise a dialog.
    NoSuchRef,
}

/// How much history simplification the walk does — git's `--full-history` switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Simplify {
    /// git's own default. With a path filter, a merge whose result matches one parent is
    /// simplified down to that parent, so the log reads as the sequence of commits that
    /// actually changed the file rather than every merge that carried it along. The commits
    /// removed are counted into [`CommitRow::pruned`] rather than silently dropped.
    #[default]
    Default,
    /// `--full-history`: keep every commit that touched the path on any parent.
    ///
    /// Honest and nearly unreadable, which is why it is not the default and why it forces
    /// [`GraphOff::FullHistory`] — the lane structure of a full-history walk is a mesh.
    Full,
}

/// One request for a page of log.
///
/// Everything about a view is in here, and that is deliberate: the walker holds no per-view
/// state between calls, so two panels asking for two different filters cannot interfere and a
/// window that is closed mid-walk leaves nothing behind. The continuation that *does* have to
/// survive a call is the token in [`LogCursor::Resume`], which travels with the request rather
/// than being remembered by the process — see [`LogResume`] for why that is not merely tidier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct LogQuery {
    pub scope: LogScope,
    pub refs: LogRefs,
    pub cursor: LogCursor,
    /// Repo-relative path filter. `None` walks everything.
    ///
    /// Applied **per repository** under [`LogScope::Merged`], because a path is only meaningful
    /// inside the repository that stores it and the same relative path in two roots is two
    /// different files.
    pub path: Option<String>,
    /// Follow the path across renames — git's `--follow`.
    ///
    /// Only meaningful with [`Self::path`], and like git's, single-path only. Ignored when
    /// `path` is `None`; whether it was actually applied comes back as [`CommitPage::followed`]
    /// rather than being assumed from the request, so a silently-ignored flag is visible.
    pub follow: bool,
    pub simplify: Simplify,
    /// Walk only first parents — git's `--first-parent`.
    ///
    /// Turns a merge-heavy history into the sequence of things that landed on this branch,
    /// which is the reading most people want and almost nobody knows the flag for.
    pub first_parent: bool,
    /// Substring match against author name and email, case-insensitive. git's `--author`.
    pub author: Option<String>,
    /// Substring match against the commit message. git's `--grep`.
    ///
    /// A substring and not a regex, on purpose: a regex from a text field is a way to hand a
    /// user a walk that never finishes, and the search that people actually type is a ticket
    /// number.
    pub text: Option<String>,
    /// How many rows to produce. Capped in Rust — see `cide_git::history` — because a frontend
    /// bug that asks for a million rows must not be able to allocate them here.
    pub limit: u32,
    /// How many commits the walk may **examine** while producing those rows.
    ///
    /// The second budget, and the one that keeps a filtered walk bounded. Without it,
    /// `--follow` over a file touched twice in a forty-thousand-commit history walks the whole
    /// repository — on the IPC thread — to fill a page of two. With it, the page comes back
    /// short and honest with [`LogStop::Budget`], and the user's next scroll continues from the
    /// frontier instead of starting again.
    pub scan_limit: u32,
    /// Compute the lane structure. `false` skips it entirely rather than computing it and
    /// dropping it, which matters for the merged and filtered views where it would be
    /// discarded anyway.
    pub graph: bool,
    /// Cap on concurrently open lanes. Rows needing more set [`GraphRow::overflow`] and the
    /// excess is not drawn.
    ///
    /// A cap rather than "draw whatever there is" because the gutter is a fixed strip of chrome
    /// beside a list: a repository whose `--all` view opens two hundred lanes would otherwise
    /// size that strip to two hundred columns and leave four pixels for the commit summaries.
    pub graph_lanes: u16,
}

// --- the continuation ---------------------------------------------------------------------

/// Everything the walker needs to produce the *next* page, handed to the client and handed
/// back.
///
/// # Why the continuation is on the wire instead of in a server-side cursor
///
/// The obvious design keeps a live `git2::Revwalk` in a map keyed by a view id. It loses on
/// three counts, each of which was a real consideration rather than a hypothetical. A live
/// revwalk pins an open repository and a growing object cache per panel, and panels are cheap
/// to open and easy to forget to close — so the leak is the normal case, not the exceptional
/// one. It cannot survive the window being detached into another OS window, which is a gesture
/// this app treats as free. And it makes "the same view in two windows" two independent walks
/// over one mutable cursor, which is the class of bug the whole workspace-in-Rust design (ADR
/// 0002) exists to avoid.
///
/// A token is a value. It can be duplicated, dropped, or replayed, and the worst outcome of any
/// of those is a page that is refused with [`LogStop::CursorLost`].
///
/// # Why the resume types are the only structs here without `deny_unknown_fields`
///
/// The whole point of [`Self::version`] is that an older build must be able to *recognise* a
/// token it cannot use, and answer [`crate::git::GitError::StaleLogCursor`]. With
/// `deny_unknown_fields`, a token from a newer build fails inside serde before the version
/// check ever runs, and the caller gets a deserialisation error naming a field instead of the
/// one refusal it knows how to recover from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogResume {
    /// Format version of this token. Bumped whenever the shape below changes; a token whose
    /// version this build does not know is refused, not guessed at.
    pub version: u32,
    /// Fingerprint of the [`LogQuery`] this token belongs to — every field of it except the
    /// cursor.
    ///
    /// Checked before the token is used, because handing a token from one query to another
    /// produces a page that is neither: the frontier was built under one path filter and the
    /// budget accounting under another. The failure without this check is not a crash, it is a
    /// log that quietly shows the wrong commits, which is the worst kind.
    pub key: String,
    /// One entry per repository the scope named, in the scope's order.
    pub repos: Vec<RepoResume>,
    /// The graph's open columns, when the page that produced this token drew one.
    ///
    /// `None` when it did not. Resuming *with* a graph from a token that has no lane state
    /// cannot continue columns that were never opened, so the next page starts its lanes fresh
    /// and reports [`GraphOff::Rerooted`] rather than drawing edges into nothing.
    pub lanes: Option<LaneResume>,
}

/// One repository's position in a paged walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RepoResume {
    pub repo: RepoId,
    /// The walk's priority queue, serialised: the commits the next page continues from.
    ///
    /// Not "the last oid emitted". A history is a DAG, so after emitting a merge the walk is
    /// standing on several commits at once, and a single oid cannot say which. Restarting from
    /// one of them silently drops the other side of every merge.
    pub frontier: Vec<FrontierRef>,
    /// Oids already emitted or deliberately simplified away, so the next page cannot emit one
    /// twice.
    ///
    /// Bounded rather than the whole walk's history: a commit can only be reached again through
    /// a frontier entry, so the set that has to survive is the one reachable from the frontier,
    /// not everything ever seen. `cide_git::history` prunes it on the way out.
    pub hidden: Vec<String>,
    /// Commit time of the last row emitted from this repository, unix seconds. Under
    /// [`LogScope::Merged`] this is the key the interleave orders by.
    pub watermark: i64,
    /// The oids emitted at exactly [`Self::watermark`].
    ///
    /// Needed because commit times collide constantly — a rebase writes an entire branch inside
    /// one second, and a scripted import writes thousands. A watermark alone therefore cannot
    /// say which of the ties were already shown, and the page boundary either repeats them or
    /// swallows them. This is the field that makes paging a merged walk exact instead of nearly
    /// right.
    pub recent: Vec<String>,
    /// Commits this repository has examined **across every page so far**, so
    /// [`LogQuery::scan_limit`] is a budget for the walk rather than for one call. Without the
    /// carry, a filter that finds nothing turns into an unbounded walk one page-worth at a
    /// time.
    pub scanned: u32,
    /// This repository is exhausted. The next page skips it entirely instead of re-opening it
    /// to discover the same thing.
    pub done: bool,
}

/// One commit on the walk's frontier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FrontierRef {
    pub oid: String,
    /// The sort key — commit time, unix seconds.
    ///
    /// Named `key` and not `time` because ordering is the only thing it is for. A later
    /// `--topo-order` or `--date-order` option keeps this field and changes what fills it,
    /// where a field called `time` would have to either lie or be joined by a second one.
    pub key: i64,
    /// The path filter **as it stands at this commit**, after every rename followed so far.
    ///
    /// A `--follow` walk changes the name it is looking for as it goes back in time. A resumed
    /// page that restarted from the path the user typed would stop finding the file at exactly
    /// the rename the follow was for — and it would stop *quietly*, as an apparently ordinary
    /// [`LogStop::Exhausted`]. A `Vec` because a rename can be ambiguous mid-walk and both
    /// candidates have to stay live until one of them dies out.
    pub paths: Vec<String>,
}

/// The graph's open columns at a page boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaneResume {
    pub lanes: Vec<OpenLaneWire>,
    /// The colour cursor, so the first row of the next page continues the rotation instead of
    /// restarting it. Restarting recolours every open branch at each page boundary, which as
    /// the user scrolls looks exactly like the graph redrawing itself at random.
    pub next_color: u16,
}

/// One lane left open at a page boundary.
///
/// Named `…Wire` because `cide-git` has its own richer `OpenLane` with the walker's bookkeeping
/// in it; this is the part that has to survive the round trip, and keeping the names distinct
/// stops the two being confused at the seam where one is built from the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpenLaneWire {
    /// Which column, left to right from zero.
    pub column: u16,
    /// The commit this lane is waiting for. The row that draws that oid is the row that closes
    /// this lane.
    pub awaits: String,
    /// Index into the renderer's palette, not a colour value. The palette is a CSS concern and
    /// putting an `#rrggbb` on the wire would make the graph ignore the theme.
    pub color: u16,
}

// --- rows -----------------------------------------------------------------------------------

/// What kind of ref a chip on a commit row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RefKind {
    /// `HEAD` itself, drawn only when it is detached — an attached `HEAD` is already implied by
    /// the branch chip beside it, and drawing both puts two chips on the same commit saying the
    /// same thing.
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

/// One ref pointing at a commit, as the log draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RefChip {
    pub kind: RefKind,
    /// Short name: `main`, `origin/main`, `v1.2.0`.
    pub name: String,
    /// The full refname — `refs/heads/main`, `refs/tags/v1.2.0`.
    ///
    /// Both, because the short name is genuinely ambiguous: a branch and a tag may both be
    /// called `release`, and every action the chip's context menu offers (checkout, delete,
    /// push) has to name the one the user clicked rather than whichever git resolves first.
    pub full: String,
    /// This is where `HEAD` points.
    pub current: bool,
}

/// One row of the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitRow {
    /// Which repository this commit is from.
    ///
    /// Present even under [`LogScope::One`], where it is constant. The row component does not
    /// know the scope, and every follow-up call it can make — open the commit, diff a file,
    /// revert — needs a repository to name; deriving it from ambient state is how a click in a
    /// submodule's log ends up acting on the superproject.
    pub repo: RepoId,
    /// The full 40-hex oid.
    ///
    /// Full and not abbreviated, because this is what every follow-up call passes back. An
    /// abbreviation is only unique until the repository grows, and a log page rendered on
    /// Monday must not become ambiguous on Friday.
    pub oid: String,
    /// What is drawn, abbreviated by libgit2's own uniqueness rule so it cannot disagree with
    /// what `git log` prints in the same repository.
    pub short_oid: String,
    /// First line of the message, no trailing newline.
    pub summary: String,
    pub author: String,
    /// Kept beside the name because the avatar and the "commits by this person" filter both key
    /// on the address — two people share a name far more often than an address.
    pub author_email: String,
    /// Author time and commit time, unix seconds.
    ///
    /// Both, because a rebase or a cherry-pick leaves them different and each answers a
    /// different question: the log *sorts* by committed and a reader wants to know when the
    /// work was *done*. Showing only one is how a rebased branch appears to have been written
    /// in the future.
    pub authored: i64,
    pub committed: i64,
    /// Full oids of the parents, in git's order. More than one is a merge, and the index into
    /// this list is what [`DiffAgainst::Parent`] names.
    pub parents: Vec<String>,
    /// How many commits history simplification removed between this row and the one below it.
    ///
    /// Zero without a filter. Non-zero, the graph draws a [`GraphEdge::Bundle`] and the row can
    /// say "3 commits hidden" — which is the difference between a filtered log that looks like
    /// the whole history and one that admits it is a summary.
    pub pruned: u32,
    /// Branch, tag and `HEAD` chips pointing at this commit. Usually empty.
    pub refs: Vec<RefChip>,
}

/// A point at which a followed path changed name.
///
/// Carried in a separate list on the page ([`CommitPage::renames`]) rather than as a field on
/// [`CommitRow`], because the hop belongs *between* two rows: the panel draws it as a separator,
/// and a rename that falls exactly on a page boundary has to be attachable to the boundary
/// rather than to a row that is not in this page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RenameHop {
    pub repo: RepoId,
    /// The commit that did the renaming.
    pub oid: String,
    /// The old and new repo-relative paths, in walk order: `from` is the name the file has in
    /// the *older* commits, `to` the name it has from this commit forwards.
    pub from: String,
    pub to: String,
    /// git's rename score, 0–100. Below the detection threshold there is no hop at all; the
    /// number is here so the UI can mark a 51 % match as the guess it is.
    pub similarity: u16,
}

/// One repository's contribution to a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RepoPage {
    pub repo: RepoId,
    /// The repository's display name, denormalised onto the page.
    ///
    /// So that "the walk ran out of budget in cide-git" can be said without a second lookup.
    /// The alternative is a frontend that has to join every page against the repository list
    /// before it can render a status line, and a page that arrives before that list — which is
    /// the ordinary case on first paint — renders a uuid.
    pub name: String,
    pub stop: LogStop,
    /// Commits examined for this page, this repository. Not cumulative — the running total
    /// lives in [`RepoResume::scanned`], where the budget is enforced.
    pub scanned: u32,
}

/// One page of log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitPage {
    /// Newest first.
    pub commits: Vec<CommitRow>,
    pub graph: LogGraph,
    /// The continuation, or `None` when every repository in the scope is exhausted.
    ///
    /// `None` is the *only* honest way to say "there is no more", and it is deliberately not
    /// inferable from [`Self::stop`]: a page can stop for a reason that still leaves a frontier
    /// (`Budget`, `Page`) and one that does not (`Exhausted`, `NoSuchRef`), and a frontend that
    /// re-derived the rule would get `CursorLost` wrong — which produces an infinite "load
    /// more" that returns the same zero rows for ever.
    pub resume: Option<LogResume>,
    /// Per-repository outcomes, in the scope's order.
    pub repos: Vec<RepoPage>,
    /// The page's overall stop: the *weakest* of [`Self::repos`]' stops, so `Exhausted` only
    /// when every repository is exhausted. A merged walk where one root is finished and another
    /// has forty thousand commits left is not finished.
    pub stop: LogStop,
    /// Commits examined across every repository for this page.
    pub scanned: u32,
    /// A newer query for the same view superseded this one mid-walk.
    ///
    /// **Not an error**, and it must not be shown as one: it is what typing in the filter box
    /// produces, several times per word. The rows present are correct as far as they go; the
    /// caller simply has a better answer coming and should not overwrite it with this one.
    pub cancelled: bool,
    /// Rename hops crossed inside this page, oldest last like the rows.
    pub renames: Vec<RenameHop>,
    /// `--follow` was actually applied: [`LogQuery::follow`] was set *and* a single path filter
    /// was given. Reported rather than assumed, so a flag that was silently ignored is visible
    /// in the response instead of being a mystery about missing rows.
    pub followed: bool,
    /// Rename following stopped at its own cap. From that point back, the history shown is the
    /// file's under its current name only — so the log is a true subsequence, but not the whole
    /// story, and the panel says so instead of implying the file was created there.
    pub follow_capped: bool,
}

// --- the graph --------------------------------------------------------------------------------

/// One line segment in a row's gutter.
///
/// The lane structure is computed in Rust and shipped as segments rather than being derived in
/// the renderer from `parents`. Two reasons. Deriving it needs the *whole* walk in hand —
/// lane assignment for row N depends on every row before it — so a virtualised list, which is
/// the only kind that can show a hundred thousand commits, cannot do it: it does not have the
/// rows it has scrolled past. And the resume token has to carry the open lanes across a page
/// boundary anyway ([`LaneResume`]), so the state exists here regardless; computing it twice,
/// in two languages, guarantees the two eventually disagree about where a line goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum GraphEdge {
    /// A branch that neither starts nor ends here: a vertical line straight through the row.
    Pass { lane: u16, color: u16 },
    /// An edge arriving from lane `top` in the row above into this row's commit dot. A merge
    /// row has one of these per parent lane that closes into it.
    Enter { top: u16, color: u16 },
    /// An edge leaving this row's commit dot down into lane `bottom` in the row below.
    Exit {
        bottom: u16,
        color: u16,
        /// The parent this edge goes to is **not in this page** — the walk was cut by a limit,
        /// a budget or a shallow floor. Drawn fading out rather than into a node that never
        /// arrives, which is what an ordinary `Exit` at the bottom of a page looks like and is
        /// how a graph acquires lines to nowhere.
        dangling: bool,
    },
    /// `count` commits were simplified away in this lane between this row and the next — the
    /// visual form of [`CommitRow::pruned`]. Drawn as a broken or ticked segment, so a filtered
    /// log does not pretend two commits a thousand apart are adjacent.
    Bundle { lane: u16, count: u16 },
}

/// The gutter for one row, parallel to [`CommitPage::commits`] index for index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GraphRow {
    /// The column this row's own commit dot sits in.
    pub lane: u16,
    /// Palette index for the dot. See [`OpenLaneWire::color`] for why this is an index.
    pub color: u16,
    pub edges: Vec<GraphEdge>,
    /// This row needed more concurrent lanes than [`LogQuery::graph_lanes`] allowed, and the
    /// ones past the cap are not in `edges`.
    ///
    /// Per row and not per page, deliberately: a repository with one chaotic merge week should
    /// not lose its graph for the other five years. The row marks itself, the renderer draws an
    /// ellipsis in the last column, and the rows around it are still exact.
    pub overflow: bool,
}

/// Why a page has no graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GraphOff {
    /// A path, author or text filter is on, so the rows are a *subsequence* of history. Lines
    /// drawn between them would assert parent relationships that do not exist — the row above
    /// is not the child of the row below, it is merely the next match. This is the case that
    /// makes a graph actively wrong rather than merely absent.
    Filtered,
    /// [`Simplify::Full`]. A full-history walk is a mesh; the lanes are technically computable
    /// and unreadable.
    FullHistory,
    /// [`LogScope::Merged`]. There is no parent relation between two repositories.
    Merged,
    /// [`LogQuery::graph`] was false.
    Disabled,
    /// The walk began from an explicit [`LogCursor::At`] or [`LogCursor::After`] with no
    /// [`LaneResume`] to continue, so the first rows' parents are off the top of the page and
    /// the columns cannot be joined to anything. Drawing them anyway produces a graph whose
    /// first screen is wrong and then silently becomes right.
    Rerooted,
}

/// A page's gutter, or the reason there isn't one.
///
/// A tagged union rather than an empty `Vec<GraphRow>`, for the reason `symbols.rs` gives about
/// [`crate::symbols::FileOutline`] and `diagnostics.rs` about
/// [`crate::diagnostics::DiagnosticsSnapshot`]: an empty array and *no graph is possible for
/// this query* are opposite claims, and `[]` means both. The panel has to be able to say
/// "graph unavailable: filtered" rather than rendering an empty gutter that looks like a bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum LogGraph {
    Rows {
        /// Exactly as long as [`CommitPage::commits`], index for index.
        rows: Vec<GraphRow>,
        /// The widest row's lane count. The gutter is sized once from this rather than per row,
        /// because a gutter that resizes as the list scrolls moves every summary sideways.
        lanes: u16,
        /// Any row in this page overflowed. A page-level mirror of [`GraphRow::overflow`], so
        /// the panel can show one notice instead of scanning every row to find out whether to.
        overflow: bool,
    },
    Off {
        reason: GraphOff,
    },
}

// --- one commit --------------------------------------------------------------------------------

/// How many lines one file's change added and removed, or why nobody knows.
///
/// A union and not a pair of `u32`s, because **0/0 is a real answer** — a mode change, a
/// rename with no edits, an empty file — and it is also exactly what a `u32` pair says when
/// nothing counted. Counting means diffing every file in the commit against its parent, which
/// for a four-thousand-file merge is seconds, so "nobody counted" is a state the wire has to be
/// able to express rather than a case to be avoided by always counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum LineCount {
    Counted {
        added: u32,
        deleted: u32,
    },
    /// libgit2 called a side binary. There are no lines to count, and `0/0` would read as an
    /// empty change to a file that may have gained a megabyte.
    Binary,
    /// Past the per-file size cap. `bytes` is the blob's size, so the row can say *why* rather
    /// than showing a dash.
    TooLarge {
        bytes: u64,
    },
    /// Nobody asked yet. The default for the first response, which deliberately omits counts so
    /// the file list paints immediately; [`CommitLineCounts`] fills them in afterwards.
    NotCounted,
    /// The counting pass hit its deadline before reaching this file. Distinct from
    /// [`Self::NotCounted`] because a retry is worth offering for one and pointless for the
    /// other.
    TimedOut,
}

/// Which side a commit's file list was computed against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DiffAgainst {
    /// Position in [`CommitRow::parents`], plus that parent's oid.
    ///
    /// Both, because the index is what the UI's "compare against parent 2" control toggles and
    /// the oid is what makes the answer checkable in a bug report. A merge defaults to index 0
    /// — the mainline — which is git's own default and the only one that makes a merge's diff
    /// small enough to read.
    Parent { index: u32, oid: String },
    /// A root commit. Every file is an addition, against git's empty tree.
    EmptyTree,
}

/// One file in a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitFile {
    pub path: String,
    /// The pre-rename path, when git found a rename.
    pub old_path: Option<String>,
    pub status: FileState,
    pub binary: bool,
    pub lines: LineCount,
}

/// The totals line under a commit's file list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitTotals {
    pub files: u32,
    pub added: u32,
    pub deleted: u32,
    /// At least one file's [`CommitFile::lines`] is not [`LineCount::Counted`], so `added` and
    /// `deleted` are a **lower bound**. The UI prefixes them with a `≥`; without this flag it
    /// would state a total that is quietly wrong, which is worse than stating none.
    pub partial: bool,
}

/// Everything the commit detail pane draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitDetail {
    /// The same row the log drew, so the detail pane and the list cannot disagree about a
    /// summary or a ref chip.
    pub commit: CommitRow,
    /// The **full** message, summary line included.
    ///
    /// Not the body with the summary stripped: a message whose body repeats the summary line —
    /// which every `git commit -m` of a one-line change produces when someone later adds a
    /// trailer — could not be reconstructed from a split, and the split has to be undone
    /// anywhere the message is copied or amended.
    pub message: String,
    pub committer: String,
    pub committer_email: String,
    pub against: DiffAgainst,
    pub files: Vec<CommitFile>,
    /// The file list was capped. Bounded in Rust for the reason
    /// [`crate::git::FetchOutcome::commits`] gives: a twelve-thousand-file merge must not put
    /// twelve thousand rows on the IPC wire in order to have the renderer drop them.
    pub files_truncated: bool,
    /// `parents.len() > 1`. Precomputed because three separate pieces of UI branch on it and
    /// re-deriving it in each is three chances to get an empty-parents root commit wrong.
    pub merge: bool,
    pub total: CommitTotals,
}

/// The second pass: the same file list with [`LineCount`]s filled in.
///
/// A separate response and a separate call, because the first one has to paint. A commit's file
/// list comes out of one tree diff and is fast; the line counts need a *content* diff of every
/// file in it, and on a large merge that is the difference between a pane that appears
/// instantly with numbers arriving a moment later and a pane that appears after four seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitLineCounts {
    /// The same paths in the same order as [`CommitDetail::files`], so the renderer patches by
    /// index rather than by matching path strings — which would go wrong for the two entries a
    /// rename produces.
    pub files: Vec<CommitFile>,
    pub total: CommitTotals,
    /// The pass ran out of time. Files past that point carry [`LineCount::TimedOut`], and the
    /// totals are marked [`CommitTotals::partial`].
    pub deadline_hit: bool,
}

// --- revisions ---------------------------------------------------------------------------------

/// One side of a revision comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum RevSide {
    Commit {
        oid: String,
    },
    /// *The first parent of the other side*, resolved at read time.
    ///
    /// A **relative** side, and that is the whole point of it. A revision diff tab is persisted
    /// in `workspace.json` (see [`crate::workspace::DiffOrigin::GitRevision`]), and "this commit
    /// against its parent" has to still mean that after a restart. Freezing the parent's oid
    /// into the saved tab instead would survive a restart and not survive a rebase: the frozen
    /// oid then names a commit that is no longer an ancestor of anything on the branch, and the
    /// tab reopens showing a diff against a commit the user cannot find in their own log.
    FirstParent,
    /// The file as it is on disk right now, uncommitted edits included. The only side in this
    /// module that is *not* frozen, which is why it is a variant rather than a magic oid — a
    /// caller that must refuse a moving side can match on it.
    WorkingTree,
}

/// One file's hunks between two revisions.
///
/// The read-only twin of [`crate::git::FileDiff`], and deliberately not that type. It has no
/// `rev` and no `partial_ok`, because there is nothing to stage from it and therefore no
/// staleness to check and no refusal to explain — reusing `FileDiff` here would put two fields
/// on the wire that are meaningless for every value of them, and the first caller to trust
/// `partial_ok` on a commit diff would be offering a *Stage hunk* button on frozen history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RevisionDiff {
    pub path: String,
    /// The pre-rename path, when the pair differs by a rename.
    pub old_path: Option<String>,
    /// The requested sides, echoed back. What the tab's header shows.
    pub new: RevSide,
    pub old: RevSide,
    /// The commit oids those sides **resolved to**, so a [`RevSide::FirstParent`] or
    /// [`RevSide::WorkingTree`] answer can still be quoted in a bug report or pinned by a
    /// follow-up call. `None` for a side with no commit — the working tree, or a file that does
    /// not exist on that side.
    pub new_oid: Option<String>,
    pub old_oid: Option<String>,
    pub status: FileState,
    pub binary: bool,
    /// Unix mode bits, e.g. 33188 (0o100644). Zero when the side does not exist — the same
    /// convention [`crate::git::FileDiff`] uses, so a reader who knows one knows both.
    pub old_mode: u32,
    pub new_mode: u32,
    pub hunks: Vec<DiffHunkView>,
}

/// One path's summary inside a [`RevisionRange`].
///
/// Counts rather than hunks: the range view is a *list* of files and expanding one is a second
/// call. Shipping every hunk of a hundred-file range to draw a hundred rows of `+12 −3` is the
/// same mistake [`CommitLineCounts`] exists to avoid, one level up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RevisionChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileState,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
}

/// Every file that differs between two revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RevisionRange {
    pub new: RevSide,
    pub old: RevSide,
    /// What the two sides resolved to. See [`RevisionDiff::new_oid`].
    pub new_oid: Option<String>,
    pub old_oid: Option<String>,
    /// Summary lines of the two endpoints, for the header. Empty for
    /// [`RevSide::WorkingTree`], which has no commit and therefore no message — and an empty
    /// string rather than an `Option` because the header renders it either way and a `None`
    /// would only be turned back into one.
    pub new_summary: String,
    pub old_summary: String,
    pub files: Vec<RevisionChange>,
    /// The file list was capped. Same rule as [`CommitDetail::files_truncated`].
    pub truncated: bool,
}

/// One file's contents at one revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RevisionBlob {
    /// The blob decoded lossily as UTF-8. Lossy rather than refusing, because a source file
    /// with one Latin-1 byte in a comment is still a file a person wants to read, and the
    /// replacement character says where the problem is.
    pub text: String,
    /// The blob's own oid — not the commit's. What a `git cat-file` would take, and what makes
    /// two revisions of an unchanged file identifiable as the same bytes.
    pub oid: String,
    /// libgit2 called it binary. `text` is empty in that case rather than a screenful of
    /// replacement characters.
    pub binary: bool,
    /// The blob's full size, even when [`Self::truncated`] cut `text` short. The reader needs
    /// to know how much it is not seeing.
    pub bytes: u32,
    pub truncated: bool,
}

/// What a revspec typed by a user resolved to.
///
/// Resolved in Rust and echoed back in full, because the string the user typed (`HEAD~3`,
/// `main@{yesterday}`) means something different tomorrow, and a tab titled with it would
/// quietly change what it shows. The tab keeps the oids; the label keeps the phrasing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolvedRev {
    /// Exactly what was typed.
    pub spec: String,
    /// The single revision, or the *older* end of a range — the left-hand side of `a..b`.
    pub from: String,
    /// The newer end of a range. `None` when [`Self::range`] is false.
    pub to: Option<String>,
    /// The spec was a range (`a..b` or `a...b`) rather than a single revision. Not inferable
    /// from `to` being `Some` by any rule worth writing twice, and the two produce different
    /// views.
    pub range: bool,
    /// The short form a tab title uses: `a1b2c3d ‥ e4f5g6h`, or the branch name when the spec
    /// named one. Built in Rust so every surface abbreviates the same way.
    pub label: String,
}

// --- blame -----------------------------------------------------------------------------------

/// Which version of a file was blamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BlameSource {
    /// `HEAD`'s version. The cheapest, and correct only for a clean file.
    Head,
    /// The file on disk, with uncommitted changes attributed to nobody.
    Working,
    /// The **editor buffer**, including unsaved edits.
    ///
    /// Its own state and not merged into `Working`, because the two disagree exactly when the
    /// user is typing — which is when the gutter is on screen. A gutter computed against the
    /// saved file drifts a line at a time as the buffer grows, and every annotation below the
    /// caret points at the wrong commit.
    Buffer,
    /// A historical revision, as reached from the log or from a [`BlameParent`] hop.
    Revision,
}

/// How hard blame works to trace a line back past a move.
///
/// Ordered by cost, and the cost is not linear: `CopiesAnywhere` is git's `-C -C`, which
/// searches every file in every parent commit and can take minutes on a large repository. The
/// enum exists so the expensive modes are a deliberate choice rather than something a default
/// hands to everyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BlameFollow {
    /// Follow the file across renames only — git's default. The default here too.
    #[default]
    Renames,
    /// Also detect lines moved within the same file — `-M`.
    MovedLines,
    /// Also detect lines copied from other files **modified in the same commit** — `-C`.
    CopiesInCommit,
    /// Also detect lines copied from any file in the parent — `-C -C`. The expensive one.
    CopiesAnywhere,
}

/// Options for one blame.
///
/// Container-level `#[serde(default)]` so a caller that wants the ordinary answer sends `{}`,
/// and so adding an option here does not break every existing call site — the same reason
/// [`crate::settings::ClaudeCli`] carries one. The derived `Default` is safe here, unlike
/// `ClaudeInjection`'s: every field's zero value is the behaviour that was there before the
/// field existed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
#[ts(export)]
pub struct BlameRequest {
    pub follow: BlameFollow,
    /// git's `-w`. Reattributes a line whose only change was reindentation, which is the single
    /// most common reason a blame gutter is useless — one reformatting commit otherwise owns
    /// every line of the file.
    pub ignore_whitespace: bool,
    /// Walk first parents only. On a merge-heavy branch this attributes a line to the merge
    /// that brought it to *this* branch rather than to the commit on the side branch, which is
    /// the reading someone asking "when did this land here" wants.
    pub first_parent: bool,
    /// Stop the walk at this revision — git's `--reverse` boundary in the useful direction:
    /// blame the file as of an older commit rather than as of `HEAD`. `None` blames the current
    /// source.
    pub newest: Option<String>,
}

/// A consecutive span of lines attributed to one commit.
///
/// **Run-length encoded, and this is the load-bearing decision of the blame wire format.** A
/// twenty-thousand-line file blames to a few hundred distinct commits and typically a few
/// thousand runs; one object per line would put twenty thousand records on the IPC wire — each
/// with an oid, a name, an address and a timestamp if denormalised, or an index if not — to
/// paint a gutter sixty lines high. The runs are what make blame openable on a large file at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlameRun {
    /// First line of the run, **1-based**, matching every other line number on this wire — see
    /// the units note in `symbols.rs`.
    pub start: u32,
    /// How many lines. Never zero.
    pub lines: u32,
    /// Index into [`BlameFile::commits`], or `None` for lines that are not committed —
    /// uncommitted local edits under [`BlameSource::Working`] or [`BlameSource::Buffer`].
    ///
    /// An index and not an inline commit: a run per commit-change means the same commit appears
    /// in dozens of runs, and denormalising its author and message into each is most of the
    /// payload for none of the information.
    pub commit: Option<u32>,
}

/// One commit referenced by a blame, denormalised just enough for the gutter and its tooltip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlameCommit {
    pub oid: String,
    pub short_oid: String,
    pub summary: String,
    pub author: String,
    pub author_email: String,
    /// Author time, unix seconds. Author and not committer, because the gutter's question is
    /// "when was this line written".
    pub authored: i64,
    /// The path this file had **at this commit**, when a rename has been followed since. `None`
    /// when it is the same as [`BlameFile::path`]. Needed for the "open this file as it was
    /// then" action, which otherwise asks for a path that did not exist yet.
    pub orig_path: Option<String>,
    /// git's boundary commit: the walk's limit, not necessarily where the line was written.
    ///
    /// Marked because a boundary attribution is an artefact of how far the blame was allowed to
    /// go — a shallow clone, or [`BlameRequest::newest`] — and presenting it as authorship
    /// tells the user a specific person wrote a line they may never have seen.
    pub boundary: bool,
}

/// A whole file's blame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlameFile {
    /// Repo-relative path of the file as blamed.
    pub path: String,
    /// The commit the blame was computed against, full oid. Every run's attribution is relative
    /// to this, so a gutter cached in the frontend can tell whether it is still current.
    pub head: String,
    /// Total lines covered. The runs are contiguous and cover exactly `1..=lines`, which is the
    /// invariant that lets the renderer index by line without a search.
    pub lines: u32,
    pub runs: Vec<BlameRun>,
    /// Every commit the runs index into, in first-appearance order.
    pub commits: Vec<BlameCommit>,
    pub source: BlameSource,
    /// The blamed source differs from [`Self::head`] — there are uncommitted changes, so some
    /// runs carry `commit: None`.
    pub dirty: bool,
    /// The follow mode actually used, which may be weaker than the one requested.
    pub follow: BlameFollow,
    /// Set when [`Self::follow`] is weaker than what was asked for, saying why in one sentence
    /// for the gutter's tooltip.
    ///
    /// An honest downgrade rather than a silent one: a `-C -C` that was abandoned after ten
    /// seconds and quietly answered as a plain rename-follow gives the user a gutter that
    /// *looks* like the expensive one and is not, and they have no way to tell.
    pub downgraded: Option<String>,
}

/// The "blame the parent of this commit" hop, from the gutter's context menu.
///
/// The single most useful gesture in a blame gutter: the commit shown for a line is very often
/// a reformat or a rename, and the real change is one hop further back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlameParent {
    /// Full oid of the parent to blame at.
    pub rev: String,
    pub short_rev: String,
    /// The path **at that revision**. Carried because the parent may spell it differently — the
    /// hop is most often taken across exactly the rename that makes the two differ, so
    /// re-using the current path is wrong precisely when the gesture is most wanted.
    pub path: String,
    /// The parent's summary line, so the menu item can name where it is about to go.
    pub summary: String,
}

// --- the commit actions --------------------------------------------------------------------

/// Applying an existing commit's patch somewhere else.
///
/// One enum for both directions, because everything about them is the same — resolve a commit,
/// pick a mainline for a merge, apply, refuse on conflict — except the sign of the patch. Two
/// enums would mean two of every error variant below, and the frontend catching one of them and
/// not the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReplayOp {
    /// Apply the commit's patch **inverted** onto `HEAD`.
    Revert,
    /// Apply the commit's patch as it stands onto `HEAD`.
    CherryPick,
}

/// What a replay leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReplayMode {
    /// Commit immediately, with a generated message.
    Commit,
    /// Leave the change in the working tree for the user to review, edit and commit
    /// themselves — IDEA's behaviour, and the safer default for a revert, which is very often
    /// the first half of a larger fix.
    WorkingTree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ReplayRequest {
    /// The commit to replay, as a full oid. Not a revspec: this comes from a row the user
    /// clicked, so the ambiguity a revspec can carry has nothing to resolve and everything to
    /// go wrong.
    pub commit: String,
    pub mode: ReplayMode,
    /// Which parent is the mainline, **1-based**, as git's `-m` numbers them.
    ///
    /// Required for a merge and refused for anything else — [`crate::git::GitError::
    /// MergeNeedsMainline`] and [`crate::git::GitError::NotAMerge`]. Deliberately not defaulted
    /// to 1: "revert this merge" without saying which side to keep is a question with no
    /// correct default, and picking one silently reverts an entire feature branch or none of
    /// it, with no way for the user to tell which they got until they read the diff.
    pub mainline: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayOutcome {
    pub op: ReplayOp,
    /// The commit that was replayed, full oid.
    pub source: String,
    /// The commit that was created, full oid. **Empty** under [`ReplayMode::WorkingTree`],
    /// which creates none — empty rather than an `Option` so it reads the same way as
    /// [`crate::git::FetchOutcome`]'s vacant fields, which use the same convention for the same
    /// reason.
    pub created: String,
    /// The summary line of the new commit, or the one that would be used.
    pub summary: String,
    pub files: u32,
}

/// How far back a reset moves the index and the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ResetKind {
    /// Move `HEAD` only. Index and working tree untouched, so the dropped commits' changes are
    /// left staged. Loses nothing.
    Soft,
    /// Move `HEAD` and the index. The changes stay in the working tree, unstaged. Loses nothing
    /// that was committed.
    Mixed,
    /// Move `HEAD`, the index **and** the working tree. **This destroys uncommitted work**, and
    /// it is the reason [`ResetPreview`] exists and why [`ResetRequest::shelve_first`] is
    /// offered directly beside it.
    Hard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ResetRequest {
    /// Where to move `HEAD` to — a full oid from a log row.
    pub target: String,
    pub kind: ResetKind,
    /// Shelve the working tree first, under this name.
    ///
    /// `None` means do not, which for [`ResetKind::Hard`] is the path that destroys work — so
    /// the field is `Option<String>` rather than a `bool` plus a name, making "yes, and here is
    /// what to call it" a single unambiguous value and leaving no way to ask for a shelf
    /// without naming it.
    pub shelve_first: Option<String>,
    /// Proceed despite a dirty tree, or despite a file the shelf could not capture.
    ///
    /// Never defaulted to `true` anywhere — the same rule [`crate::git::CommitRequest::force`]
    /// states. This is the second click on a dialog that has already listed what is at stake.
    pub force: bool,
}

/// Everything the reset confirmation needs, computed in Rust before anything is written.
///
/// A preview and not a set of numbers the dialog assembles itself, because every one of these
/// answers takes a walk or a status call, and a dialog that made five IPC calls to populate
/// itself would paint five times and could show a state that never existed as a whole. It is
/// also the only place a `--hard` is explained *before* it happens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResetPreview {
    /// The branch that would move, or the short oid when detached.
    pub head: String,
    pub detached: bool,
    pub head_oid: String,
    pub target_oid: String,
    pub target_summary: String,
    /// Commits reachable from `HEAD` but not from the target — what the reset drops.
    pub commits_dropped: u32,
    /// Commits reachable from the target but not from `HEAD`. Non-zero when the target is not
    /// an ancestor, which is a *forward* move or a sideways one and is not what most people
    /// mean by "reset" — so the dialog has to be able to say it.
    pub commits_gained: u32,
    /// The first few dropped commits, for the dialog. Capped in Rust, like
    /// [`crate::git::FetchOutcome::commits`], so a reset over four hundred commits does not put
    /// four hundred rows on the wire to show ten.
    pub dropped: Vec<PulledCommit>,
    /// How many more there were beyond [`Self::dropped`].
    pub more_dropped: u32,
    /// Paths currently staged, and paths dirty in the working tree. Both, because
    /// [`ResetKind::Mixed`] loses the first list's staging and [`ResetKind::Hard`] loses the
    /// second list's contents, and the dialog names exactly what the chosen kind costs.
    pub staged: Vec<String>,
    pub dirty: Vec<String>,
    /// How many untracked files a `--hard` would **keep**.
    ///
    /// Stated positively and on purpose: `git reset --hard` does not delete untracked files, and
    /// the most common fear at this dialog is that new files are about to vanish. Saying "3
    /// untracked files are not affected" answers it; saying nothing leaves the user to guess,
    /// and the guess is wrong.
    pub untracked_kept: u32,
    /// What [`ResetRequest::shelve_first`] would capture, ready to hand to the shelf unchanged.
    ///
    /// The selections and not just the paths, because the shelf takes
    /// [`crate::git::PathSelection`]s and rebuilding them at the call site would be a second
    /// place that decides what "everything dirty" means — which is exactly how a shelf ends up
    /// capturing a different set from the one the dialog listed.
    pub shelvable: Vec<PathSelection>,
    /// Mirrors [`crate::git::RepoChanges::use_staging_area`], because a `Mixed` reset means
    /// something different in the two modes — in staging-area mode it unstages, in changelist
    /// mode the index is rebuilt from changelists anyway — and the dialog's wording has to
    /// follow.
    pub use_staging_area: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResetOutcome {
    pub kind: ResetKind,
    pub head_before: String,
    pub head_after: String,
    pub commits_dropped: u32,
    /// How many files a `--hard` overwrote. Zero for the other two kinds, by construction.
    pub files_discarded: u32,
    /// The shelf entry that was made, when [`ResetRequest::shelve_first`] asked for one — the
    /// whole entry and not just its id, so the toast can offer *Unshelve* without a second
    /// call. Mirrors [`crate::git::CheckoutOutcome::stashed`].
    pub shelved: Option<ShelfEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct TagRequest {
    pub name: String,
    /// The commit to tag, full oid.
    pub target: String,
    /// `Some` makes an **annotated** tag (a real object, with a tagger and a date); `None` a
    /// lightweight one (a ref, and nothing else). The distinction is not cosmetic — `git
    /// describe` and most release tooling ignore lightweight tags — so it is expressed by which
    /// value is present rather than by a separate `annotated: bool` that could disagree with
    /// the message.
    pub message: Option<String>,
    /// Move a tag that already exists. Refused without it —
    /// [`crate::git::GitError::TagExists`] — because a tag is a published promise about which
    /// commit a release is, and moving one silently is how two people end up building different
    /// `v1.2.0`s.
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TagOutcome {
    pub name: String,
    /// The commit the tag now points at.
    pub oid: String,
    pub annotated: bool,
    /// An existing tag was force-updated rather than created. The toast says so, because "moved
    /// v1.2.0" and "created v1.2.0" are things the user needs to be able to tell apart after
    /// the fact.
    pub moved: bool,
}

/// Checking out a commit from the log, which necessarily detaches `HEAD`.
///
/// Its own outcome and not [`crate::git::CheckoutOutcome`], which is about branches: that type's
/// `branch` and `created_from_remote` are meaningless here, and this one's `previous` — the
/// thing to offer as a way back — has nowhere to live there. The overlapping fields keep the
/// same names and the same meanings on purpose, so the two toasts read alike.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DetachOutcome {
    /// Short oid of the commit now checked out.
    pub head: String,
    /// Its summary line, so the toast can say what you are standing on rather than only where.
    pub summary: String,
    /// The branch that was checked out before, or a short oid if `HEAD` was already detached.
    ///
    /// The single most important field here: a detached `HEAD` is the state users most often
    /// reach by accident and least often know how to leave, and this is what lets the toast
    /// offer *Back to main* instead of leaving them to work it out.
    pub previous: String,
    /// A stash was made to get the working tree out of the way. Same meaning as
    /// [`crate::git::CheckoutOutcome::stashed`].
    pub stashed: Option<String>,
    /// The stash was made, the checkout succeeded, and the changes could **not** be put back.
    /// They are still in `git stash list`; this is git's own message for why.
    pub restore_failed: Option<String>,
}
