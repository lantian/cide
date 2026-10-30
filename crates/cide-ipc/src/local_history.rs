//! Local History: the versions cide keeps of a project's files, independently of git.
//!
//! Named in full everywhere. [`crate::history`] is the **git** log and has been since M18, and a
//! second module called `history` would have every grep for one land in the other.
//!
//! These two enums are the disk format as well as the wire format, for the reason `cide-tasks`
//! gives for its DTOs: `cide-local-history` writes them into its log, and a second copy over
//! there is two definitions that disagree the first time one of them is edited. Names are
//! therefore permanent. Add a variant, never rename one.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What one row of a file's history says happened to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VersionKind {
    /// Seen by a walk, with no way to know since when.
    ///
    /// **Distinct from [`Self::Created`], and a folder restore depends on it.** The first scan of
    /// a project sees every file for the first time, and that says nothing about when any of
    /// them appeared. Calling them `Created` would make "restore this folder to last Tuesday"
    /// trash a file that had been there for years, because its history would claim it did not
    /// exist yet. A folder restore deletes on `Created` and only on `Created`.
    Baseline,
    /// A root's first full scan reached the disk. Before this row, a file with no history could
    /// be new or could merely be unscanned, so only after it can a watcher event mean `Created`.
    BaselineDone,
    /// Appeared while being watched, so it really did not exist before this row.
    Created,
    /// The content changed.
    Modified,
    /// The path no longer exists.
    Deleted,
    /// A name the user or cide gave a point in time. Holds no content of its own.
    Label,
}

/// How cide came to know about a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VersionSource {
    /// Written by a save in cide.
    Saved,
    /// Reported by the watcher: an agent, a terminal, another program. `git` lands here too, and
    /// there is deliberately no git variant: `FsChange::git` says a watched git file moved in the
    /// same burst, which is not the same as git having written these files.
    External,
    /// Found by comparing the disk with the store, after the fact: changes made while cide was
    /// closed, while capture was off, or in a burst too large for the watcher to itemise.
    Detected,
    /// Written by a Local History restore.
    Restored,
    /// A rename or move made through cide's own file operations.
    FileOp,
}

/// One row, as a window sees it.
///
/// Numbers go as JSON numbers: these are row ids and millisecond timestamps, both far inside
/// `Number.MAX_SAFE_INTEGER`, unlike the nanosecond stamp `FileStamp` has to carry as a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalVersion {
    #[ts(type = "number")]
    pub id: u64,
    #[ts(type = "number")]
    pub ts: u64,
    #[ts(type = "number")]
    pub group: u64,
    /// Absolute, spelled the way the project spells its root.
    pub path: String,
    pub kind: VersionKind,
    pub source: VersionSource,
    #[ts(type = "number")]
    pub size: u64,
    /// A copy exists. False for a deletion, and for a file that was over the size limit.
    pub kept: bool,
    /// The other end of a rename, relative to the root.
    pub moved: Option<String>,
    pub label: Option<String>,
}

/// One capture burst, in a folder's or project's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalHistoryGroup {
    #[ts(type = "number")]
    pub group: u64,
    #[ts(type = "number")]
    pub ts: u64,
    pub source: VersionSource,
    /// Files in this group under the scope that was asked about.
    pub files: u32,
    pub label: Option<String>,
    /// Every file is a first sighting: one "First snapshot" line, with no file list to open.
    pub snapshot: bool,
}

/// One file's history, or why it has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalFileHistory {
    pub versions: Vec<LocalVersion>,
    /// Set when the file is not kept at all: an empty list alone would read as a bug.
    pub untracked: Option<String>,
}

/// What a side of a diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum LocalSide {
    Version {
        #[ts(type = "number")]
        id: u64,
    },
    /// The file on disk, read now.
    Current,
}

/// Whether a project has a history, and what it costs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalHistoryStatus {
    /// The effective switch: what capture is doing now, whatever the store holds.
    pub enabled: bool,
    /// Nothing can be read yet, because the store is opening or was never opened.
    pub loading: bool,
    /// The store is held by another cide, so nothing can be recorded or restored here.
    pub read_only: bool,
    pub baseline_done: bool,
    #[ts(type = "number")]
    pub rows: u64,
    #[ts(type = "number")]
    pub paths: u64,
    #[ts(type = "number")]
    pub disk_bytes: u64,
}

/// The versions of a file whose text holds a query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalSearchResult {
    #[ts(type = "Array<number>")]
    pub matches: Vec<u64>,
    /// The scan stopped at its limit, so an older version may match too.
    pub partial: bool,
}

/// What restoring a folder to a moment would do, for the confirmation. Paths relative to the
/// root. A courtesy: the restore works this out again when it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalRestorePlan {
    pub write: Vec<String>,
    pub remove: Vec<String>,
    /// Files with no version that old. They stay as they are.
    pub left: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalRestoreFailure {
    pub path: String,
    pub reason: String,
}

/// What restoring a folder did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalRestoreReport {
    pub written: u32,
    pub removed: u32,
    pub left: u32,
    pub failed: Vec<LocalRestoreFailure>,
}

/// What a restore did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LocalRestored {
    Rewritten,
    Recreated,
}
