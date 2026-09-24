//! The New project wizard's wire shapes. (M97)
//!
//! # What the wizard is
//!
//! Until M97 cide could only *open* a folder somebody else had already made into a project. The
//! wizard is the other half: pick what kind of project it is — empty, OpenSpec-driven, or driven
//! by cide's own task tracker — pick where it lives, and cide creates the folder, `git init`s it,
//! sets up whatever the kind needs and opens it. On the task-tracker path it also hands the
//! project's console a first line, so the product owner starts by drafting milestones, roles and
//! tasks from the user's brief instead of sitting at an empty prompt.
//!
//! # Why these are their own module
//!
//! `spec`'s and `tasks`' reason: nothing else in the workspace model knows a project that does
//! not exist yet, and the one type the wizard shares with the rest — [`ProjectId`], which the
//! creation answers with — is imported, not re-declared.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::ProjectId;

/// Which road through the wizard the user took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NewProjectKind {
    /// A folder and nothing else. `git init` is still offered: most projects want one, and it is
    /// the one step that is awkward to do later from inside cide.
    Empty,
    /// `openspec init`, an optional project context, optionally subagents.
    Spec,
    /// Subagents (and therefore git), and a brief the console turns into milestones, roles and
    /// tasks once the project is open.
    Tasks,
}

/// What is at a path the user has typed or picked, for the location step's live hints.
///
/// Every field is a fact about the disk *now*; the wizard re-probes as the path changes and
/// `project_new` re-checks everything it relies on rather than trusting an old probe.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NewProjectProbe {
    /// The path as cide will use it: `~` expanded, relative paths refused (see `problem`).
    pub path: String,
    pub exists: bool,
    pub is_dir: bool,
    /// How many entries the directory already holds; `0` for one that does not exist. The
    /// wizard keeps them — it never deletes — but says so, because "new project" in a folder full
    /// of somebody's files is usually a mistyped path.
    pub entries: u32,
    /// The directory is itself a work tree (`.git` inside it).
    pub has_git: bool,
    /// The nearest enclosing work tree, when the directory is *inside* one without being its
    /// root. Subagent worktrees would then be the enclosing repository's, which is rarely meant.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inside_repo: Option<String>,
    pub has_openspec: bool,
    pub has_cide: bool,
    /// A project over this path is already open; creating will activate it.
    pub open_already: bool,
    /// Why this path cannot be used at all, as a sentence. `None` means it can.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// Why the `openspec` CLI cannot be run, as a sentence; `None` when it can. Carried on the
    /// probe so the OpenSpec step can say so *before* the user commits, not as a failed step.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openspec_missing: Option<String>,
}

/// Everything the wizard collected, in one call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NewProjectRequest {
    pub path: String,
    pub kind: NewProjectKind,
    pub git_init: bool,
    /// OpenSpec's `context:`; ignored off the Spec road.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_context: Option<String>,
    /// Switch subagents on. Always true on the Tasks road, whatever is sent.
    pub agents: bool,
    /// What the user wants built; the Tasks road's console seed. Empty means the console asks.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
}

/// One line of the creation checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NewProjectStep {
    Folder,
    Git,
    Openspec,
    Agents,
    Open,
    Brief,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NewProjectStepState {
    Running,
    Done,
    Failed,
}

/// `cide://project-new-progress`: a step started, finished or failed.
///
/// The final answer of `project_new` repeats every failure, so a window that missed an event —
/// it subscribed a frame late — still ends with the truth. The events are for the checklist
/// moving while the user watches; the answer is the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NewProjectProgress {
    pub step: NewProjectStep,
    pub state: NewProjectStepState,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// What `project_new` did.
///
/// A failed *optional* step does not fail the call — the folder exists and the project is open,
/// and refusing to say so because OpenSpec's CLI was missing would leave the user with a project
/// they did not know had been created. `project` is `None` only when the open itself failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NewProjectOutcome {
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectId>,
    pub path: String,
    pub failures: Vec<NewProjectProgress>,
}
