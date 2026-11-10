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
    #[serde(default)]
    pub planning_policy: Option<crate::PlanningPolicy>,
    #[serde(default)]
    pub planning_phase: Option<crate::PlanningPhase>,
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

/// What a conversation is doing, for the Sessions tab's status glyph and its waiting counts.
/// Outbound. (M199)
///
/// **One table, decided in Rust**, by `cide_core::sessions::activity`, from a run's
/// [`crate::agents::RunState`] when the conversation is a run's and from the hook-derived
/// [`crate::SessionState`] otherwise. The Subagents tab's run rows draw the same run states with
/// the same glyphs, so a run reads the same on both tabs.
///
/// *A question* is deliberately not a variant. Whether a conversation has asked the user
/// something is a fact of the Waiting tab's requests and board questions, which the webview
/// already holds and re-reads on its own events; folding it in here would make the listing stale
/// the moment an answer was typed, until the next re-list. The panel lays it over this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SessionActivity {
    /// A child is starting, or a run is queued or starting.
    Starting,
    /// A turn is in progress.
    Working,
    /// Alive, its turn over, nothing asked of the user.
    Idle,
    /// Its turn ended in a way that wants a reply.
    AwaitingInput,
    /// Stopped at a permission prompt.
    Permission,
    Paused,
    /// A run that cannot go on without someone — a refused verify, a lost worktree.
    Blocked,
    /// A run that failed.
    Failed,
    /// The conversation ended normally.
    Finished,
    /// Stopped, cancelled, interrupted, or exited with a non-zero code.
    Stopped,
}

/// A folder the user made in the Sessions tab's *Opened* tree. Durable (in the journal) and
/// outbound. (M199)
///
/// A grouped conversation stays in its group after it ends — it never drops into *Closed* — and
/// the journal never prunes it; see `cide_core::sessions::Organize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionGroup {
    /// Chosen by the client that created it (a UUID), so the panel can start renaming the new
    /// row without waiting for an answer to learn its id.
    pub id: String,
    pub name: String,
    /// The group it is nested in; `None` at the top level.
    #[serde(default)]
    pub parent: Option<String>,
    /// Folded in the tree. Durable rather than webview state: a group the user folded is a
    /// statement about the project that should survive a restart, the way a tree's expansion is.
    #[serde(default)]
    pub collapsed: bool,
}

/// One change to the Sessions tab's grouping. Inbound, to `sessions_organize`. (M199)
///
/// One command with an edit rather than a command per gesture: the seven share one lock, one
/// validation and one "tell the other windows" step, and seven commands would be seven places
/// for one of those to be forgotten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "op")]
#[ts(export)]
pub enum SessionEdit {
    /// A new, empty group, last among its siblings.
    Create {
        id: String,
        name: String,
        parent: Option<String>,
    },
    Rename {
        id: String,
        name: String,
    },
    /// Remove a group. What it held — sessions and groups — moves up into its parent; at the top
    /// level, sessions become loose again (and an ended one goes back to *Closed*).
    Delete {
        id: String,
    },
    /// Re-parent a group and place it before `before` (a sibling under the new parent), or last.
    /// Refused into its own subtree.
    Move {
        id: String,
        parent: Option<String>,
        before: Option<String>,
    },
    Collapse {
        id: String,
        collapsed: bool,
    },
    /// Fold or unfold every group: the tree's *Expand all* and *Collapse all*.
    CollapseAll {
        collapsed: bool,
    },
    /// Put a conversation into a group, or take it out with `None`.
    Assign {
        conversation: String,
        group: Option<String>,
    },
    /// cide's own name for a conversation, drawn before its `/rename` name; `None` (or blank)
    /// clears it. It is not typed into the session: the harness never learns of it.
    Label {
        conversation: String,
        label: Option<String>,
    },
}

/// One row of the Sessions tab. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionRow {
    pub record: SessionRecord,
    /// A child is running for it right now — alive, not merely still registered.
    pub live: bool,
    /// The handle of that child, which can differ from [`SessionRecord::session`] (a resumed
    /// conversation gets a fresh one). The key the panel joins `cide://session-state` on.
    pub live_session: Option<SessionId>,
    /// A pane of this project is showing it, so Open reveals rather than resumes.
    pub pane: Option<PaneId>,
    /// Whether its transcript can be searched: claude, qwen and codex keep one cide can read;
    /// opencode's is a database it does not.
    pub searchable: bool,
    pub activity: SessionActivity,
    /// The [`SessionGroup`] it is filed in.
    pub group: Option<String>,
    /// cide's own name for it — see [`SessionEdit::Label`].
    pub label: Option<String>,
}

/// A project's sessions, newest first, and its groups in their order. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionListing {
    pub rows: Vec<SessionRow>,
    /// Every group, siblings in the user's order.
    pub groups: Vec<SessionGroup>,
}

/// A submitted human prompt, not a terminal keystroke or a CLI's injected context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserInput {
    /// Transcript byte offset: two equal prompts remain two separate entries.
    pub id: String,
    pub ordinal: u32,
    pub text: String,
}

/// An ascending page of prompts from one conversation. Older pages end before `ordinal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserInputPage {
    pub inputs: Vec<UserInput>,
    pub total: u32,
    pub has_previous: bool,
    /// Distinguishes a missing/unsupported transcript from a conversation with no prompts.
    pub available: bool,
    /// Changes when a transcript is replaced or truncated, so clients discard old pages.
    pub generation: u32,
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
