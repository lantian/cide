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
//! * **A human assignee** — the human is the product owner and is on every task. A field whose
//!   value is constant is not a field.
//!
//! Adding any of them later is a field plus a column plus a line in every agent's prompt. That
//! is the price, and it is worth paying only for one that changes a decision. Three additions
//! have now paid it — [`Task::change`] (M28, its doc carries the argument) and, under
//! [`Task::links`] (M30), two entries this list used to hold:
//!
//! * **`blocked_by`** was cut with "the orchestrator sequences by *dispatching*, so a dependency
//!   edge would be advisory at best" — and for as long as the edge would have been advisory, the
//!   cut was right. [`LinkType::BlockedBy`] is admitted because it is not advisory: auto-dispatch
//!   skips a task whose blockers are not done, and an explicit dispatch of one is *refused*, with
//!   the blocker named in the refusal. It changes what a run reads (`cide_task_list` prints the
//!   blocking pair), what the panel draws (a chip on both cards), and what accepting the task
//!   does (nothing, until the blocker is done) — the three-part test above, passed in full. What
//!   it does not change: a *narrative* block — a failed worktree merge — is still
//!   [`TaskStatus::Todo`] plus a comment naming the conflicting paths, because that block has a
//!   story to tell and an edge cannot tell it.
//! * **Parent/child** was cut because a *tree* the panel must draw at 320px buys hierarchy at
//!   the cost of the flat list being readable. The flat list is still the rendering;
//!   [`LinkType::SubtaskOf`] is a pointer drawn as a chip on the card, never as indentation in
//!   the list. What changed is where the decomposition lives: the orchestrator used to hold it
//!   only in its own context window, which a compaction or a restart silently discards, and a
//!   durable edge is that plan surviving the session that made it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{AgentId, ChangeName, CommentId, ProjectId, SessionId, TaskAttachmentId, TaskId};

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
    /// The text, verbatim. **Markdown**, since M27.
    ///
    /// This said "never rendered as markup" until M31, and by then it had been wrong for a
    /// milestone: `TaskMarkdown.tsx` renders it. The refusal it recorded was about a *road*, not
    /// about markup — `string -> HTML string` into `dangerouslySetInnerHTML`, model-authored, in
    /// the IDE's own chrome. That road is still not taken. The parser produces no HTML node of
    /// any kind, every string reaches the DOM as a React child through React's escaping, and a
    /// link is drawn as accented text that activates nothing; `TaskMarkdown.tsx`'s header carries
    /// the whole argument and `check:markdown` asserts the parser can never invent such a node.
    ///
    /// Leaving the stale sentence here was not free. It was mirrored into the MCP input schema
    /// (`cide_agents::tools`), so every agent was told in the tool it was about to call that its
    /// report would not be formatted — and wrote one flat paragraph, which is exactly what the
    /// card then drew. A comment is read by a person; the wording an agent is handed here is the
    /// thing that decides whether it is readable.
    ///
    /// One dialect note: the card parses with `softBreak: 'break'`, so a lone newline is a line.
    /// A comment is a message, not a `.md` file — the markdown *preview* keeps CommonMark.
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
    /// Files attached to this comment, oldest first. (M39)
    ///
    /// `#[serde(default)]` for [`Task::links`]' reason and with the same non-bump of
    /// [`TaskFile::CURRENT_SCHEMA`]. Records are tombstoned, never removed — see
    /// [`TaskAttachment::deleted`]. A deleted comment keeps its attachment *records* (the
    /// tombstones have to survive the merge) but `cide_tasks` removes their bytes.
    #[serde(default)]
    pub attachments: Vec<TaskAttachment>,
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

/// The directory under a project root that holds attachment bytes, relative. (M39)
///
/// Here in `cide-ipc` rather than beside `cide_tasks::TASKS_RELATIVE`, because two crates need
/// the layout and only one of them may depend on `cide-tasks`: the store writes the file, and
/// `cide_agents::tools` prints its **absolute path** in `cide_task_get` so an agent can `Read`
/// an image the user attached. A second spelling of the formula in `cide-agents` would be the
/// two-copies drift this crate's header exists to prevent, so the formula lives on the record
/// itself — [`TaskAttachment::relative_path`].
///
/// Committed, like the rest of `.cide/` — a screenshot a teammate attached to a task should be
/// there when the task is. cide writes nothing to any `.gitignore` about it.
/// **The pre-M68 layout.** Read, never written — see [`TaskAttachment::legacy_relative_path`].
pub const ATTACHMENTS_DIR: &str = ".cide/attachments";

/// The directory holding one subdirectory per task, relative to a project root: `.cide/tasks`.
///
/// Spelled here as well as in `cide_tasks::TASKS_DIR` for [`ATTACHMENTS_DIR`]'s stated reason: two
/// crates need the attachment layout and only one of them may depend on `cide-tasks`, so the formula
/// lives on the record itself. The two constants are asserted equal by `cide_tasks`' own test.
pub const TASKS_DIR_RELATIVE: &str = ".cide/tasks";

/// What a task's attachment directory is called inside its own directory.
pub const ATTACHMENTS_LEAF: &str = "attachments";

/// What an attachment's bytes are, coarsely: a picture the card can draw, or a file it names.
///
/// Decided **once, at import, by sniffing the bytes** (`cide_core::image::sniff`), never by the
/// extension — a `.png` that is a text file would otherwise be handed to `<img>` and draw
/// nothing. It is a hint for the panel and for the list an agent reads, and *only* that: the
/// asset-protocol grant that lets a thumbnail load re-sniffs the file on disk every time, because
/// a record in a committed JSON file is a claim anybody can edit and the grant is a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AttachmentKind {
    Image,
    File,
}

/// One file attached to a task's body or to a comment. (M39)
///
/// **Metadata only.** The bytes are a file on disk at [`Self::relative_path`], copied there by
/// `cide_tasks::attachments::import`; this record is what travels in `.cide/tasks.json`, on the
/// wire and in an agent's `cide_task_get`. A record that carried bytes would put a screenshot
/// through Tauri's JSON IPC as an array of decimal numbers (`crate::image`'s header) and into a
/// file whose diffs people read.
///
/// Immutable once written, except for [`Self::deleted`]. There is no rename and no replace: a
/// person who wants a different file attaches a different file, and that is what keeps the merge
/// rule to one line — see `cide_tasks::union_attachments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskAttachment {
    pub id: TaskAttachmentId,
    /// The file's name as a person sees it, and the last component of its path on disk.
    ///
    /// The original name, sanitised to one path component (no separators, no NUL, not empty),
    /// kept verbatim otherwise — the OS opener picks an application by the real extension, and
    /// a person recognises `design-v3.png`, not a uuid. Each attachment has a directory of its own
    /// named by its id, which is what lets two attachments on one task share a name.
    pub name: String,
    pub bytes: u64,
    pub kind: AttachmentKind,
    /// Who attached it. From the connection, never from the payload — [`TaskComment`]'s rule.
    pub added_by: TaskAuthor,
    pub added_unix_ms: u64,
    /// A tombstone, on [`TaskComment::deleted`]'s exact terms: one-way, kept in the vector so a
    /// merge with a stale file cannot resurrect the record, and the merge rule is "either side
    /// deleted ⇒ deleted". The bytes are removed when this is set; the record stays.
    #[serde(default)]
    pub deleted: bool,
}

impl TaskAttachment {
    /// Where the bytes live, relative to the project root:
    /// `.cide/tasks/<task>/attachments/<attachment>/<name>`.
    ///
    /// A directory per attachment rather than a uuid-prefixed file name, so [`Self::name`] is the
    /// real file name on disk and nothing has to be decoded to open it.
    ///
    /// # Under the task's own directory since M68
    ///
    /// It used to be `.cide/attachments/<task>/…`, and the reason given for putting the task id in
    /// the path was that `git rm -r .cide/attachments/t-17` should be one gesture. That reason is
    /// better served here: `git rm -r .cide/tasks/t-17` takes the task's description, its whole
    /// conversation **and** every file anybody attached to it, in one gesture — where the old layout
    /// left `.cide/attachments/t-17/` behind, because deleting a task cleaned up one tree and not the
    /// other.
    ///
    /// [`Self::legacy_relative_path`] is the old spelling, still read.
    #[must_use]
    pub fn relative_path(&self, task: &TaskId) -> PathBuf {
        PathBuf::from(TASKS_DIR_RELATIVE)
            .join(task.as_str())
            .join(ATTACHMENTS_LEAF)
            .join(self.id.as_str())
            .join(&self.name)
    }

    /// Where the bytes lived before M68: `.cide/attachments/<task>/<attachment>/<name>`.
    ///
    /// # Why this is still here
    ///
    /// Because a relocation can be interrupted. The schema 1 → 2 conversion moves every attachment
    /// directory, and a crash, a full disk or a `Ctrl-C` in the middle leaves some files moved and
    /// some not. A reader that knew only the new path would answer *no such attachment* for the rest
    /// — a thumbnail that never loads and an Open button that does nothing, with the bytes sitting
    /// safely on disk the whole time.
    ///
    /// So every read tries [`Self::relative_path`] first and falls back to this. That makes a partial
    /// move harmless rather than a bug report, and it costs one `stat` on a path that is usually
    /// absent. It is **not** a write path: nothing new is ever written here.
    #[must_use]
    pub fn legacy_relative_path(&self, task: &TaskId) -> PathBuf {
        PathBuf::from(ATTACHMENTS_DIR)
            .join(task.as_str())
            .join(self.id.as_str())
            .join(&self.name)
    }
}

/// Where an attachment goes. Inbound. (M39)
///
/// Three shapes because there are three gestures: files dropped on the card body, files added to
/// a comment that already exists, and files that ride along with a comment being written — the
/// composer's "report with a screenshot", which must land as **one** mutation so a refused file
/// never leaves a comment behind that names an attachment it does not have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    deny_unknown_fields
)]
#[ts(export)]
pub enum AttachTarget {
    Task,
    Comment { id: CommentId },
    NewComment { text: String },
}

/// A file cide wrote somewhere temporary on the caller's behalf, for a gesture that has no
/// task or comment to attach to yet. Outbound. (M39)
///
/// The clipboard case: a screenshot pasted into the comment composer or the New task dialog has
/// no id to be imported under, so it is staged as a file and its path rides on the eventual
/// `task_attach`/`task_new` like a picked file would. `cide_tasks::attachments::import` consumes
/// a staged source rather than copying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StagedFile {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
}

/// One status transition, recorded where the mutation is applied. (M27)
///
/// # Why a structured record rather than a seeded comment
///
/// The alternative was `TasksStore::edit` appending "moved to Review" lines to the log, which
/// costs nothing on the wire — and it is the design [`Task::created_by`] already tried and
/// reversed, for reasons that all apply here: the panel would have to *parse prose* to draw the
/// transitions apart from conversation, the user can delete comments (so the history would be
/// editable in exactly the way provenance must not be), and every agent reading the task would
/// wade through bookkeeping lines to find the words. A separate vector is invisible to the log,
/// undeletable by design (no [`TaskEdit`] variant touches it), and renders collapsed.
///
/// `by` is stamped from the connection's identity exactly as a comment's author is — see
/// [`TaskEdit::Comment`] — so an agent cannot record a move as the user. `from` is read off the
/// task at the moment of the change, not supplied: a caller-supplied `from` is one an agent can
/// get wrong, and the pair is what makes each row legible on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskStatusChange {
    pub from: TaskStatus,
    pub to: TaskStatus,
    pub by: TaskAuthor,
    /// Stamped by Rust when the change lands, for [`TaskComment::at_unix_ms`]'s reason.
    pub at_unix_ms: u64,
}

/// One kind of edge between two tasks. (M30)
///
/// Each directed kind is **stored in one canonical direction** and read the other way round at
/// render time — [`Task::agent`]'s one-writer rule applied to edges. Storing both directions
/// would mean two rows for one fact, and the first merge with a stale file where only one row
/// survived would be a link that exists from one task and not from the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LinkType {
    /// Symmetric: reads as "related to" from both ends, and either end may unlink it. Gates
    /// nothing — it is a cross-reference for a reader, never an instruction to the dispatcher.
    Related,
    /// Directed, and the one kind that changes what dispatch *does*: this task is not dispatched
    /// until the target is [`TaskStatus::Done`]. Stored on the **blocked** task, so the field
    /// that changes a task's dispatch sits on the task whose dispatch it changes; the target's
    /// "blocks" reading is derived at render.
    BlockedBy,
    /// Directed: this task is one piece of the target's decomposition. Stored on the **child** —
    /// decomposing creates children pointing at one parent, one edge per create, rather than a
    /// parent whose edge list must be rewritten once per child. Gates nothing.
    SubtaskOf,
}

/// One stored edge between two tasks, on the source task only. (M30)
///
/// Unlike a [`TaskStatusChange`], an edge is **mutable** — unlink exists — so it carries the two
/// fields that let it survive the out-of-process merge: a tombstone and a stamp. There is
/// deliberately **no `LinkId`**: a comment needed a uuid because edits change its text while its
/// identity must persist, but an edge *is* its `(link, target)` pair — there is nothing else to
/// it — so the pair is the merge identity and a minted id would be a second name for the same
/// fact, with all of [`TaskFile::tasks`]' two-identities hazard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskLink {
    pub link: LinkType,
    /// The other task. Never validated against existence *here* — `cide-tasks` refuses a target
    /// that does not exist at the gesture, but a target deleted afterwards leaves the edge
    /// dangling and legal, because ids are never reused (see [`TaskId`]) and so a dangling edge
    /// can never silently come to mean new work. Renderers mark it; nothing prunes it.
    pub target: TaskId,
    /// A tombstone — but unlike [`TaskComment::deleted`] it is a **toggle**, not one-way.
    /// A deleted comment is replaced by writing a new comment under a new id; re-linking the
    /// same `(link, target)` pair recreates the *same* key, so "deleted wins for ever" would
    /// make an unlink permanent after any merge. The stamp below is what resolves the toggle;
    /// `cide_tasks::union_links` carries the argument.
    #[serde(default)]
    pub deleted: bool,
    /// Stamped by Rust when the link or unlink lands, for [`TaskComment::at_unix_ms`]'s reason —
    /// and load-bearing beyond display: per `(link, target)` key, the merge keeps the copy with
    /// the newer stamp.
    pub at_unix_ms: u64,
}

/// One edge named at task creation. Inbound. (M30)
///
/// No tombstone and no stamp — those are the store's to write, for the same reason a comment's
/// timestamp is: a stamp supplied by whoever is writing is a stamp an agent can get wrong, and
/// this one decides a merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct TaskLinkSpec {
    pub link: LinkType,
    pub target: TaskId,
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
    /// The live Claude session this task's work was handed to, if it went to one. (M28)
    ///
    /// # Why this is a second field and not a widened [`Self::agent`]
    ///
    /// Because a role and a session are different kinds of thing with different id spaces —
    /// `AgentId` is a name a file in `.cide/agents/` gives itself, a [`SessionId`] is the uuid
    /// cide passes to `claude --session-id` — and because of what keying them apart buys.
    ///
    /// **Only one of the two is ever set.** `TaskStore::edit` clears the other on every write, so
    /// "who is on this" stays one fact with one writer; the card reads whichever is present.
    ///
    /// **And the auto-dispatch trigger fires on `agent` alone.** That is the whole of why
    /// handing a task to an open session cannot start it twice: assigning a *role* is a dispatch
    /// gesture (`cide_agents::autodispatch`'s assignment edge), and this field is not one, so a
    /// session target is structurally incapable of also spawning a subagent. It is not a check
    /// that could be forgotten — there is no code path from here into the queue.
    ///
    /// `#[serde(default)]`, on [`Self::history`]'s posture: `.cide/tasks.json` is committed, and
    /// a build that refused last week's file over a field it added would be the tracker locking
    /// the team out of its own repository. Absent means the task went to a role, or nowhere.
    ///
    /// **Not a claim that the session still exists.** A conversation the user closed leaves this
    /// pointing at nothing, exactly as [`Self::agent`] can name a role whose file was deleted;
    /// the panel resolves it against what is live and says so when it cannot, which is
    /// `agentChip`'s rule one layer up.
    #[serde(default)]
    #[ts(optional)]
    pub session: Option<SessionId>,
    /// The OpenSpec change this task implements, if any — the folder name under
    /// `openspec/changes/`. (M28)
    ///
    /// # Why this one is a field, when six others are not
    ///
    /// The header above lists what a task deliberately does not carry, and every one of them is
    /// cut by the same test: *a field an agent must be taught to fill and a column the panel must
    /// draw, and neither changes what anybody does next.* This passes that test three times over,
    /// which is why it is here and `priority` is not. It changes what a dispatched run **reads**
    /// — `spec_preamble` points it at this change's proposal, design and checklist. It changes
    /// what the panel **draws** — a progress bar off that checklist, the delta as requirement
    /// cards, a validity badge. And it changes what accepting the task **does** — integrate, then
    /// archive this change, then `Done`. A task without it takes none of those paths and renders
    /// exactly as it did before this field existed.
    ///
    /// **It is the change this task belongs to, never "the change an agent is currently on".**
    /// [`Self::agent`]'s argument, restated for the same reason: one writer for one fact. What a
    /// live run is working through is `AgentRun::task` followed to this field, derived at render
    /// time and stored nowhere.
    ///
    /// `#[serde(default)]`, on [`Self::history`]'s posture and for its stated reason — and note
    /// that this does **not** bump [`TaskFile::CURRENT_SCHEMA`]: an additive field with a default
    /// is not a shape change, because every file that already exists still means exactly what it
    /// said.
    ///
    /// Not validated here — this crate is the wire shape. `cide-tasks` refuses a name that is not
    /// OpenSpec's kebab grammar before it can reach the file, because this value ends up in a
    /// `Path::join`.
    #[serde(default)]
    #[ts(optional)]
    pub change: Option<ChangeName>,
    /// Typed edges to other tasks — this task's own gestures only. (M30)
    ///
    /// # Why this one is a field, when the header cut it twice
    ///
    /// The module header carries the reversal at length. The short form, against the same
    /// three-part test [`Self::change`] passed: a [`LinkType::BlockedBy`] edge changes what a
    /// run **reads** (the blocking pair rides `cide_task_list`), what the panel **draws** (a
    /// chip on both cards), and what dispatch **does** (auto-dispatch skips a blocked task and
    /// an explicit dispatch of one is refused, naming the blocker).
    ///
    /// **Each edge is stored once, on its canonical side** — see [`LinkType`] — and the other
    /// task's reading (`blocks`, `subtask`) is derived at render from the whole board, which
    /// every consumer already holds. Entries may be tombstoned ([`TaskLink::deleted`]) and may
    /// dangle after the target is deleted; both are legal states of the *file*, refused only as
    /// *gestures* — `cide-tasks`' `validate` doc says why the difference matters.
    ///
    /// `#[serde(default)]`, on [`Self::history`]'s posture and for its stated reason — and, as
    /// with [`Self::change`], no [`TaskFile::CURRENT_SCHEMA`] bump: an additive field with a
    /// default is not a shape change.
    #[serde(default)]
    pub links: Vec<TaskLink>,
    /// Oldest first, which is the order the panel renders and the order an agent reads.
    pub comments: Vec<TaskComment>,
    /// Files attached to the body, oldest first. (M39)
    ///
    /// `#[serde(default)]` and no [`TaskFile::CURRENT_SCHEMA`] bump, on [`Self::links`]' posture.
    /// Records only; the bytes are at [`TaskAttachment::relative_path`].
    #[serde(default)]
    pub attachments: Vec<TaskAttachment>,
    /// Every status transition, oldest first. (M27)
    ///
    /// `#[serde(default)]` so every task file that already exists parses — the same posture
    /// [`Self::created_by`] took when it landed: `.cide/tasks.json` is the user's committed
    /// data, and a build that refused last week's file over a field it added would be the
    /// tracker locking the team out of its own repository. Recorded only on an actual change:
    /// a `SetStatus` naming the status the task already has writes no row.
    #[serde(default)]
    pub history: Vec<TaskStatusChange>,
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

/// One task as the *board* carries it: everything except its body, its log and its files. (M68)
///
/// # Why this is a separate type and not `Task` with three keys left out
///
/// Two reasons, and the second is the one that decides it.
///
/// `Task::body` and `Task::comments` carry no `#[serde(default)]` — deliberately, because for
/// those two an absent key is not a fact about the task but a truncated document. A `Task` with
/// them omitted is therefore a *hard* deserialize failure, and giving them defaults to make the
/// omission legal would re-open the argument [`TaskAuthor::user`] makes: once absent means
/// `User`, or means "no comments", nothing downstream can tell a task that has none from a
/// payload that did not carry any.
///
/// And that distinction is exactly what this type exists to keep. The board is index-shaped
/// because `cide://tasks-changed` used to carry every word ever written into a project's tracker
/// — 2.19 MB on a four-month-old board, of which the comments alone were 77.5% — and Tauri
/// delivers an event by inlining the payload into a JavaScript **source string** and `eval`ing it
/// on the GTK main loop (`tauri`'s `event::emit_js_script`). Measured: 10.2 ms to parse that board
/// as JS source against 2.9 ms as JSON, per window, per comment an agent wrote. `TASKS_CHANGED`'s
/// own doc has the rest.
///
/// So a row is what every consumer of the *list* already needed and nothing more: the panel's row
/// draws a glyph, an id, a title and an agent chip, and every off-panel reader
/// (`openCount`, the link pickers, `taskReveal`, the OpenSpec tab) reads status, id, title or
/// `change`. A whole task is fetched by id when something opens one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskRow {
    pub id: TaskId,
    pub title: String,
    pub status: TaskStatus,
    /// The role this task is **for** — see [`Task::agent`], which this mirrors exactly.
    pub agent: Option<AgentId>,
    /// `#[ts(optional)]` **paired with `skip_serializing_if`**, unlike [`Task::session`].
    ///
    /// That pairing is not tidiness. `crate::docker`'s own header spells the failure out: on its
    /// own, `#[ts(optional)]` changes the emitted *type* and not what serde writes, so the field
    /// the TypeScript calls `undefined` arrives as `null`, `=== undefined` is false, and the next
    /// property access throws. `Task` has carried that mismatch since M28 and `adapt.ts` absorbs
    /// it with `== null`; a new type is a chance not to add a third instance of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session: Option<SessionId>,
    /// `#[ts(optional)]` + `skip_serializing_if`, on [`Self::session`]'s argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub change: Option<ChangeName>,
    /// This task's own edges, tombstones included — [`Task::links`]' posture verbatim.
    ///
    /// Links stay on the row rather than moving to the content file with the log, because they
    /// are read *across* tasks: `blocks` is derived by scanning every other task's list, and
    /// auto-dispatch refuses a blocked task by reading its blockers' statuses. A reader that had
    /// to open one file per task to answer "is this one blocked" would be loading the whole board
    /// to render a list, which is the cost this type exists to avoid.
    #[serde(default)]
    pub links: Vec<TaskLink>,
    #[serde(default = "TaskAuthor::user")]
    pub created_by: TaskAuthor,
    pub created_unix_ms: u64,
    /// See [`Task::updated_unix_ms`] — the merge tiebreak and the panel's in-group sort key.
    ///
    /// **A write to a task's content must bump this**, or a comment stops moving its task's merge
    /// rank and its position in the panel. Nothing throws; the board just quietly stops reordering.
    pub updated_unix_ms: u64,
    /// How many live comments the task has. A **cache**; the content file is the truth.
    ///
    /// Named `…Count` rather than `comments`, and it is worth the extra word: a field called
    /// `comments` that is a number where every other reader expects a vector turns `.length` into
    /// `undefined` instead of a type error, and `undefined > 0` is false — so a task with forty
    /// comments would render as one with none, in silence. The name makes that a compile error.
    ///
    /// It is a cache because the count lives in a committed, hand-editable file separate from the
    /// comments it counts, so the two can disagree — after a hand edit, a partial merge, or a
    /// crash between two writes. `cide_tasks` recomputes it whenever it loads content and takes
    /// content's answer, on `repair`'s rule for an attachment record: the file on disk wins over
    /// the record that describes it.
    pub comment_count: u32,
    /// Live attachments across the body and every live comment, as one number. (M39's count.)
    ///
    /// A cache, on [`Self::comment_count`]'s terms. `cide_task_list` prints it because "there is a
    /// file on this one" changes whether an agent spends a `cide_task_get`.
    pub attachment_count: u32,
}

impl TaskRow {
    /// Project one task down to the row the board carries. (M68)
    ///
    /// # Why this is on the DTO rather than in `cide-tasks`
    ///
    /// [`TaskAttachment::relative_path`]'s reason, verbatim: two crates need the formula and only one
    /// of them may depend on `cide-tasks`. The store builds rows, and `cide_agents::tools` needs one
    /// to render `cide_task_get`'s header from a whole task it was handed — so the formula lives on
    /// the record, where there is exactly one of it.
    ///
    /// That single-producer property is the whole point. The two counts are the one part of a row that
    /// is not a field copy, and a second place computing them would be a second answer to *how many
    /// comments does this task have* — where both copies are plausible, and a wrong count looks exactly
    /// like a right one until somebody opens the card it describes.
    ///
    /// `deleted` is filtered on both, matching what the panel and `cide_task_list` draw: a tombstoned
    /// comment is one the user removed and `adapt.ts` drops it at the seam, so counting it would put a
    /// `3 comment(s)` on a card showing two. Attachments are counted across the body **and** every
    /// live comment, which is [`TaskAttachment`]'s own reading of where a task's files live — and a
    /// comment's attachments are skipped wholesale when the comment itself is a tombstone, because
    /// `TaskEdit::DeleteComment` tombstones them with it.
    #[must_use]
    pub fn of(task: &Task) -> Self {
        let live_comments = || task.comments.iter().filter(|comment| !comment.deleted);
        let live = |attachments: &[TaskAttachment]| {
            attachments
                .iter()
                .filter(|attachment| !attachment.deleted)
                .count()
        };
        let attachments = live(&task.attachments)
            + live_comments()
                .map(|comment| live(&comment.attachments))
                .sum::<usize>();
        Self {
            id: task.id.clone(),
            title: task.title.clone(),
            status: task.status,
            agent: task.agent.clone(),
            session: task.session,
            change: task.change.clone(),
            links: task.links.clone(),
            created_by: task.created_by.clone(),
            created_unix_ms: task.created_unix_ms,
            updated_unix_ms: task.updated_unix_ms,
            // Saturating rather than `as`: a `usize` that does not fit a `u32` is four billion comments
            // on one task, and a wrapping cast would report a small number for an enormous one — the
            // one way a count can be wrong that reads as ordinary.
            comment_count: u32::try_from(live_comments().count()).unwrap_or(u32::MAX),
            attachment_count: u32::try_from(attachments).unwrap_or(u32::MAX),
        }
    }
}

/// One whole task, as `task_get` answers it: its [`TaskRow`] and everything the row leaves out.
/// (M68)
///
/// # Why the row is nested rather than flattened in
///
/// Because the two counts have exactly one producer, `cide_tasks::row_of`, and they must keep it.
/// A shape that inlined the row's fields beside the content ones would have to either carry the
/// counts again — a second place deriving a number, where both copies are plausible and a wrong one
/// looks exactly like a right one — or omit them, which pushes the derivation into the *frontend*
/// adapter and puts the same second copy one layer further away.
///
/// Nesting keeps it to one: whoever builds this asks `row_of` for the row, and every consumer,
/// wire or webview, spreads that answer.
///
/// It also happens to be the shape the storage split is heading for — a row from the index joined
/// to a content file — so this is the join, written down once while both halves still live in one
/// file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskDetail {
    pub row: TaskRow,
    /// The whole statement of the work, possibly empty — [`Task::body`]'s posture verbatim.
    pub body: String,
    pub comments: Vec<TaskComment>,
    /// The body's files. A comment's ride in its own [`TaskComment::attachments`].
    pub attachments: Vec<TaskAttachment>,
    pub history: Vec<TaskStatusChange>,
}

/// One task's content: everything about it that is not in the index. (M68)
///
/// The whole of `.cide/tasks/<task-id>/task.json`:
/// `{ "body": "…", "comments": [ … ], "history": [ … ], "attachments": [ … ] }`.
///
/// # Why a task's own file, and why these four fields
///
/// Because this is where a tracker's bytes actually are. Measured on a four-month-old board: 2.19 MB
/// across 126 tasks, of which the comment text alone was 77.5% and the bodies another 18%. The index
/// beside it is 4.5% — and the index is what every *list* reads, what the board broadcasts, and what
/// a mutation has to rewrite. Splitting along that line makes the hot file bounded by task **count**
/// rather than by every word ever written into the project, with no policy deciding what is "cold".
///
/// The four fields are the ones no reader of a list needs and no reader of a card can do without.
/// `attachments` here is the **body's**; a comment's ride in its own [`TaskComment::attachments`],
/// which is why `cide_tasks::attachment_record` searches the two chained together and why splitting
/// them across two files would put one lookup on both sides of a lazy load.
///
/// Tombstones live here, as they do everywhere in this file's vocabulary: a deleted comment stays so
/// that a merge against a stale copy cannot resurrect it. `adapt.ts` is the single seam that drops
/// them, and it is deliberately in the webview — see `cide_tasks::detail_of`.
///
/// **No `rev` and no schema of its own.** Both would be a second ordering counter over one logical
/// document, and `emit::tasks_changed`'s doc spends a paragraph on why that is a trap. The index
/// carries the version for the whole tracker; a content file is dated by its own mtime, which is
/// what `cide_tasks` compares before it writes one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskContent {
    /// The whole statement of the work, possibly empty — [`Task::body`]'s posture verbatim.
    ///
    /// `#[serde(default)]`, unlike `Task::body`, and the difference is the point: a `Task` with no
    /// `body` key is a truncated document, while a content file with none is a task nobody has
    /// written a description for. Here the absence is a fact about the task, which is exactly the
    /// test [`TaskAuthor::user`]'s doc sets for when a default is honest.
    #[serde(default)]
    pub body: String,
    /// Oldest first, which is the order the panel renders and an agent reads.
    #[serde(default)]
    pub comments: Vec<TaskComment>,
    /// Every status transition, oldest first. (M27)
    #[serde(default)]
    pub history: Vec<TaskStatusChange>,
    /// The body's files. Records only; the bytes are at [`TaskAttachment::relative_path`]. (M39)
    #[serde(default)]
    pub attachments: Vec<TaskAttachment>,
}

/// The index: the whole of `.cide/tasks.json`.
///
/// `{ "schemaVersion": 2, "rev": 42, "nextId": 18, "tasks": [ … ] }`, written with
/// `to_vec_pretty`, because a one-line JSON file makes every change a whole-file diff and this
/// file's diffs are read by people.
///
/// **Rows since M68**, not whole tasks — [`TaskContent`] has the argument, and `schemaVersion` went
/// to 2 with it. Schema 1 was every task inline here; `cide_tasks::read` migrates such a file and
/// nothing writes one again.
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
    /// The number the next created task takes: `t-<next_id>`. **Only a create advances it.**
    ///
    /// Until M61 there was no such field and the next id was `max(largest t-<n>, rev) + 1`. The
    /// `rev` half was there so that deleting the newest task could not lower the mark — a reused
    /// id silently re-points every comment, prompt and commit message that named the first one,
    /// and nothing anywhere can detect it. But `rev` moves on *every* accepted mutation, so each
    /// comment, assignment and status change between two creates spent a number, and a board
    /// with agents on it went `t-772`, `t-774`, `t-776`, `t-780` — the "gap after a delete" that
    /// rule's doc admitted to was in practice a gap after nearly everything. This counter keeps
    /// the two facts apart: `rev` says how many times the file changed, this says how many tasks
    /// were ever minted, and a delete lowers neither.
    ///
    /// **Zero means "not yet computed"** and is what a file written before the field existed
    /// deserialises to; this build never writes it, and `cide_tasks::repair` settles such a file
    /// under the old rule *once* — past `rev` as well as past every id, because an id deleted
    /// under the old rule is not in the file to be seen. So an existing board takes one last jump
    /// and is contiguous from there. A value behind the largest id in the file (a hand edit, a
    /// file copied from another board) is raised past it by the same repair, and `validate`
    /// refuses a file where it is not. The out-of-process merge takes the higher of the two sides'.
    #[serde(default)]
    pub next_id: u64,
    /// **A list, not a map**, and the order is meaningful.
    ///
    /// Two reasons, and the first is about people: this file is read in pull requests, where a
    /// list has an order a human can follow top to bottom and a map has whatever order the
    /// serialiser felt like. The second is about the shape: a map would put each id in two
    /// places — the key and the value's `id` — and the first hand edit that disagreed with
    /// itself would be a task with two identities.
    ///
    /// There is deliberately **no `order` field**: a stored rank is a second source of truth
    /// that a hand-edited or merged file can contradict, and then something has to decide which
    /// of the two orderings wins. The array is the *file's* order — what a pull request reads
    /// top to bottom, where appends land. The Tasks panel does not draw it verbatim any more:
    /// each status group is drawn by `updated_unix_ms`, newest first, with the array as the
    /// tie-break (`groups` in `ui/src/sidebar/TasksPanel/model.ts` carries the argument). That
    /// is a reading order derived from stamps this struct already carries, not a second stored
    /// one, which is why it does not reopen the argument above.
    pub tasks: Vec<TaskRow>,
}

impl TaskFile {
    /// The shape this build writes.
    ///
    /// **2 since M68**: the index plus one `.cide/tasks/<id>/task.json` per task. 1 was the whole
    /// tracker in this one file; `cide_tasks::read` migrates it, and nothing writes a 1 again.
    pub const CURRENT_SCHEMA: u32 = 2;

    /// The first schema this build can still *read*.
    ///
    /// Named rather than spelled inside the ladder so that dropping support for a version is a
    /// visible edit to a constant with a doc comment, instead of the quiet deletion of a match arm.
    /// A tracker is a file in somebody's repository and a build that refused last year's is the
    /// tracker locking a team out of its own history.
    pub const OLDEST_READABLE_SCHEMA: u32 = 1;
}

impl Default for TaskFile {
    fn default() -> Self {
        Self {
            schema_version: Self::CURRENT_SCHEMA,
            rev: 0,
            next_id: 1,
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
    ///
    /// **Rows, not whole tasks** (M68) — see [`TaskRow`] for the measured reason. A body, a log
    /// and a task's files are fetched by id, once something opens one.
    ///
    /// `rev` must stay on this arm. The receiving store's drop rule is
    /// `Number.isFinite(next.rev) && next.rev > current.rev` (`newerBoard`, in
    /// `ui/src/sidebar/TasksPanel/model.ts`), so a `rev` that went missing would make
    /// `Number.isFinite(undefined)` false, every snapshot would be judged not-newer, and the panel
    /// would freeze on whatever board it happened to hold — for the rest of the session, with no
    /// type error anywhere and nothing logged.
    Ready { tasks: Vec<TaskRow>, rev: u64 },
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
    /// The OpenSpec change this task implements, if it is known at creation. (M28)
    ///
    /// Set at creation and not in a follow-up edit, which matters more than it looks: the
    /// auto-dispatch trigger reads the task the mutation *left behind*, so a create-then-link
    /// would dispatch a run from a task that did not yet name its change, and the run would be
    /// told nothing about the checklist it was started for.
    #[ts(optional)]
    pub change: Option<ChangeName>,
    /// Edges known at creation. Absent means none. (M30)
    ///
    /// Here for [`Self::change`]'s exact reason, with a sharper edge: a creation naming an
    /// assignee dispatches, and the trigger reads the task the mutation left behind — so
    /// create-then-link would dispatch a run from a task whose `blockedBy` did not exist yet,
    /// and the one interleaving the gate exists for is the one it could never see.
    #[ts(optional)]
    pub links: Option<Vec<TaskLinkSpec>>,
    /// Files to attach to the body the moment the task exists. Absent means none. (M39)
    ///
    /// **Source paths**, on this machine, which `cide_tasks::attachments::import` copies under
    /// `.cide/attachments/<the new id>/`; a path under its staging directory is consumed. Here
    /// rather than as a follow-up `task_attach` for [`Self::links`]' reason in miniature: the
    /// board is broadcast once with the attachments in place instead of once without and once
    /// with, and a dropped file in the New task dialog is one gesture, not two.
    #[ts(optional)]
    pub attachments: Option<Vec<PathBuf>>,
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
    /// Hand this task's work to a live Claude session, or take it back. (M28)
    ///
    /// **Applying this clears [`Task::agent`]**, and `Assign` clears [`Task::session`] — the two
    /// are one fact with two shapes, and `TaskStore::edit` is where that invariant lives. A task
    /// claiming both a role and a session would be a card that cannot say who is working on it.
    ///
    /// Unlike `Assign`, this is **not** a dispatch gesture: nothing downstream reads it and
    /// starts anything. See [`Task::session`] for why that is the point.
    SetSession {
        session: Option<SessionId>,
    },
    /// Link this task to an OpenSpec change, or unlink it. (M28)
    ///
    /// `change: null` **unlinks**, which is [`Self::Assign`]'s shape and is here for the same
    /// reason: absent and null are different instructions, and an enum arm is how the difference
    /// survives the wire. A task whose change was proposed and then abandoned must be able to go
    /// back to being an ordinary task without being deleted and retyped.
    SetChange {
        change: Option<ChangeName>,
    },
    /// Add one edge to another task. (M30)
    ///
    /// A pair of variants rather than a `SetLinks` array, and the header's patch-struct argument
    /// is why: an array field can say what the set *is* but not what the caller *did* — absent,
    /// null and `[]` between them cannot spell add-versus-remove, so every caller would have to
    /// read-modify-write the whole set and two concurrent adds would erase each other at the
    /// seam before the merge ever saw them.
    ///
    /// Refused for a self-link, a live duplicate, a target that does not exist, and a
    /// `blockedBy`/`subtaskOf` edge that would close a cycle — each refusal is a sentence at the
    /// gesture; `cide_tasks::TaskStore::edit` owns them.
    Link {
        link: LinkType,
        target: TaskId,
    },
    /// Tombstone one edge. (M30)
    ///
    /// Named by the same `(link, target)` pair `cide_task_get` shows. For [`LinkType::Related`]
    /// either end may unlink — the edge lives on whichever task made it, and the store looks on
    /// both.
    Unlink {
        link: LinkType,
        target: TaskId,
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
    /// Tombstone one attachment, on the body or on any comment, and remove its bytes. **The user
    /// only.** (M39)
    ///
    /// The one attachment gesture that *is* an edit: a tombstone flip fits `TaskStore::update`'s
    /// closure exactly as `DeleteComment` does. Attaching is not a variant, because it has to
    /// copy bytes to disk *before* a record naming them may exist, and that is I/O the closure
    /// must not do — `TaskStore::attach` is its own method. The file removal happens after the
    /// tombstone lands and is best effort: the record is the truth, a leftover directory is
    /// hygiene.
    DetachAttachment {
        attachment: TaskAttachmentId,
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
            change: None,
            links: vec![TaskLink {
                link: LinkType::BlockedBy,
                target: TaskId("t-3".into()),
                deleted: false,
                at_unix_ms: 1_700_000_000_000,
            }],
            session: None,
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
                attachments: vec![],
            }],
            attachments: vec![TaskAttachment {
                id: TaskAttachmentId("a-1".into()),
                name: "design-v3.png".into(),
                bytes: 12_345,
                kind: AttachmentKind::Image,
                added_by: TaskAuthor::User,
                added_unix_ms: 1_700_000_000_000,
                deleted: false,
            }],
            history: vec![TaskStatusChange {
                from: TaskStatus::Todo,
                to: TaskStatus::Doing,
                by: TaskAuthor::User,
                at_unix_ms: 1_700_000_000_000,
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
        // would read `undefined` in the webview. `blockedBy` pins the LinkType spelling the
        // MCP tools and the TypeScript union both repeat.
        for wire in [
            "createdBy",
            "createdUnixMs",
            "updatedUnixMs",
            "atUnixMs",
            "blockedBy",
            "addedBy",
            "addedUnixMs",
        ] {
            assert!(json.contains(wire), "missing {wire} in {json}");
        }
        assert_eq!(serde_json::from_str::<Task>(&json).unwrap(), a_task());
    }

    /// A task from a file written before `Task::links` existed still parses, with no links. (M30)
    ///
    /// The same claim `a_task_written_before_the_creator_field_still_parses` pins for M21's
    /// field, and it has to be re-made per field because `#[serde(default)]` is one attribute a
    /// refactor can drop with no compile error.
    #[test]
    fn a_task_written_before_links_existed_still_parses() {
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
        assert!(task.links.is_empty());
    }

    #[test]
    fn a_link_edit_names_its_kind_and_target() {
        // The two arms an MCP tool fills in: the wire spelling is the serde camelCase one, and
        // an unlink is not an absent field — it is its own instruction, `assign_can_say_nobody`'s
        // point restated for edges.
        let edit: TaskEdit =
            serde_json::from_str(r#"{"kind":"link","link":"blockedBy","target":"t-3"}"#).unwrap();
        assert_eq!(
            edit,
            TaskEdit::Link {
                link: LinkType::BlockedBy,
                target: TaskId("t-3".into()),
            }
        );
        let edit: TaskEdit =
            serde_json::from_str(r#"{"kind":"unlink","link":"related","target":"t-9"}"#).unwrap();
        assert_eq!(
            edit,
            TaskEdit::Unlink {
                link: LinkType::Related,
                target: TaskId("t-9".into()),
            }
        );
    }

    #[test]
    fn an_inbound_link_spec_refuses_a_field_it_does_not_know() {
        // A caller that sends a stamp or a tombstone is a caller trying to write the store's
        // fields; `deny_unknown_fields` makes that a parse error rather than a silent drop.
        let err = serde_json::from_str::<TaskLinkSpec>(
            r#"{"link":"related","target":"t-2","atUnixMs":5}"#,
        );
        assert!(err.is_err(), "a store-owned field was accepted inbound");
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

    /// A task from a file written before attachments existed still parses, with none. (M39)
    ///
    /// Re-made per field, as `a_task_written_before_links_existed_still_parses` says: the
    /// `#[serde(default)]` is one attribute a refactor can drop with no compile error, and here
    /// it guards two places — the task and every comment.
    #[test]
    fn a_task_written_before_attachments_existed_still_parses() {
        let json = r#"{
            "id": "t-3",
            "title": "Write the loader",
            "body": "",
            "status": "todo",
            "agent": null,
            "comments": [{
                "author": {"kind": "user"},
                "text": "first",
                "atUnixMs": 1700000000000
            }],
            "createdUnixMs": 1699999999000,
            "updatedUnixMs": 1700000000000
        }"#;
        let task: Task =
            serde_json::from_str(json).expect("a file this build has to be able to open");
        assert!(task.attachments.is_empty());
        assert!(task.comments[0].attachments.is_empty());
    }

    /// The attachment's path is the one formula both crates print and write. (M39)
    #[test]
    fn an_attachment_lives_under_its_task_and_its_own_id() {
        let task = a_task();
        let path = task.attachments[0].relative_path(&task.id);
        assert_eq!(
            path,
            PathBuf::from(".cide/tasks/t-17/attachments/a-1/design-v3.png"),
            "under the task's own directory since M68, so `git rm -r .cide/tasks/t-17` takes a \
             task's description, its conversation and its files in one gesture"
        );
        assert_eq!(
            task.attachments[0].legacy_relative_path(&task.id),
            PathBuf::from(".cide/attachments/t-17/a-1/design-v3.png"),
            "and the pre-M68 spelling is still computable, because a relocation that was \
             interrupted leaves files there and a reader that could not name them would report \
             every one of them as missing"
        );
        assert!(
            path.is_relative(),
            "joined onto a root by whoever holds one"
        );
    }

    /// The three targets are three `kind`s, and a detach is an edit. (M39)
    #[test]
    fn an_attach_target_names_where_the_file_goes() {
        let task: AttachTarget = serde_json::from_str(r#"{"kind":"task"}"#).unwrap();
        assert_eq!(task, AttachTarget::Task);
        let comment: AttachTarget =
            serde_json::from_str(r#"{"kind":"comment","id":"c-1"}"#).unwrap();
        assert_eq!(
            comment,
            AttachTarget::Comment {
                id: CommentId("c-1".into())
            }
        );
        let fresh: AttachTarget =
            serde_json::from_str(r#"{"kind":"newComment","text":"with a screenshot"}"#).unwrap();
        assert_eq!(
            fresh,
            AttachTarget::NewComment {
                text: "with a screenshot".into()
            }
        );
        // A comment target with no `id` is refused, not narrowed to the body — the two are
        // different files landing in different places.
        assert!(serde_json::from_str::<AttachTarget>(r#"{"kind":"comment"}"#).is_err());

        let edit: TaskEdit =
            serde_json::from_str(r#"{"kind":"detachAttachment","attachment":"a-1"}"#).unwrap();
        assert_eq!(
            edit,
            TaskEdit::DetachAttachment {
                attachment: TaskAttachmentId("a-1".into())
            }
        );
    }
}
