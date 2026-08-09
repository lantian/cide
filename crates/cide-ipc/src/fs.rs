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
    pub root: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TreeRowKind {
    Dir,
    File,
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
    /// A watched git file changed — `HEAD`, `index`, or a ref. The branch readout and the
    /// git panel refresh on this; nothing else needs to.
    pub git: bool,
}
