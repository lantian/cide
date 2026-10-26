//! Transient editor state. Never included in the workspace or its persistence format.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// CodeMirror owns the history encoding. Rust only arbitrates revisions between windows.
/// Keeping it in the process lets the last editor window close without stranding the other
/// view's undo stack. The serialized state is fetched on demand, never in workspace events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SharedDocumentSnapshot {
    pub version: u32,
    pub state: String,
    pub saved_text: String,
    pub stamp: Option<crate::FileStamp>,
}
