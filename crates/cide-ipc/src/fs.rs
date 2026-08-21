//! Wire types for the file tree, the watcher and file operations. (M8)
//!
//! # Why the tree is a window of rows and not a tree
//!
//! A project root with 100k files is ordinary, and the frontend renders a virtualized list
//! of at most a few dozen visible rows. Shipping a nested structure would mean serialising
//! the whole thing to paint 30 rows, so the wire model is the *flattened* form the list
//! already wants: `fs.treeCount` for the scrollbar and `fs.treeRows(offset, len)` for the
//! slice that is actually on screen. Expansion state lives in Rust because it is what
//! decides that flattening — a frontend that owned it would have to send it back on every
//! window request.
//!
//! Paths travel as `PathBuf`, which serde renders as a plain string. Entries whose names are
//! not valid UTF-8 are dropped during the walk rather than lossily converted: a row's path is
//! the identifier the frontend hands back to `fs.expand` and `fs.reveal`, and a path that has
//! been through `to_string_lossy` no longer names the file it came from.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One row of the flattened, windowed file tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TreeRow {
    #[ts(type = "string")]
    pub path: PathBuf,
    /// The last component, which is all the row draws.
    pub name: String,
    /// Indent level. 0 is a top-level row: a project root when the project has several, the
    /// root's own children when it has one.
    pub depth: u16,
    pub kind: TreeRowKind,
    /// Only meaningful for directories.
    pub expanded: bool,
    /// Whether to draw a twisty. A directory that walked to nothing (empty, or entirely
    /// ignored) gets none, so clicking it cannot appear to do nothing.
    pub has_children: bool,
    /// A symlink is shown as a link rather than followed: the walk does not descend through
    /// one, so its subtree is not in the index and must not be drawn as expandable.
    pub symlink: bool,
    /// Index into `Project::roots`. The tree colours and groups by root in a multi-root
    /// project, and needs this without re-deriving it from path prefixes on every row.
    ///
    /// [`NO_ROOT`] for a row that belongs to no root at all — every row of a synthetic group.
    pub root: u16,
    /// Drawn dim after the name: a dependency's version, a group's count, `not downloaded`.
    ///
    /// `None` for every row that came out of the walk, which is every row the file tree drew
    /// before M13. A second *field* rather than a suffix on `name` because `name` is what the
    /// icon lookup, the rename box and the sibling-name check all read — folding `serde 1.0.229`
    /// into it would mean an icon chosen from a version number.
    pub detail: Option<String>,
}

/// One row of the tree that a speed-search query matched, and where in its name.
///
/// `row` is an index into the *composed* flattening — the same address `fs.treeRows` serves and
/// the same one the frontend's cursor moves to — so the seam between the walked index and the
/// synthetic groups is closed in Rust and never reaches the webview.
///
/// `start`/`end` are offsets into [`TreeRow::name`] in **UTF-16 code units**, which is the unit
/// `String.prototype.slice` indexes in. See `cide_fs::speed` for why the conversion happens here
/// rather than in the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TreeMatch {
    pub row: u32,
    pub start: u32,
    pub end: u32,
}

/// Everything one speed-search keystroke gets back.
///
/// `query` and `count` are both here so a frame that describes a *different* question can be
/// discarded rather than drawn, which is the sharpest hazard in this feature: match indices
/// describe one flattening, and a watcher burst or an expand renumbers every one of them. The
/// frontend keeps a frame only while `query` is still what is typed and `count` is still the
/// tree's row count; `PickerFrame` echoes its query back for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TreeMatches {
    /// The query these matches answer, echoed back. A reply that lands after a newer keystroke
    /// is discardable without a request id.
    pub query: String,
    /// How many rows were searched — the tree's row count at the moment of the walk.
    pub count: u32,
    /// In walk order, which is tree order. Deliberately not ranked: see `cide_fs::speed`.
    pub matches: Vec<TreeMatch>,
    /// The limit cut the list short and there was more. Said out loud in the overlay, because a
    /// silently short answer is indistinguishable from a tree that does not contain the file.
    pub truncated: bool,
}

/// [`TreeRow::root`] for a row no project root contains.
///
/// A reserved value rather than an `Option<u16>`: the field is on every row of a 100k-row tree,
/// `Option` would widen the wire form of all of them for a handful of synthetic rows, and every
/// consumer already treats it as an index it must bounds-check. `u16::MAX` cannot collide — a
/// project with 65,535 roots is not a thing, and `cide_fs::Index` indexes `roots` by this value.
pub const NO_ROOT: u16 = u16::MAX;

/// What a tree row *is*, which decides every gesture it answers.
///
/// The two synthetic variants are the M13 addition, and they are on this enum rather than being
/// a flag beside it because "is this a directory" was already the question every call site
/// asked. A boolean `synthetic: bool` would have left `kind == 'dir'` true for a group header
/// and every one of those call sites silently wrong; widening the enum makes the compiler and
/// `check-groups.mjs` name each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TreeRowKind {
    Dir,
    File,
    /// A synthetic group header — *External Libraries*. Expands and collapses like a directory
    /// and is nothing else: it has no path on disk, cannot be renamed, copied, deleted or
    /// revealed in a file manager, and never opens a tab.
    ///
    /// Its `path` is a `cide://group/<id>` sentinel, which is not absolute and is therefore
    /// refused by `cide_fs::ops::check_within` — so a frontend bug that sent it to a file
    /// operation gets an error rather than an action.
    Group,
    /// A sentence, drawn where rows would be. *Resolving dependencies…*, *`cargo` is not on
    /// PATH…*, *No external dependencies.*
    ///
    /// It exists so that a group can never be **silently** empty: an empty group and a broken
    /// one look identical, and the one thing a user cannot debug is a twisty that opens onto
    /// nothing. Not selectable, not openable, carries no menu.
    Note,
    /// A **pinned** top-level row — *Project Notes*. It **opens** rather than expanding, which
    /// is the one thing a [`TreeRowKind::Group`] cannot do.
    ///
    /// Like a header it is synthetic: it has no children, is never expanded, and its `path` is
    /// the same `cide://group/<id>` sentinel, so `cide_fs::ops::check_within` refuses it and a
    /// frontend bug that sent it to `fs_rename` or to `file_read` gets an error rather than an
    /// action. Unlike a header it is not a drawer with contents — the file it stands for is
    /// named by Rust, created by the command the row's click routes to, and opened as an
    /// **ordinary** `TabKind::File` tab so that save, the dirty marker, undo, find-in-file and
    /// the markdown grammar all work with no special case anywhere.
    ///
    /// Why a kind rather than reusing `File` with the sentinel path: `rowVerbs('file')` is
    /// `addressable`, so *Copy Path* would be live on the row and would put the literal string
    /// `cide://group/projectNotes` on the clipboard — the correct-looking answer about the
    /// wrong thing. A pin is openable and nothing else.
    Pin,
}

impl TreeRowKind {
    /// Whether this row stands for something on disk that file operations may touch.
    ///
    /// The single place the "which kinds are real" question is answered in Rust; the frontend's
    /// mirror is `ui/src/sidebar/groupRows.ts`. Every command in `cmd::fs` that takes a path
    /// still re-checks containment — this is about *rows*, not about trust.
    pub fn is_path(self) -> bool {
        matches!(self, Self::Dir | Self::File)
    }
}

/// What a project's index is doing, and how it is watching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FsStatus {
    /// True between `fs.index` starting and the walk finishing. The picker is usable the
    /// whole time — this only tells the UI whether its counts are still climbing.
    pub indexing: bool,
    pub files: u32,
    pub dirs: u32,
    /// `treeCount` at the moment the status was taken, so a caller that is only sizing a
    /// scrollbar does not need a second round trip.
    pub rows: u32,
    pub watch: WatchStatus,
}

/// How the watcher is watching, and why.
///
/// `Polling` is a degraded mode with a visible cost — a change can take a whole poll
/// interval to be noticed — so the reason is carried to the UI rather than logged. A user
/// whose inotify limit is exhausted can raise `fs.inotify.max_user_watches`; a user who is
/// told nothing just thinks the file tree is broken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchStatus {
    pub backend: WatchBackend,
    /// Set only when `backend` is `polling`. A sentence, meant for a banner.
    pub reason: Option<String>,
    /// Directories under an explicit watch. Zero in polling mode, which watches roots
    /// recursively instead.
    pub watched_dirs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WatchBackend {
    /// inotify on Linux — the normal case.
    Native,
    /// Stat-polling fallback. See [`WatchStatus::reason`].
    Polling,
}

/// One coalesced burst of filesystem change.
///
/// Bursts, not events: a `cargo build` produces tens of thousands of inotify events in a
/// second and the file tree needs to hear about it once. The watcher holds paths until the
/// tree has been quiet for its debounce window, so `touch`ing 5000 files is one of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FsChange {
    /// Paths that changed, already filtered through the walk's ignore matchers.
    #[ts(type = "Array<string>")]
    pub paths: Vec<PathBuf>,
    /// The burst was larger than the watcher will hold, so `paths` is a prefix rather than
    /// the whole story and the receiver should re-read what it cares about. Reporting the
    /// truncation is the point: silently dropping the tail is how a stale tree happens.
    pub truncated: bool,
    /// A watched git file changed — `HEAD`, `index`, or a ref.
    ///
    /// The branch readout gates on this: a label only moves when `HEAD` or a ref does, and
    /// without the gate every file save would cost a branch walk of every repository.
    ///
    /// The **git panel does not**, and the asymmetry is the point rather than an oversight. A
    /// plain working-tree write is what turns a file from clean to modified and it raises no
    /// flag here, so the panel refreshes on any burst for its project and lets one coalesced
    /// `git status` decide what actually moved. `gitlog`'s `gitRefsMoved` is a third answer
    /// again — it ignores index-only bursts, because a history walk only cares about refs.
    ///
    /// This comment claimed both consumers from the day the flag landed and had neither until
    /// M17: the log and the blame gutter were the first, and the two named here came later.
    pub git: bool,
}

// --- Copy / cut / paste of the files themselves -----------------------------------------

/// Which gesture put the paths on the tree's clipboard.
///
/// Two variants and not three: there is no "duplicate". A duplicate is a [`PasteMode::Copy`]
/// whose destination happens to be the source's own parent, and the collision rule already
/// answers it with `main copy.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PasteMode {
    /// The source stays where it is.
    Copy,
    /// The source is removed **on paste**, never before — a cut that is never pasted must
    /// cost nothing, which is the whole reason the mode travels with the paste rather than
    /// being acted on when the user pressed Ctrl+X.
    Cut,
}

/// What one pasted path became.
///
/// One of these per *source*, not per file on disk: pasting a directory of 4000 files is one
/// entry describing the directory. The counts inside it are what the panel says afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PastedEntry {
    #[ts(type = "string")]
    pub source: PathBuf,
    /// Where it landed.
    ///
    /// A path that was already occupied only when `replaced` is non-zero — that is, only when
    /// the caller sent a `Replace` decision for this source. Otherwise the name moved out of
    /// the way instead; see `renamed`.
    #[ts(type = "string")]
    pub dest: PathBuf,
    /// The destination took a *different* name because the one it wanted was taken.
    ///
    /// Reported rather than left for the user to notice, because the other reading of a paste
    /// that quietly produced `main copy.rs` is that it did nothing: the row the user was
    /// looking at is still there, still holding the older file.
    pub renamed: bool,
    /// Entries inside a copied directory that are neither a file, a directory nor a symlink —
    /// fifos, sockets, devices — and were left behind.
    ///
    /// Copying them is either meaningless (a socket) or a hang (reading a fifo waits for a
    /// writer that never comes), and refusing a whole paste over one stray `.sock` in a project
    /// would be the worse answer. Counted so the panel can say so.
    pub skipped: u32,
    /// Existing files this entry overwrote, because the user chose [`PasteChoice::Replace`].
    ///
    /// Zero for every paste nobody explicitly answered *Replace* to, which is the invariant
    /// worth being able to read off the result: a non-zero count here can only come from a
    /// decision that was sent with the command.
    ///
    /// More than one when a folder was merged: the answer was given about `src`, and the files
    /// it overwrote were inside it. That is the number the panel says afterwards, because
    /// "replaced src" describes something the user cannot check by looking.
    pub replaced: u32,
}

/// What to do about one name a paste would land on that is already taken.
///
/// **There is no `Cancel` here on purpose.** Cancel is not a third thing the disk does; it is
/// the command never being sent. Every question is answered *before* `fs_paste` is called, so a
/// user who backs out halfway has nothing to undo — see [`PasteCollision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PasteChoice {
    /// Overwrite what is there.
    ///
    /// **For two directories this is a *merge*, not a swap.** The pasted folder's contents are
    /// laid into the existing one: files that exist in both are overwritten, files that exist
    /// only in the destination are left alone. The alternative — delete the destination folder
    /// and put the source in its place — is the one that destroys files the user never saw and
    /// was never shown, so it is not what this word means here.
    Replace,
    /// The rename that has always shipped: `main.rs` beside a `main.rs` becomes `main copy.rs`.
    KeepBoth,
}

/// One answer, bound to the source it answers for.
///
/// A list of pairs rather than a map keyed by path: JSON object keys would make a path with a
/// `.` or a `/` in it a key nobody can round-trip safely, and the order the sources were given
/// in is the order the questions were asked in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PasteDecision {
    /// Exactly one of the paths in the paste's `sources`, matched by equality.
    #[ts(type = "string")]
    pub source: PathBuf,
    pub choice: PasteChoice,
}

/// A name a paste is about to land on that something already occupies.
///
/// Answered by `fs_paste_plan` **before** anything is written, which is the whole shape of this
/// feature: the dialog collects every answer, and only then is `fs_paste` called. A user who
/// cancels at the fourth of seven questions has had nothing written on their behalf, so there
/// is no partial paste to describe and no half-finished folder to clean up.
///
/// The counts are what make *Replace* an informed answer rather than a dare. A file collision
/// costs one file; a folder merge can cost hundreds, and the number is the only way to know
/// that before choosing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PasteCollision {
    #[ts(type = "string")]
    pub source: PathBuf,
    /// The path that is already taken: `dest_dir` joined with the source's own name.
    #[ts(type = "string")]
    pub dest: PathBuf,
    /// The last component, which is what the dialog says.
    pub name: String,
    /// Both sides are directories, so *Replace* means merge. See [`PasteChoice::Replace`].
    pub merge: bool,
    /// *Replace* is refused for this collision, and this sentence says why.
    ///
    /// Set when one side is a directory and the other is not — at the top, or anywhere inside a
    /// folder being merged. Swapping a file for a folder is not a replacement of anything, and
    /// guessing which one the user meant to keep is not a guess worth making. The dialog offers
    /// only *Keep both* and *Cancel* when this is set, and `fs_paste` refuses a `Replace`
    /// decision for it even if one is sent.
    pub blocked: Option<String>,
    /// How many existing **files** *Replace* would overwrite. 1 for a file-on-file collision.
    pub replaces: u32,
    /// Entries in the destination folder a merge would leave completely alone.
    ///
    /// A folder counts as one entry and is not descended into: this number exists to say "your
    /// files are still there", and inflating it by counting a `node_modules` recursively would
    /// make it read as noise.
    pub keeps: u32,
    /// The first few relative paths `replaces` counted, so the dialog can name names.
    pub sample: Vec<String>,
    /// The preview walk hit its budget: `replaces` and `keeps` are lower bounds.
    ///
    /// Reported rather than swallowed. "at least 4096 files" is an answer; a wrong exact number
    /// in a dialog about data loss is not.
    pub truncated: bool,
}
