//! User decisions belong to the conversation that asked, independently of the task board.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{SessionId, TaskAuthor, TaskId, TaskQuestion, TaskStatus, sessions::SessionRecord};

/// Identifies a pending native operation, rather than a new user turn to deliver later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NativeQuestionSource {
    pub session: SessionId,
    pub generation: SessionId,
    pub conversation: String,
    pub request: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserRequestQuestion {
    pub id: String,
    pub header: String,
    pub question: TaskQuestion,
    pub custom: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserRequestAnswer {
    pub id: String,
    pub text: String,
    pub selected_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserRequestTask {
    pub id: TaskId,
    pub title: String,
    pub status: TaskStatus,
    pub parent: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum UserRequestKind {
    Question {
        question: TaskQuestion,
    },
    Questions {
        questions: Vec<UserRequestQuestion>,
        source: NativeQuestionSource,
    },
    MoveTasks {
        milestone: String,
        goal: TaskId,
        tasks: Vec<UserRequestTask>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum UserRequestResponse {
    Answer {
        text: String,
        selected_ids: Vec<String>,
    },
    Answers {
        answers: Vec<UserRequestAnswer>,
    },
    Approve {
        selected_ids: Vec<TaskId>,
        note: String,
    },
    Reject {
        note: String,
    },
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserRequest {
    pub id: String,
    pub title: String,
    pub message: String,
    pub task: Option<TaskId>,
    pub author: TaskAuthor,
    pub request: UserRequestKind,
    pub origin: SessionRecord,
    pub wait: bool,
    pub created_unix_ms: u64,
    pub response: Option<UserRequestResponse>,
    // Task changes are checkpointed before applying, and completed once durable.
    pub applying: bool,
    pub delivered: bool,
    pub delivery_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserRequestsView {
    pub rev: u64,
    pub requests: Vec<UserRequest>,
}
