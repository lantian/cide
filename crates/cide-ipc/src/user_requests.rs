//! User decisions belong to the conversation that asked, independently of the task board.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{TaskAuthor, TaskId, TaskQuestion, TaskStatus, sessions::SessionRecord};

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
