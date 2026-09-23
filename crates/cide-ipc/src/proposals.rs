//! Changes an agent proposes to what only the user may change. (M83)
//!
//! Milestones and the files their gates read are the user's: a goal the agent working towards it
//! can rewrite is not a goal (`crate::milestones`' header). But the agent is often the first to
//! see that a gate is wrong or incomplete — *this gate should also run `e2e.sh all`* — and before
//! this the only thing it could do was write a comment and hope somebody read it. A proposal is
//! that comment made actionable: the whole change, ready to apply, queued for the user, who
//! accepts it (cide applies it exactly) or rejects it (it is gone).
//!
//! Proposals live in the profile's state directory, not in the repository: they are a
//! conversation between this machine's agents and this machine's user, and an accepted one lands
//! in the repository as the change itself — a commit, or the milestones in `.cide/config.json`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::TaskId;
use crate::milestones::MilestonePlan;
use crate::tasks::TaskAuthor;

/// One proposed change, waiting for the user. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Proposal {
    /// `p-1`, `p-2`, … — per project, never reused.
    pub id: String,
    pub title: String,
    /// Why, in markdown, as the agent wrote it.
    pub rationale: String,
    pub by: TaskAuthor,
    #[ts(type = "number")]
    pub created_unix_ms: u64,
    /// The task it came out of, if any; accepting or rejecting it is noted there.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    pub change: ProposalChange,
}

/// What accepting a proposal does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ProposalChange {
    /// Replace the milestones with this plan. `before` is the plan when it was proposed, so the
    /// user sees the difference and a plan that moved since is noticed.
    Plan {
        plan: MilestonePlan,
        before: MilestonePlan,
    },
    /// Write these files (guarded ones — anything else goes through a branch like all work).
    Files { files: Vec<ProposedFile> },
    /// Nothing to apply: a suggestion in words. Accepting it acknowledges it.
    Note,
}

/// One file in a [`ProposalChange::Files`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProposedFile {
    /// Relative to the project root, `/`-separated.
    pub path: String,
    /// The new content; `None` deletes the file.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// The content when it was proposed (`None`: the file did not exist). Accepting refuses if
    /// the file no longer says this — the proposal was written against something else.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// A unified diff of `before` → `content`, for reading.
    pub diff: String,
}
