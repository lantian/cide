//! Every agent conversation cide has hosted for a project: the Agents panel's **Sessions** tab.
//! (M134)
//!
//! Before this, nothing outlived a pane. `workspace.json` names the sessions its panes hold and
//! forgets one the moment the pane closes; `agent-runs.json` keeps runs, but only work and
//! OpenSpec runs (a review's checkout is deleted at quit) and only the last fifty per project. A
//! console conversation closed on Monday was a transcript on disk under an id nobody could name,
//! and getting back to it meant finding that id by hand.
//!
//! So cide keeps a **journal**: one [`SessionRecord`] per conversation it has seen, written as it
//! sees them — a pane in the tree, a run in the registry — and never removed because the pane or
//! run went away. `cide_core::sessions` owns the merge rules and is the place to read them.
//!
//! # Keyed by conversation, not by pane or run
//!
//! [`SessionRecord::id`] is the id the *harness* would resume — a claude `--session-id`, a codex
//! thread id, an opencode `ses_…` — because that is what the user is looking for: a
//! conversation. Keying by cide's handle instead would list one conversation twice the moment it
//! was re-opened (a resumed pane gets a fresh handle), and would list a run's mirror tab as a
//! second session beside the run. Until a harness has named its conversation (a codex console
//! learns its thread id from the first event), the handle stands in and [`SessionRecord::known`]
//! is `false`; `cide_core::sessions` re-keys the row when the real id arrives.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::agents::Harness;
use crate::ids::{PaneId, RunId, SessionId, TaskId};

/// Where a conversation was hosted — the Sessions tab's filter.
///
/// First seen wins, with one exception: a run kind outranks a pane kind, because a run's mirror
/// tab is a pane of type [`Self::Tab`] showing a conversation that is a [`Self::Subagent`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SessionKind {
    /// The project console's primary pane — the pinned tab's conversation.
    Console,
    /// Another agent pane split into the console tab.
    ConsoleSplit,
    /// A Claude tab of its own (`ClaudeFull`), opened by the user.
    Tab,
    /// A tab `cide_session_open` started on one piece of work, in its own worktree.
    Worker,
    /// The planner tab cide opens by itself.
    Planner,
    /// A review tab cide opens by itself for finished runs.
    Reviewer,
    /// A subagent run dispatched to a role.
    Subagent,
    /// A GitLab merge-request review run.
    MrReview,
    /// An OpenSpec Propose/Explore/Apply session.
    OpenSpec,
}

impl SessionKind {
    /// Whether this kind is a run's, which outranks a pane's — see the type's doc.
    pub fn is_run(self) -> bool {
        matches!(self, Self::Subagent | Self::MrReview | Self::OpenSpec)
    }
}

/// One conversation, as the journal remembers it. Durable, and outbound inside [`SessionRow`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionRecord {
    /// The harness's conversation id, or cide's session handle standing in until the harness
    /// names one — see [`Self::known`].
    pub id: String,
    /// Whether [`Self::id`] is the harness's own id, the one a resume can use.
    pub known: bool,
    pub kind: SessionKind,
    pub harness: Harness,
    /// The pane's title, or the run's role label.
    pub title: String,
    /// The name the user gave the conversation with `/rename`, read from its transcript. Drawn
    /// instead of [`Self::title`] when there is one; kept apart so a pane's next sighting, which
    /// knows only its own title, cannot overwrite it.
    #[serde(default)]
    pub name: Option<String>,
    /// Where the harness was started — the directory it filed the transcript under, and the one
    /// a resume must start in (a worktree for a run, the project root for a console).
    #[ts(type = "string")]
    pub cwd: PathBuf,
    /// cide's handle when it was last seen, which is what "is it live" is asked with.
    #[serde(default)]
    pub session: Option<SessionId>,
    #[serde(default)]
    pub run: Option<RunId>,
    /// The role's label for a run.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub task: Option<TaskId>,
    #[serde(default)]
    pub task_title: Option<String>,
    /// What it was first asked: a run's prompt, or the first user message of a transcript,
    /// clipped to [`PROMPT_CLIP`] characters.
    #[serde(default)]
    pub prompt: Option<String>,
    /// The worktree it stands in, when it stands in one.
    #[serde(default)]
    pub branch: Option<String>,
    #[ts(type = "number")]
    pub started_unix_ms: u64,
    #[ts(type = "number")]
    pub last_seen_unix_ms: u64,
}

/// How much of a first prompt the journal keeps. A row shows one line of it and the search reads
/// it; a whole brief is kilobytes the file does not need.
pub const PROMPT_CLIP: usize = 300;

/// One row of the Sessions tab. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionRow {
    pub record: SessionRecord,
    /// A child is running for it right now.
    pub live: bool,
    /// A pane of this project is showing it, so Open reveals rather than resumes.
    pub pane: Option<PaneId>,
    /// Whether its transcript can be searched: claude, qwen and codex keep one cide can read;
    /// opencode's is a database it does not.
    pub searchable: bool,
}

/// A project's sessions, newest first. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionListing {
    pub rows: Vec<SessionRow>,
}

/// A transcript that says what was searched for. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptHit {
    /// [`SessionRecord::id`] of the conversation.
    pub id: String,
    /// The first match, with a little of the text around it.
    pub snippet: String,
}

/// A transcript search's answer. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum TranscriptSearch {
    /// Every searchable transcript was read, or the file budget ran out (`capped`).
    Done {
        hits: Vec<TranscriptHit>,
        capped: bool,
    },
    /// A newer search replaced this one before it finished.
    Cancelled,
}
