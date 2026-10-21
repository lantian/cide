//! Read-only explanations of task dispatch, separate from the committed task record.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{AgentId, ProjectId, RunId, TaskId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskDispatchReason {
    pub code: String,
    pub text: String,
    pub task: Option<TaskId>,
    pub milestone: Option<String>,
}

impl TaskDispatchReason {
    pub fn new(code: &str, text: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            text: text.into(),
            task: None,
            milestone: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TaskDispatchState {
    Ready,
    Blocked,
    Queued,
    Running,
    Idle,
    Paused,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskDispatchStatus {
    pub project: ProjectId,
    pub task: TaskId,
    pub agent: Option<AgentId>,
    pub label: Option<String>,
    pub state: TaskDispatchState,
    pub run: Option<RunId>,
    pub reasons: Vec<TaskDispatchReason>,
}
