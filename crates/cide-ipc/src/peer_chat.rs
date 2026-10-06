//! Invocation-local configuration for an experimental two-agent conversation.
use crate::{ConsoleHarness, PaneId};
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
