//! Errors crossing the IPC boundary.
//!
//! Every variant is a tag the frontend can branch on. Nothing here is a bare `String`:
//! `TabPinned` is a thing the UI reacts to by dimming a close button, and matching on
//! prose to discover that is how error handling rots.

use cide_ipc::{PaneId, ProjectId, SplitId, TabId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
#[ts(export)]
pub enum CoreError {
    /// `tabs[0]` is the pinned project console. It cannot be closed or reordered, and this
    /// is enforced here rather than in the frontend so that no UI bug can lose it.
    #[error("the project console tab is pinned and cannot be closed or moved")]
    TabPinned,

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
