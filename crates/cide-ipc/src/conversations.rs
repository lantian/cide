//! Readable, harness-independent conversations for companion devices.
use crate::remote::PermissionPrompt;
use crate::{Harness, ProjectId, SessionId, SessionState};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationSummary {
    pub project: ProjectId,
    pub id: String,
    pub harness: Harness,
    pub title: String,
    pub preview: String,
    #[ts(type = "number")]
    pub updated_unix_ms: u64,
    pub session: Option<SessionId>,
    pub state: Option<SessionState>,
    pub writable: bool,
    pub can_resume: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationItem {
    pub id: String,
    pub ordinal: u32,
    /// user, assistant, tool, error, or notice. Unknown kinds remain readable.
    pub kind: String,
    pub text: String,
    pub tool: Option<String>,
    pub status: Option<String>,
    pub detail_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationPage {
    pub conversation: ConversationSummary,
    /// Source identity. A change invalidates every old cursor and item.
    pub generation: String,
    pub revision: String,
    pub items: Vec<ConversationItem>,
    pub before: Option<String>,
    pub after: String,
    pub reset: bool,
    pub available: bool,
    pub detail: Option<String>,
    pub prompt: Option<PermissionPrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "action",
    deny_unknown_fields
)]
#[ts(export)]
pub enum ConversationRequest {
    List {
        project: ProjectId,
    },
    Read {
        project: ProjectId,
        id: String,
        /// Session views follow /clear and /resume; archived views stay on the named conversation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        follow_session: Option<SessionId>,
        #[serde(default)]
        generation: Option<String>,
        #[serde(default)]
        before: Option<String>,
        #[serde(default)]
        after: Option<String>,
    },
    Detail {
        project: ProjectId,
        id: String,
        generation: String,
        item: String,
        offset: u32,
    },
    Send {
        project: ProjectId,
        id: String,
        session: SessionId,
        operation: String,
        text: String,
    },
    Resume {
        project: ProjectId,
        id: String,
        operation: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "action"
)]
#[ts(export)]
pub enum ConversationResponse {
    List {
        conversations: Vec<ConversationSummary>,
    },
    Read {
        page: Box<ConversationPage>,
    },
    Detail {
        text: String,
        next: Option<u32>,
    },
    Receipt {
        operation: String,
        status: String,
        session: Option<SessionId>,
    },
}
