//! Errors crossing the IPC boundary.
//!
//! Every variant is a tag the frontend can branch on. Nothing here is a bare `String`:
//! `TabPinned` is a thing the UI reacts to by dimming a close button, and matching on
//! prose to discover that is how error handling rots.

use cide_ipc::{PaneId, ProjectId, SplitId, TabId, UnsavedTab};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// `rename_all_fields` as well as `rename_all`: the first renames variants, the second the
// *fields* of struct variants. Without it `UnsavedChanges { tabs }` would be fine but the
// next multi-word field added here would reach the webview as snake_case, which has already
// been a real bug in this project.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    content = "detail"
)]
#[ts(export)]
pub enum CoreError {
    /// `tabs[0]` is the pinned project console. It cannot be closed or reordered, and this
    /// is enforced here rather than in the frontend so that no UI bug can lose it.
    #[error("the project console tab is pinned and cannot be closed or moved")]
    TabPinned,

    /// Closing would discard unsaved edits, and the caller did not pass `force`.
    ///
    /// Enforced here, in the domain, for exactly the reason [`Self::TabPinned`] is: the
    /// frontend's dialog is a courtesy, and a courtesy is not a guard. A dropped promise, a
    /// component that unmounts mid-await, a second window that never rendered the dialog —
    /// each of those is a lost buffer if Rust closes on request. So `close_tab` and
    /// `close_project` refuse, and the *only* way past is a caller that says `force`,
    /// which is a caller that has already shown the user this list.
    ///
    /// Carries the tabs rather than a count, so the frontend can name them without a second
    /// round trip against a workspace that has since moved on.
    #[error("{} would be discarded: {}", plural(tabs.len()), names(tabs))]
    UnsavedChanges { tabs: Vec<UnsavedTab> },

    /// The primary Claude pane cannot be closed while it is the only pane in its tab —
    /// that would leave the pinned console empty with no way back.
    #[error("the primary Claude pane cannot be closed while it is the only pane")]
    PanePrimary,

    /// A project must keep at least its console tab.
    #[error("a project must keep at least one tab")]
    LastTab,

    /// A tab must keep at least one pane.
    ///
    /// Distinct from [`Self::LastTab`] because these are tagged so the frontend can word
    /// the message: closing the only pane of an ordinary tab has nothing to do with tabs,
    /// and telling the user it does is worse than saying nothing.
    #[error("a tab must keep at least one pane")]
    LastPane,

    /// A project must have at least one root directory.
    #[error("a project must have at least one root")]
    NoRoots,

    #[error("no such project: {0}")]
    NoSuchProject(ProjectId),
    #[error("no such tab: {0}")]
    NoSuchTab(TabId),
    #[error("no such pane: {0}")]
    NoSuchPane(PaneId),
    #[error("no such split: {0}")]
    NoSuchSplit(SplitId),
    #[error("no such command: {0}")]
    NoSuchCommand(String),

    /// An index-based reorder was given an out-of-range position.
    #[error("index {index} is out of range (len {len})")]
    IndexOutOfRange { index: usize, len: usize },

    /// The pane tree violated an invariant. Only ever produced by `layout::validate`, and
    /// only ever a bug in this crate.
    #[error("pane tree invariant violated: {0}")]
    Invariant(String),

    #[error("{0}")]
    Io(String),

    #[error("{0}")]
    Serde(String),
}

/// `1 unsaved file` / `3 unsaved files`, for [`CoreError::UnsavedChanges`].
///
/// A named function rather than an inline expression in the `#[error]` attribute: the format
/// string is not a place anyone reads carefully, and "1 unsaved files" in an error a user
/// sees is the kind of thing that survives for years.
fn plural(count: usize) -> String {
    match count {
        1 => "1 unsaved file".to_string(),
        n => format!("{n} unsaved files"),
    }
}

/// The tabs' basenames, comma-separated.
///
/// Basenames rather than full paths: the message is one line and three absolute paths in a
/// monorepo do not fit on one. The structured `tabs` field carries the paths for the dialog,
/// which has room for them.
fn names(tabs: &[UnsavedTab]) -> String {
    tabs.iter()
        .map(|t| t.title.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
