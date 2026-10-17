//! Invocation-local configuration for an experimental two-agent conversation.
use crate::{ConsoleHarness, PaneId, SessionId, SessionState, TabId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerChatSelection {
    pub harness: ConsoleHarness,
    /// Native model identifier; OpenCode uses provider/model. None inherits the CLI default.
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerChat {
    pub main: PaneId,
    pub peer: PaneId,
    pub main_harness: ConsoleHarness,
    pub selection: Option<PeerChatSelection>,
    pub paused: bool,
    /// Prevent either panel launching independently during atomic initial setup.
    #[serde(default)]
    pub starting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerChatReceipt {
    pub id: String,
    pub sender: PaneId,
    pub status: String,
    pub detail: String,
}

/// Review completion is distinct from transport delivery and the reviewer's runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PeerChatReviewState {
    NotRequested,
    RequestQueued,
    AwaitingReply,
    ReplyQueued,
    Completed,
    Cancelled,
    Skipped,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerChatReviewStatus {
    pub tab: TabId,
    pub writer: SessionId,
    pub reviewer: SessionId,
    pub request_id: Option<String>,
    pub state: PeerChatReviewState,
    pub writer_state: SessionState,
    pub reviewer_state: SessionState,
    pub blocked_reason: Option<String>,
    pub ready_to_finalize: bool,
}

/// Full invocation-local snapshot, versioned so an initial pull cannot replace a newer event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerChatReviews {
    #[ts(type = "number")]
    pub revision: u64,
    pub reviews: Vec<PeerChatReviewStatus>,
}
