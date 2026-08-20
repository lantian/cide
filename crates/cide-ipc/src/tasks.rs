//! The task tracker's wire types. (M18)
//!
//! `.cide/tasks.json` is simultaneously the in-memory domain, the disk format and the wire
//! format, which is the same arrangement `workspace.json` has and for the same reason: a second
//! copy of [`Task`] living in `cide-tasks` would be two shapes that disagree the first time one
//! of them gains a field.
//!
//! The file lives **in the project**, not in `$XDG_STATE_HOME`. It is the medium the
//! product-owner session and its subagents exchange state through *and* the record the team
//! reads: it is committed, it appears in a pull request, and `git log -p .cide/tasks.json` is a
//! full history nobody had to build a feature for. `workspace.json` is machine-written state
//! about one user's windows and belongs nowhere near a repository. The two look superficially
//! alike — both are debounced, atomically written JSON with a `rev` — and that resemblance is
//! the reason this paragraph exists.
//!
//! # What a task deliberately does not have
//!
//! Every one of these was proposed, and every one is cut for the same single reason: it is a
//! field an agent must be *taught* to fill and a column the panel must *draw*, and neither
//! changes what anybody does next. A tracker whose rows carry six attributes nobody reads is a
//! tracker agents fill in wrongly and users stop trusting.
//!
//! * **Priority** — the list is ordered and an ordered list *is* the priority. A separate
//!   priority field means two orderings on screen, and the one the eye follows is the wrong one.
//! * **Estimate** — a number a language model invents, that nothing measures against.
//! * **Labels** — a second grouping axis competing with [`TaskStatus`], which is the axis the
//!   panel already groups by.
//! * **Due date** — nothing in this app can act on a date passing, so it would be decoration
//!   that ages into a lie.
//! * **Parent/child** — the orchestrator holds the decomposition in its own context; a tree the
//!   panel would have to draw at 320px buys hierarchy at the cost of the flat list being
//!   readable.
//! * **A human assignee** — the human is the product owner and is on every task. A field whose
//!   value is constant is not a field.
//! * **`blocked_by`** — the orchestrator sequences by *dispatching*, so a dependency edge would
//!   be advisory at best; and a dependency graph the panel cannot draw is a graph nobody reads.
//!   What a real block looks like in practice — a failed worktree merge — is [`TaskStatus::Todo`]
//!   plus a comment naming the conflicting paths, which is a shape an agent can actually write.
//!
//! Adding any of them later is a field plus a column plus a line in every agent's prompt. That
//! is the price, and it is worth paying only for one that changes a decision.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{AgentId, CommentId, ProjectId, TaskId};

/// Where a task is.
///
/// # The two variants that were proposed and lost
///
/// **`Blocked`.** It is a *reason*, not a place. Every tracker that ships it accumulates tasks
/// parked there for weeks with nothing on the row saying what the block was or whether it still
/// holds, because the state itself carries no text and nobody is obliged to add any. A blocked
/// task here is `Todo` with a comment — which is strictly more informative, is a shape an agent
/// can write without being taught a new vocabulary, and is exactly what a failed worktree merge
/// produces (`cide_agent_integrate` comments the conflicting paths and leaves the task where it
/// was).
///
/// **`Cancelled`.** This file is a working queue, not an archive: a cancelled task is one the
/// user deletes, and `git log -p .cide/tasks.json` already holds the record of it having
/// existed, in more detail than a tombstone row could. A `Cancelled` group would also compete
/// with `Done` for the bottom of the panel, where neither is being read.
///
/// `Copy` and `Hash` because the panel groups by this and the group order is a lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TaskStatus {
    Todo,
    Doing,
    Review,
    Done,
}

/// Who wrote a comment.
///
/// A **tagged enum rather than a bare author string**, so the panel can style the user's own
/// lines without string-matching a name that a config file is free to choose. With a string,
/// "is this mine" would be `author === 'you'` against a value an agent definition could legally
/// claim, and the styling would be wrong in the one case it matters: a user reading back a
/// conversation to work out which half of it they said.
///
/// [`Self::Agent`] carries `label` beside `agent` for the reason [`crate::AgentRun::agent_label`]
/// states one layer up: the label is copied at write time, so a comment written by `developer`
/// still reads correctly after the role has been renamed. A comment that renamed its own author
/// when a config file changed would be a record that lies about what happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum TaskAuthor {
    /// The person at the keyboard.
    User,
    /// The project's primary session — the product owner that decomposes and dispatches.
    Orchestrator,
    /// A dispatched subagent, writing through the `cide_task_comment` MCP tool.
    Agent { agent: AgentId, label: String },
}

impl TaskAuthor {
    /// [`Self::User`], as a function, because that is the shape `#[serde(default = "…")]` takes.
    ///
    /// Named rather than reached through a `Default` impl on purpose. A `Default for TaskAuthor`
    /// would make "the user" the silent answer *everywhere* the type appears — including a comment
    /// whose author failed to deserialise, which would sign an agent's line with the user's name.
    /// The only place an absent author legitimately means the user is [`Task::created_by`] on a
    /// file written before that field existed, and that call site says so out loud.
    #[must_use]
    pub const fn user() -> Self {
        Self::User
    }
}

/// One line of a task's log.
///
/// # Append-only for **agents**. The user may edit and delete. (M21)
///
/// It was append-only for everyone, and that argument is still the right one where it bites: an
/// agent that can quietly rewrite a comment after the fact leaves neither the user reading the
/// panel nor the next agent reading the task any way to tell that what they are acting on is not
/// what was written.
///
/// What it never justified was stopping the person at the keyboard from fixing their own typo or
/// removing a comment that should not be there. **The boundary is the caller, not the comment's
/// author**: `TasksStore::edit` refuses [`TaskEdit::EditComment`] and [`TaskEdit::DeleteComment`]
/// for every author but [`TaskAuthor::User`], and no MCP tool exposes either — the arrangement
/// `TasksStore::remove` already had, for the same reason.
///
/// Three fields carry that, and each is load-bearing rather than decorative — see their own
/// notes: [`Self::id`], [`Self::edited_at_unix_ms`] and [`Self::deleted`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskComment {
    /// Stable identity, and the reason a delete survives a merge.
    ///
    /// The file is merged with a stale copy of itself by unioning comments, and that union used
    /// to be *structural* — `(author, at_unix_ms, text)` were the only three fields there were,
    /// so `==` said exactly what the rule said. Under editing that rule is wrong twice: an edit
    /// changes `text`, so the stale copy's original re-appears beside the new one, and a delete
    /// removes a comment the stale copy still has, so the next merge puts it back.
    ///
    /// A comment written by this build gets a fresh v4 uuid. A comment already in a file gets one
    /// **derived from the old merge key** by [`Self::legacy_id`] — deterministic, so two readers
    /// of the same file agree, and identical under exactly the conditions structural equality was
    /// identical under. That is what makes the change invisible to every task file that already
    /// exists rather than a migration.
    #[serde(default)]
    pub id: CommentId,
    pub author: TaskAuthor,
    /// The text, verbatim.
    ///
    /// **Never rendered as markup.** It is model-authored, and rendering model-authored markup
    /// inside the IDE's own chrome is an injection surface bought for nothing at a 320px panel
    /// width. `TasksPanel` draws it as text with whitespace preserved.
    pub text: String,
    /// Milliseconds since the Unix epoch, stamped by Rust when the comment lands.
    ///
    /// Written where the mutation is applied rather than taken from the caller, for
    /// [`crate::ViewPosition::touched_at`]'s reason: a timestamp supplied by whoever is writing
    /// is a timestamp an agent can get wrong, and this one orders the log.
    pub at_unix_ms: u64,
    /// When the user last edited it, or `None` for a comment that stands as written.
    ///
    /// Drawn by the panel, and that is the point rather than a nicety. The user editing an
    /// *agent's* comment is the one case left that could mislead a later reader — the whole
    /// hazard the append-only rule existed to prevent, with the user in the agent's place — and a
    /// visible "edited" mark is what keeps the log honest about it.
    ///
    /// It also decides the merge: for one id, the copy with the newer edit wins. A comment that
    /// was never edited sorts as its `at_unix_ms`, so an edited copy always beats an untouched
    /// one no matter which file it came from.
    #[serde(default)]
    pub edited_at_unix_ms: Option<u64>,
    /// A tombstone. The comment is gone from the panel and from every agent's view of the task.
    ///
    /// Kept as a flag rather than removed from the vector, because a removed comment is one the
    /// next merge with a stale file puts straight back. `deleted` only ever goes false → true, so
    /// the merge rule is "either side deleted ⇒ deleted" and no ordering question arises.
    ///
    /// The text is **cleared** when this is set. The tombstone has to persist; the words do not,
    /// and a deleted comment whose content sat in `.cide/tasks.json` for ever would be a delete
    /// that did not delete.
    #[serde(default)]
    pub deleted: bool,
}

impl TaskComment {
    /// The id a comment written before [`TaskComment::id`] existed gets on load.
    ///
    /// A pure function of the **old merge key**, so it is stable across readers and across runs,
    /// and two legacy comments collide exactly when structural equality would have called them
    /// one — which is the behaviour the file already had.
    ///
    /// FNV-1a rather than a real hash: this is not security, `cide-ipc` has no hashing dependency
    /// and must not grow one for a compatibility shim, and a collision costs what it always cost.
    #[must_use]
    pub fn legacy_id(author: &TaskAuthor, at_unix_ms: u64, text: &str) -> CommentId {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(match author {
            TaskAuthor::User => "user".to_string(),
            TaskAuthor::Orchestrator => "orchestrator".to_string(),
            TaskAuthor::Agent { agent, label } => format!("agent:{}:{label}", agent.0),
        }
        .as_bytes());
        eat(&at_unix_ms.to_le_bytes());
        eat(text.as_bytes());
        CommentId(format!("legacy-{h:016x}"))
    }
}

/// One task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Task {
    /// `t-17`. Stable for the life of the file and never reused — see [`TaskId`].
    pub id: TaskId,
    /// One line. What the row shows and what an agent quotes.
    pub title: String,
    /// The whole statement of the work, possibly empty.
    ///
    /// Not `Option<String>`: "no body" and "an empty body" are the same fact about a task, and
    /// a nullable string would make every reader handle two spellings of it.
    pub body: String,
    pub status: TaskStatus,
    /// The role this task is **for** — never "who is working on it now".
    ///
    /// The distinction is the whole reason this field has a doc comment. A task outlives the run
    /// that failed at it: the run exits, the task stays assigned to `developer`, and the next
    /// dispatch picks it up. "Which agent is on it *now*" is [`crate::AgentRun::task`] read the
    /// other way round, derived at render time by `TasksPanel/model.ts`'s `agentChip` and stored
    /// nowhere.
    ///
    /// Two writers for one fact is how a task ends up claiming an agent that exited an hour ago,
    /// with nothing anywhere in a position to correct it — the run that would have known is gone.
    pub agent: Option<AgentId>,
    /// Oldest first, which is the order the panel renders and the order an agent reads.
    pub comments: Vec<TaskComment>,
    /// Who asked for this task: the person at the keyboard, the orchestrator, or a subagent.
    /// (M21)
    ///
    /// # Why this is a field now, having deliberately not been one
    ///
    /// `TaskStore::create` used to answer *who asked for this* by **seeding the log**: a task an
    /// agent created opened with one comment saying so, and a task the user created opened with
    /// nothing. The reasoning was that a `creator` field is one more thing an agent must be taught
    /// to fill and one more column the panel must draw.
    ///
    /// Both halves of that turned out to be wrong here. Nothing has to be *taught* — every
    /// `create` call already carries a [`TaskAuthor`], because that is where a comment's author
    /// comes from, so the field is filled by the seam rather than by a model. And the absence of a
    /// line was doing the work of "the user made this", which is a fact inferred from a *missing*
    /// record: a user who deletes that seeded comment — which M21 lets them do — would silently
    /// turn an agent's task into their own.
    ///
    /// So it is stored, once, at creation, and never edited afterwards: [`TaskEdit`] has no
    /// variant for it and no MCP tool can express one. A creator that could be rewritten is
    /// provenance that lies, which is the thing the log's append-only rule exists to prevent.
    ///
    /// **Absent means [`TaskAuthor::User`]**, which is what a file written before this field
    /// existed decays to — and `cide_tasks::repair` recovers the real answer for those files from
    /// the seeded comment before anything reads them.
    #[serde(default = "TaskAuthor::user")]
    pub created_by: TaskAuthor,
    pub created_unix_ms: u64,
    /// Bumped on every accepted mutation, and **load-bearing beyond display**: the
    /// out-of-process merge in `cide_tasks` resolves a task that changed on both sides by
    /// keeping the higher `updated_unix_ms`.
    pub updated_unix_ms: u64,
}

/// The whole of `.cide/tasks.json`.
///
/// `{ "schemaVersion": 1, "rev": 42, "tasks": [ … ] }`, written with `to_vec_pretty`, because a
/// one-line JSON file makes every change a whole-file diff and this file's diffs are read by
/// people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskFile {
    /// Bumped when the on-disk shape changes, so an old file can be migrated rather than
    /// silently misread — [`crate::Workspace::schema_version`]'s job, for this file.
    pub schema_version: u32,
    /// Bumped on every accepted mutation.
    ///
    /// It rides `cide://tasks-changed` and the receiving store drops any snapshot whose `rev` is
    /// not newer, exactly as `workspace_changed` does. This file genuinely has several writers —
    /// two cide windows and every dispatched agent — so out-of-order arrival is not theoretical
    /// here the way it is for a single-writer registry.
    pub rev: u64,
    /// **A list, not a map**, and the order is meaningful.
    ///
    /// Two reasons, and the first is about people: this file is read in pull requests, where a
    /// list has an order a human can follow top to bottom and a map has whatever order the
    /// serialiser felt like. The second is about the shape: a map would put each id in two
    /// places — the key and the value's `id` — and the first hand edit that disagreed with
    /// itself would be a task with two identities.
    ///
    /// There is deliberately **no `order` field**. The array *is* the order. A stored rank is a
    /// second source of truth that a hand-edited or merged file can contradict, and then
    /// something has to decide which of the two orderings the panel obeys.
    pub tasks: Vec<Task>,
}

impl TaskFile {
    /// The shape this build writes.
    pub const CURRENT_SCHEMA: u32 = 1;
}

impl Default for TaskFile {
    fn default() -> Self {
        Self {
            schema_version: Self::CURRENT_SCHEMA,
            rev: 0,
            tasks: Vec::new(),
        }
    }
}

/// What cide knows about a project's tracker right now.
///
/// A tagged union with first-class `Absent` and `Unreadable` states, on
/// [`crate::DiagnosticsSnapshot`]'s exact argument: **an empty array cannot tell "nothing is
/// tracked yet" from "there is no tracker in this project" from "the file will not parse"**, and
/// those are three different sentences on the screen and three different sets of buttons under
/// them. A panel that renders all three as an empty list is the confident-empty-list failure
/// that module exists to prevent, wearing different clothes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum TaskBoard {
    /// No `.cide/tasks.json` here. Not an error — most projects have never had one.
    ///
    /// The panel offers exactly one button, *New task*, which creates the file. `hint` is the
    /// prose above it, built in Rust where the fact is known, and `path` is printed in full,
    /// because a control that quietly adds a tracked file to somebody's repository is a surprise
    /// commit and the surprise is the part that has to go.
    Absent {
        hint: String,
        #[ts(type = "string")]
        path: PathBuf,
    },
    /// The file is there and does not parse, or could not be read.
    ///
    /// This variant exists so the panel can offer **nothing that writes**: Reveal and Retry, and
    /// no *New task*, no *Reset*, no repair. An app that "recovers" from an unparseable tracker
    /// by overwriting it has destroyed the user's data in order to fix its own display — and
    /// the most likely cause of an unparseable tracker is a half-resolved merge conflict, which
    /// is to say a file that still contains both sides of everything.
    ///
    /// `error` is the parser's own message, shown verbatim: the user is the one who can fix it
    /// and they need the line number.
    Unreadable {
        #[ts(type = "string")]
        path: PathBuf,
        error: String,
    },
    /// Read. `tasks: []` is a real, reportable zero — a tracker that exists and is empty.
    Ready { tasks: Vec<Task>, rev: u64 },
}

/// A new task. Inbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct TaskNew {
    pub project: ProjectId,
    /// The one required field. A task with no title is a row nobody can act on.
    pub title: String,
    /// Absent means empty. [`Task::body`] is not nullable; this is, only so the common call —
    /// the orchestrator creating five titles at once — need not send five empty strings.
    #[ts(optional)]
    pub body: Option<String>,
    /// The role this is for, if it is known at creation. Absent means unassigned.
    #[ts(optional)]
    pub agent: Option<AgentId>,
    /// Where it starts. Absent means [`TaskStatus::Todo`], which is what every caller but the
    /// compose dialog sends. (M21)
    ///
    /// # This field is the user's, and structurally not an agent's
    ///
    /// [`TaskStore::create`](../../cide_tasks/struct.TaskStore.html#method.create) used to say
    /// there was deliberately no wire shape for this, because *"a task created directly into
    /// `Done` is a status nobody moved it to"*. That worry is about **provenance**, and it is
    /// still right about the caller it was written for: an agent minting finished work would
    /// leave a tracker whose statuses record nothing.
    ///
    /// It does not apply to a person filling in a dialog with the four states in front of them
    /// and pressing Create — *I am starting this now* is an ordinary thing to say, and the old
    /// shape made them say it in two writes instead of one. The restriction that matters is
    /// kept, and kept **structurally**: `cide_agents::tools`' create tool has no such argument
    /// and `TaskSink::create` has no such parameter, so the MCP path cannot express one. It is
    /// not a check that could be forgotten; it is a signature.
    #[ts(optional)]
    pub status: Option<TaskStatus>,
}

/// One change to one task. Inbound.
///
/// # Why an enum and not a `TaskPatch` of options
///
/// [`crate::SettingsPatch`] is a struct of `Option`s and works, so this shape needs a reason.
/// It is one field: **unassigning**.
///
/// In a patch struct, `agent: None` means "not mentioned, leave it alone". Expressing "set it to
/// nobody" therefore needs `Option<Option<AgentId>>` — a shape that serialises as
/// absent-versus-null, that `deny_unknown_fields` cannot police (both are well-formed), that
/// reads identically in the TypeScript type, and that every caller gets wrong exactly once
/// before discovering the difference. `SettingsPatch` gets away with per-field `Option` because
/// none of its fields is *itself* nullable; [`Task::agent`] is.
///
/// A variant per edit also makes the mutation the caller intended legible at the seam, which
/// matters more here than for settings: these calls come from MCP tools a language model fills
/// in, and a five-variant enum with one or two fields each is called correctly far more often
/// than one nine-field object.
///
/// # There is deliberately no `Reorder`
///
/// The array is the order and reordering it is a real gesture — but the panel has no drag
/// affordance in v1, so a `Reorder` variant would be a wire shape no caller reaches. That is the
/// dead-control failure wearing a different hat, and this repository has already deleted one
/// instance of it: [`crate::SplitIntent`]'s removed `Diff` variant, which existed, was reachable
/// from no code path, and whose handling arm would have minted a pane showing nothing. Add it
/// with the gesture, in the same commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    deny_unknown_fields
)]
#[ts(export)]
pub enum TaskEdit {
    SetTitle {
        title: String,
    },
    SetBody {
        body: String,
    },
    SetStatus {
        status: TaskStatus,
    },
    /// `agent: null` **unassigns**, and is the case this whole enum is shaped around.
    Assign {
        agent: Option<AgentId>,
    },
    /// Append one line to the log. There is no edit and no delete — see [`TaskComment`].
    ///
    /// The author is *not* a field here. It is decided by the app from the connection the call
    /// arrived on — `cide_agent_rpc`'s header names `CIDE_RUN`/`CIDE_SESSION`, read from the
    /// child's own environment — for the `spawned_as` reason: identity must come from the
    /// environment, never from a payload the caller composes, or an agent can sign a comment
    /// as the user.
    Comment {
        text: String,
    },
    /// Replace one comment's text. **The user only** — see [`TaskComment`].
    ///
    /// The author is not a field here either, and for the stronger version of the same reason:
    /// on `Comment` the caller's identity decides whose name goes on the line, and here it
    /// decides whether the call is allowed at all. `TasksStore::edit` refuses this for every
    /// author but [`TaskAuthor::User`], so an agent cannot rewrite a comment even by naming
    /// itself the user — the identity it would have to forge is the one it never supplies.
    EditComment {
        id: CommentId,
        text: String,
    },
    /// Tombstone one comment. **The user only.**
    ///
    /// Not a removal from the vector: see [`TaskComment::deleted`] for why a comment that is
    /// merely gone comes back on the next merge with a stale file.
    DeleteComment {
        id: CommentId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_task() -> Task {
        Task {
            id: TaskId("t-17".into()),
            title: "Add the retry bar".into(),
            body: String::new(),
            status: TaskStatus::Doing,
            agent: Some(AgentId("developer".into())),
            comments: vec![TaskComment {
                id: CommentId("c-1".into()),
                author: TaskAuthor::Agent {
                    agent: AgentId("developer".into()),
                    label: "Developer".into(),
                },
                text: "worktree merged clean".into(),
                at_unix_ms: 1_700_000_000_000,
                edited_at_unix_ms: None,
                deleted: false,
            }],
            created_by: TaskAuthor::Orchestrator,
            created_unix_ms: 1_699_999_999_000,
            updated_unix_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn a_task_round_trips_under_its_wire_names() {
        let json = serde_json::to_string(&a_task()).expect("serialize");
        // Equality alone would pass with snake_case on both sides, and every multi-word field
        // would read `undefined` in the webview.
        for wire in ["createdBy", "createdUnixMs", "updatedUnixMs", "atUnixMs"] {
            assert!(json.contains(wire), "missing {wire} in {json}");
        }
        assert_eq!(serde_json::from_str::<Task>(&json).unwrap(), a_task());
    }

    /// A task from a file written before `Task::created_by` existed still parses, and reads as
    /// the user. (M21)
    ///
    /// The decay is deliberate and it is only half the answer: `cide_tasks::repair` recovers the
    /// real creator of an agent-made task from the comment `create` used to seed. What this test
    /// pins is that the *parse* cannot fail — `.cide/tasks.json` is the user's committed data, and
    /// a build that refused to open last week's file over a field it added would be the tracker
    /// locking the team out of its own repository.
    #[test]
    fn a_task_written_before_the_creator_field_still_parses() {
        let json = r#"{
            "id": "t-3",
            "title": "Write the loader",
            "body": "",
            "status": "todo",
            "agent": null,
            "comments": [],
            "createdUnixMs": 1699999999000,
            "updatedUnixMs": 1700000000000
        }"#;
        let task: Task =
            serde_json::from_str(json).expect("a file this build has to be able to open");
        assert_eq!(task.created_by, TaskAuthor::User);
    }

    #[test]
    fn the_board_names_its_three_states_apart() {
        // The whole point of the union: three shapes, three `kind`s, no empty array standing
        // in for any of them.
        let absent = TaskBoard::Absent {
            hint: "No tracker here yet.".into(),
            path: PathBuf::from("/repo/.cide/tasks.json"),
        };
        assert!(
            serde_json::to_string(&absent)
                .unwrap()
                .contains("\"kind\":\"absent\"")
        );
        let ready = TaskBoard::Ready {
            tasks: vec![],
            rev: 3,
        };
        assert!(
            serde_json::to_string(&ready)
                .unwrap()
                .contains("\"kind\":\"ready\"")
        );
    }

    #[test]
    fn assign_can_say_nobody() {
        // The case the enum exists for: an explicit null is an unassign, and it is not the same
        // message as one that never mentioned the field.
        let edit: TaskEdit = serde_json::from_str(r#"{"kind":"assign","agent":null}"#).unwrap();
        assert_eq!(edit, TaskEdit::Assign { agent: None });
    }

    #[test]
    fn an_inbound_task_refuses_a_field_it_does_not_know() {
        // `deny_unknown_fields` is the whole drift alarm: a frontend that still sends
        // `priority` learns so here rather than having it silently dropped.
        let err = serde_json::from_str::<TaskNew>(
            r#"{"project":"00000000-0000-0000-0000-000000000000","title":"x","priority":1}"#,
        );
        assert!(err.is_err(), "unknown field was accepted");
    }
}
