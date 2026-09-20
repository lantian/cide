//! `.cide/tasks.json` — the shared task tracker, and the only thing in the process allowed to
//! write it. (M18)
//!
//! # One owning actor, and why that is what makes a single JSON file safe
//!
//! The tracker is read and written by the project's primary Claude session, by every subagent it
//! dispatches, and by the user through the `Tasks` sidebar panel. Three classes of writer on one
//! file is normally a recipe for lost updates — and it is not one here for exactly one reason:
//! **agents never write the file.** They call `cide_task_*` MCP tools, those arrive over the
//! agent-RPC socket, the app funnels them into [`TaskStore::update`], and so every mutation in
//! the system passes through one `Mutex` in one process.
//!
//! That is not an implementation detail, it is the design. The MCP-tool decision and the
//! single-file decision are the same decision viewed from two sides: the moment an agent could
//! `Edit .cide/tasks.json` directly, this module would need real file locking — an advisory
//! `flock` the rest of the world does not honour, or a lock file with a stale-owner problem — and
//! the merge in [`merge`] would stop being a rare repair and become the hot path. Two things keep
//! that from happening, and both live outside this crate: the agent prompt preamble, and a
//! `PreToolUse` hook that denies an `Edit`/`Write` resolving to this path.
//!
//! # The four layers
//!
//! 1. **A single owning actor** — [`TaskStore`], mirroring `cide_app::WorkspaceState` line for
//!    line, so that a reviewer who knows one knows the other: snapshot, run the closure, validate,
//!    roll back on `Err` *or* on a failed validation, bump `rev`, `note_change`.
//! 2. **A loader that repairs and never fails** — `load`, following `cide_core::persist::load`'s
//!    discipline. A broken task file must not become a project that cannot open.
//! 3. **Out-of-process writes** — a `git pull`, a teammate's commit or a hand edit moves the file
//!    under us. [`TaskStore::write_now`] re-`stat`s before every write and, when the stamp has
//!    moved, re-reads and merges by rule ([`merge`]). This is the layer a single actor cannot
//!    solve and the one most likely to hold a bug, which is why the merge is a pure function over
//!    two [`TaskFile`]s with a table-driven test rather than something tangled into the store.
//! 4. **Debounce and atomic write** — `Debouncer::new(persist::SAVE_DEBOUNCE)` and
//!    `persist::write_atomic_with_mode`: literally the publish `workspace.json` gets, differing
//!    only in the file mode it is asked for (see [`write_shared`]).
//!
//! # What this crate deliberately does not do
//!
//! It does not broadcast. `WorkspaceState::update` emits `cide://workspace-changed` while still
//! holding the lock, so two concurrent mutations cannot reach a window in the opposite order to
//! the one they were applied in; that costs an `AppHandle`, and an `AppHandle` in a domain crate
//! is the signal that the logic is in the wrong crate. The app calls [`TaskStore::snapshot`] or
//! [`TaskStore::board`] after a mutation and emits from there. Losing the under-the-lock ordering
//! is affordable *here* and not there, because [`TaskFile::rev`] is monotone and the frontend
//! store drops any snapshot whose `rev` is not newer — an out-of-order pair costs one dropped
//! broadcast, not a wrong tree.
//!
//! It also owns no thread. Layer 4's ticking is the app's job (`PositionsState::start_flusher` is
//! the model), for the same reason `cide_core::persist` owns none: a timer here would mean an
//! async runtime in a domain crate.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cide_core::persist::{self, Debouncer};
use cide_core::{CoreError, Result, document};
use cide_ipc::{
    AttachTarget, ChangeName, CommentId, FileStamp, LinkType, Task, TaskAttachment,
    TaskAttachmentId, TaskAuthor, TaskBoard, TaskComment, TaskEdit, TaskFile, TaskId, TaskLink,
    TaskLinkSpec, TaskNew, TaskStatusChange,
};
use parking_lot::Mutex;
use serde_json::Value;

pub mod attachments;

/// Where the tracker lives inside a project, relative to its first root.
///
/// A constant rather than three string literals, and under `.cide/` because that is the directory
/// this feature already owns — `cide_git::worktree` puts `.cide/worktrees/<agent>` beside it. Note
/// that `cide_fs::filter::Filter::admits` rejects any dot-prefixed component *before* any gitignore
/// matcher runs, so this file is invisible to cide's own tree and watcher until that filter grows
/// a seam for it; nothing in this crate depends on the watcher, and that is deliberate.
pub const TASKS_RELATIVE: &str = ".cide/tasks.json";

/// `<root>/.cide/tasks.json`.
pub fn tasks_path(project_root: &Path) -> PathBuf {
    project_root.join(TASKS_RELATIVE)
}

// --- pure functions over the DTOs -----------------------------------------------------------
//
// `cide-core`'s idiom: behaviour over `cide-ipc`'s types as free functions rather than inherent
// impls. Every one of these is testable without a disk, which is the whole reason the merge in
// particular is shaped this way.

/// Whether an id is one this build is willing to carry.
///
/// **Deliberately wider than the `t-<n>` this crate mints.** The file is committed and
/// hand-editable, and a teammate who writes `id: "spike"` into a task by hand has not corrupted
/// anything — refusing to open their project over the shape of a string would be the app claiming
/// ownership of a file that belongs to the team.
///
/// What it does refuse is an id that cannot survive the round trip that ids exist for: agents
/// **quote task ids inside prompts and comments**, so an id containing a space, a quote, a comma
/// or a newline is one a model will silently re-punctuate and nothing will ever match back. ASCII
/// alphanumerics plus `-` and `_` is the set that survives being read out of prose.
pub fn well_formed_id(id: &TaskId) -> bool {
    let text = id.as_str();
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The invariants a [`TaskFile`] must hold for this build to write it.
///
/// Called by [`TaskStore::update`] *after* the mutation, exactly as `workspace::validate` is by
/// `WorkspaceState::update`, so that a compound edit which leaves the file inconsistent is rolled
/// back rather than persisted.
///
/// The rules are few on purpose. Each one is here because something downstream breaks without it:
///
/// * **No duplicate ids** — the id is the identity. Two tasks sharing one means every comment,
///   every prompt and every `cide_task_get` addresses whichever the iteration order happened to
///   reach first, and the answer changes when the list is reordered.
/// * **No empty title** — the panel draws the title and an agent quotes it. A task with no title
///   is a row nobody can act on, which `TaskNew::title`'s own doc says in as many words.
/// * **Every id well-formed** — see [`well_formed_id`].
/// * **The schema is this build's** — a mutation must never write a `schemaVersion` this build
///   does not own. [`read`] refuses a future schema before it ever reaches memory, so reaching
///   here means a bug in this crate rather than a file on disk; it is cheap to assert and the
///   failure it prevents (silently downgrading a teammate's file on the next flush) is total.
/// * **Every link target well-formed, no self-links, one entry per `(link, target)`** — the
///   first cross-task rules this function has ever had, and each is downstream-breaking in the
///   established way: an unquotable target is a reference nothing can match back, a self-link
///   would gate a task on itself, and two entries under one key would make [`union_links`]'
///   per-key reconciliation ambiguous (the duplicate-task-id argument, one level down —
///   tombstoned entries count, because they carry the key too).
/// * **`next_id` is past every numeric id** — [`TaskFile::next_id`] is what keeps a deleted id
///   from coming back, and it can only do that while it is ahead of every id it ever handed
///   out. Only a hand edit or a bug in this crate's own [`mint_id`], [`merge`] or [`repair`]
///   can put it behind (`settle_next_id` raises it on every read), and the failure that bug
///   would cause is the silent one: the next create fills a gap that may be a deleted task.
///
/// Two link states are **deliberately legal** here, refused only as gestures in
/// [`TaskStore::edit`]:
///
/// * **A dangling target.** [`TaskStore::delete`] scrubs nothing (a scrub could not survive
///   [`merge`], which has no task tombstones and would re-adopt the stale side's edge), and ids
///   are never reused, so a dangling edge can never come to mean new work. Renderers mark it;
///   the dispatch gate treats a dangling blocker as satisfied, because a deleted task can never
///   become done and an edge that gated for ever would make deletion corrupt every task that
///   pointed at the deleted one.
/// * **A `blockedBy`/`subtaskOf` cycle.** [`merge`] can lawfully union two acyclic sides into a
///   cycle (each side added one arc), and [`TaskStore::reconcile`] swaps merge output in without
///   re-validating — a rule here that a merge can break would freeze every subsequent mutation,
///   which is the exact failure [`repair`]'s doc exists to prevent. Nothing traverses links
///   transitively at read time (the dispatch gate is one hop), so a cyclic file wedges nothing:
///   the tasks merely stand mutually undispatchable until a status edit — always allowed —
///   breaks the standoff.
pub fn validate(file: &TaskFile) -> Result<()> {
    if file.schema_version != TaskFile::CURRENT_SCHEMA {
        return Err(CoreError::Invariant(format!(
            "task file schema {} is not this build's {}",
            file.schema_version,
            TaskFile::CURRENT_SCHEMA
        )));
    }

    let mut seen: Vec<&str> = Vec::with_capacity(file.tasks.len());
    for task in &file.tasks {
        if !well_formed_id(&task.id) {
            return Err(CoreError::Invariant(format!(
                "task id {:?} is not usable as an identifier",
                task.id.as_str()
            )));
        }
        if task.title.trim().is_empty() {
            return Err(CoreError::Invariant(format!(
                "task {} has an empty title",
                task.id
            )));
        }
        if seen.contains(&task.id.as_str()) {
            return Err(CoreError::Invariant(format!(
                "duplicate task id {}",
                task.id
            )));
        }
        seen.push(task.id.as_str());
        if let Some(n) = task
            .id
            .as_str()
            .strip_prefix("t-")
            .and_then(|n| n.parse::<u64>().ok())
            && n >= file.next_id
        {
            return Err(CoreError::Invariant(format!(
                "task {} is not below the file's next id {}",
                task.id, file.next_id
            )));
        }

        for (i, entry) in task.links.iter().enumerate() {
            if !well_formed_id(&entry.target) {
                return Err(CoreError::Invariant(format!(
                    "task {} links to {:?}, which is not usable as an identifier",
                    task.id,
                    entry.target.as_str()
                )));
            }
            if entry.target == task.id {
                return Err(CoreError::Invariant(format!(
                    "task {} links to itself",
                    task.id
                )));
            }
            if task.links[..i]
                .iter()
                .any(|other| other.link == entry.link && other.target == entry.target)
            {
                return Err(CoreError::Invariant(format!(
                    "task {} carries two {:?} links to {}",
                    task.id, entry.link, entry.target
                )));
            }
        }

        // Attachment ids are unique **within the task, across the body and every comment**:
        // an id is a path component under the task's own directory, so two records sharing
        // one would alias two files onto one path — a stronger claim than a duplicate comment
        // id, which only breaks a merge key. Tombstoned records count, as with links. (M39)
        let mut seen_attachments: Vec<&str> = Vec::new();
        for attachment in task
            .attachments
            .iter()
            .chain(task.comments.iter().flat_map(|c| c.attachments.iter()))
        {
            if !well_formed_attachment_id(&attachment.id) {
                return Err(CoreError::Invariant(format!(
                    "task {} carries an attachment whose id {:?} is not usable as a path",
                    task.id,
                    attachment.id.as_str()
                )));
            }
            if !well_formed_attachment_name(&attachment.name) {
                return Err(CoreError::Invariant(format!(
                    "task {} carries an attachment whose name {:?} is not one path component",
                    task.id, attachment.name
                )));
            }
            if seen_attachments.contains(&attachment.id.as_str()) {
                return Err(CoreError::Invariant(format!(
                    "task {} carries two attachments with the id {}",
                    task.id, attachment.id
                )));
            }
            seen_attachments.push(attachment.id.as_str());
        }
    }
    Ok(())
}

/// Whether an attachment id can be the directory it names. (M39)
///
/// [`well_formed_id`]'s alphabet — a uuid is inside it — with the same reason one level down:
/// this string becomes a path component, and a separator or a `..` in it would put the bytes
/// somewhere other than under the task.
fn well_formed_attachment_id(id: &TaskAttachmentId) -> bool {
    let text = id.as_str();
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether an attachment name is exactly one path component — what
/// [`attachments::sanitize_name`] produces, restated as a check because the file is hand-editable.
fn well_formed_attachment_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
        && name.trim() == name
}

/// The line [`TaskStore::create`] used to open an agent-made task's log with, before
/// [`Task::created_by`] existed. (M21)
///
/// A literal rather than a pattern, because it was a literal: every file that carries it was
/// written by a build that wrote exactly this string, and matching loosely here could only ever
/// adopt a *real* comment as a creation record.
const LEGACY_CREATE_NOTE: &str = "created this task";

/// Make a file that parsed satisfy [`validate`], destroying as little as possible, loudly.
///
/// # Why repair at all, rather than refuse
///
/// `WorkspaceState::load` answers a workspace that fails validation by discarding it and starting
/// from defaults — a layout is recreatable, so that trade is free. **This file is the user's data
/// and is committed**, so discarding it is not on the table. But leaving it invalid is not either:
/// [`TaskStore::update`] validates and rolls back, so a single hand-edited empty title would make
/// *every subsequent mutation* fail, silently, for ever — a tracker that refuses every edit with
/// nothing on screen saying why is a worse outcome than a visible, logged repair.
///
/// So the invariant is: whatever [`read`] hands back has already been through here, and therefore
/// a validation failure inside `update` can only ever be caused by *that* mutation.
///
/// # What each repair costs, and the option that lost
///
/// * **A duplicate id keeps the first and drops the rest.** The instructed rule, and the only one
///   available: the later copy has no identity of its own to be given (renaming it would silently
///   re-point every comment and prompt that named the id), and keeping both is the state the
///   invariant exists to forbid. The dropped bytes are still in `git log -p`.
/// * **A malformed id is re-minted.** Rewriting an id normally breaks every reference to it. The
///   *prose* references were never a cost — "malformed" means precisely "cannot be quoted and
///   matched back", so no comment or prompt held a usable copy — and the structured references
///   [`Task::links`] added (M30) are re-pointed rather than broken: the re-mint pass records every
///   `was → now` and a second pass rewrites link targets through that map, which is a repair only
///   possible *because* the reference is a field and not prose. Dropping the task instead would
///   lose work over a typo.
/// * **An empty title becomes `(untitled)`.** The option that lost was dropping the task, which
///   loses work; the other was leaving it, which freezes the tracker as described above. The
///   placeholder is visible in the panel and in the next diff, which is the point.
/// * **A link that cannot be made valid is dropped.** After re-pointing: a target still
///   malformed, a self-link, or a second entry under one `(link, target)` key (the newest stamp
///   is the one kept — it is the copy [`union_links`] would have chosen). Dropped and warned, not
///   refused, for the header's reason: these arrive by hand edit or merge, and each would
///   otherwise freeze the tracker. A *dangling* target is not in this list — it is legal, see
///   [`validate`].
fn repair(file: &mut TaskFile) {
    // Settled first, so that a re-mint below takes a number past every id in the file — including
    // one further down the list that has not been visited yet — and so that a file written before
    // the counter existed has one at all.
    settle_next_id(file);

    // Every re-mint below, `was → now`, applied to link targets in a second pass: a reference
    // that is a field can follow the rename that a reference in prose never could.
    let mut reminted: Vec<(TaskId, TaskId)> = Vec::new();

    let mut kept: Vec<Task> = Vec::with_capacity(file.tasks.len());
    for mut task in std::mem::take(&mut file.tasks) {
        if !well_formed_id(&task.id) {
            // A re-mint is a mint: it spends the counter, or the next create would hand out the
            // id this task just took.
            let minted = TaskId(format!("t-{}", file.next_id));
            file.next_id += 1;
            tracing::warn!(
                was = %task.id,
                now = %minted,
                "task id was not usable as an identifier; re-minted it"
            );
            reminted.push((task.id.clone(), minted.clone()));
            task.id = minted;
        }
        if kept.iter().any(|k| k.id == task.id) {
            tracing::warn!(id = %task.id, title = %task.title, "duplicate task id; keeping the first");
            continue;
        }
        if task.title.trim().is_empty() {
            tracing::warn!(id = %task.id, "task has no title; showing it as (untitled)");
            task.title = "(untitled)".to_string();
        }
        /*
         * Every comment leaves this function with an id. (M21)
         *
         * A file written before `TaskComment::id` existed has none, and `#[serde(default)]` gives
         * the empty string. An empty id must never reach `union_comments`, which merges *by* id:
         * every comment in the file would be "the same comment" and the merge would collapse the
         * whole log to one line. Repair is the seam that guarantees it cannot happen, which is
         * the job this function already has for task ids one field up.
         *
         * Derived and not minted. `legacy_id` is a pure function of the old merge key, so two
         * readers of the same file agree without either of them writing — and the file is not
         * rewritten just for having been read by a newer build. A minted uuid here would be a
         * different id per reader, which is a merge that duplicates every comment it touches:
         * exactly the bug ids were added to prevent, introduced by the fix for it.
         */
        for comment in &mut task.comments {
            if comment.id.is_empty() {
                comment.id =
                    TaskComment::legacy_id(&comment.author, comment.at_unix_ms, &comment.text);
            }
        }
        /*
         * An attachment record that cannot stand is **dropped, never re-minted.** (M39)
         *
         * The task and comment passes above re-mint or derive an id, because a task id or a
         * comment id is only a key. An attachment id is a *directory name*: the bytes are filed
         * under it. Re-minting the record would leave the file under the old name — an orphan
         * with no symptom but a thumbnail that never loads, since `task_attachment_image` would
         * look under the new one. Dropping the record leaves the bytes where they are for a
         * person to find with `ls`, and says so in the log.
         */
        {
            let task_id = task.id.clone();
            let mut seen: Vec<String> = Vec::new();
            let mut keep = |list: &mut Vec<TaskAttachment>| {
                list.retain(|attachment| {
                    if !well_formed_attachment_id(&attachment.id)
                        || !well_formed_attachment_name(&attachment.name)
                    {
                        tracing::warn!(
                            id = %task_id,
                            attachment = %attachment.id,
                            name = %attachment.name,
                            "attachment record is not usable as a path; dropped it (its bytes, if any, are left on disk)"
                        );
                        return false;
                    }
                    if seen.contains(&attachment.id.0) {
                        tracing::warn!(id = %task_id, attachment = %attachment.id, "duplicate attachment id; keeping the first");
                        return false;
                    }
                    seen.push(attachment.id.0.clone());
                    true
                });
            };
            keep(&mut task.attachments);
            for comment in &mut task.comments {
                keep(&mut comment.attachments);
            }
        }
        /*
         * The creator of a task written before `Task::created_by` existed. (M21)
         *
         * That build recorded the answer by *seeding the log*: an agent-made task opened with one
         * comment saying `created this task`, stamped at the same millisecond as the task, and a
         * user-made one opened with nothing. So the fact is still in the file, and this reads it
         * back rather than letting every such task decay to `User` — which is what the serde
         * default gives, and which would credit the user with six subagents' work.
         *
         * Three conditions, all three needed. `User` on the left means *either* a file that had no
         * field or a task genuinely created by the user, and the seeding rule says those two are
         * the same set — a task this build wrote with a real creator is never overwritten. The
         * text and the timestamp together are what stop an ordinary comment being adopted: an
         * agent would have to write those exact three words in the millisecond the task was
         * created, which is the same shape of coincidence `TaskComment::legacy_id` already accepts.
         *
         * Derived rather than defaulted, and derived *in memory*: opening a project does not
         * rewrite the file, so a newer build reading a teammate's tracker leaves no diff. The next
         * real mutation persists `created_by` along with everything else, and from then on this
         * finds nothing to do.
         */
        if task.created_by == TaskAuthor::User
            && let Some(seed) = task.comments.first()
            && seed.author != TaskAuthor::User
            && seed.text == LEGACY_CREATE_NOTE
            && seed.at_unix_ms == task.created_unix_ms
        {
            task.created_by = seed.author.clone();
        }
        kept.push(task);
    }

    // The link pass, after the id pass so the re-mint map is complete. Order inside matters:
    // re-point first, then judge — a link whose target was just re-minted is a link the map
    // saves, not one the malformed-target rule drops.
    for task in &mut kept {
        for entry in &mut task.links {
            if let Some((_, now)) = reminted.iter().find(|(was, _)| *was == entry.target) {
                tracing::warn!(
                    id = %task.id,
                    was = %entry.target,
                    now = %now,
                    "link target was re-minted; re-pointed the link"
                );
                entry.target = now.clone();
            }
        }
        let id = task.id.clone();
        let mut seen_keys: Vec<(LinkType, TaskId)> = Vec::new();
        // Newest-stamp-first so the duplicate collapse below keeps the copy `union_links`
        // would have chosen; the file's own order for links carries no meaning (renderers
        // group by kind), so re-sorting here costs nothing a reader can see.
        task.links
            .sort_by_key(|entry| std::cmp::Reverse(entry.at_unix_ms));
        task.links.retain(|entry| {
            if !well_formed_id(&entry.target) {
                tracing::warn!(id = %id, target = %entry.target, "link target is not usable as an identifier; dropped the link");
                return false;
            }
            if entry.target == id {
                tracing::warn!(id = %id, "task linked to itself; dropped the link");
                return false;
            }
            let key = (entry.link, entry.target.clone());
            if seen_keys.contains(&key) {
                tracing::warn!(id = %id, target = %entry.target, "duplicate link; keeping the newest");
                return false;
            }
            seen_keys.push(key);
            true
        });
        task.links.sort_by_key(|entry| entry.at_unix_ms);
    }

    file.tasks = kept;
}

/// The largest `n` across every `t-<n>` in the file.
///
/// Non-numeric ids (a hand-written `spike`) contribute nothing, and `t-0007` contributes 7 while
/// being a different string from `t-7` — which is why [`mint_id`] still has to check that its
/// candidate is actually free.
fn largest_numeric_id(file: &TaskFile) -> u64 {
    file.tasks
        .iter()
        .filter_map(|task| task.id.as_str().strip_prefix("t-"))
        .filter_map(|n| n.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
}

/// Give the file a usable [`TaskFile::next_id`], whatever it arrived with. Part of [`repair`].
///
/// Two cases, and the first is the migration. **A file with no counter** — `0`, which is what
/// every file written before M61 deserialises to — is settled under the rule those files were
/// minted under: `max(largest id, rev) + 1`. Not `largest + 1`, though that is what a fresh file
/// gets: under the old rule a delete never lowered the mark *because* `rev` was in it, so the
/// numbers between the largest surviving id and `rev` may name tasks that were deleted and are
/// still quoted in comments and commit messages — and the file cannot say which. The one safe
/// floor is the old one. The cost is a single last jump, past a `rev` that comments have been
/// advancing for as long as the board has existed, after which the counter moves only on a
/// create.
///
/// **A counter behind the largest id** is a hand edit or a file copied from another board, and
/// is raised past it. A mint from below would not duplicate — [`mint_id`] checks — but it would
/// fill a gap, and a gap may be a deleted task for exactly the reason above.
fn settle_next_id(file: &mut TaskFile) {
    let floor = largest_numeric_id(file) + 1;
    if file.next_id == 0 {
        file.next_id = floor.max(file.rev + 1);
        tracing::info!(
            next_id = file.next_id,
            "task file had no id counter; settled it past every id and past rev"
        );
    } else if file.next_id < floor {
        tracing::warn!(
            was = file.next_id,
            now = floor,
            "task file's id counter was behind its largest id; raised it"
        );
        file.next_id = floor;
    }
}

/// Mint the next `t-<n>` and move the counter past it.
///
/// The loop is not paranoia: the counter is only guaranteed past every id that *parses* as
/// `t-<digits>`, and a hand-edited file can perfectly legally contain `t-0007` — which parses as
/// 7 but is a different string, so a mint of `t-7` would be a genuine duplicate rather than a
/// shadow. The counter lands one past whatever was actually used, so the next mint is the next
/// number and not a second walk over the same collision.
fn mint_id(file: &mut TaskFile) -> TaskId {
    let mut n = file.next_id.max(1);
    loop {
        let candidate = TaskId(format!("t-{n}"));
        if !file.tasks.iter().any(|task| task.id == candidate) {
            file.next_id = n + 1;
            return candidate;
        }
        n += 1;
    }
}

/// Look one task up by id.
pub fn find<'a>(file: &'a TaskFile, id: &TaskId) -> Option<&'a Task> {
    file.tasks.iter().find(|task| &task.id == id)
}

fn find_mut<'a>(file: &'a mut TaskFile, id: &TaskId) -> Option<&'a mut Task> {
    file.tasks.iter_mut().find(|task| &task.id == id)
}

/// The tagged refusal every "no such X" in the domain gets — `NoSuchTab` and `NoSuchPane`'s
/// arrangement, for this file's ids.
///
/// This used to be a [`CoreError::Io`] carrying the same sentence, with a doc explaining that the
/// variant did not exist yet; it does now (`cmd/agents.rs`' dispatch preflight was what forced
/// it), and the display string was written to match this function's byte for byte, so adopting
/// the tag changed no message anything renders or matches.
fn no_such_task(id: &TaskId) -> CoreError {
    CoreError::NoSuchTask(id.clone())
}

/// OpenSpec's own ceiling on a change name. (M28)
///
/// 200 characters, matching `dist/utils/change-utils.js` in `@fission-ai/openspec`. Restated here
/// rather than inferred, because the value ends up as a directory name and a filesystem's own
/// limit is per-component and platform-dependent — a rule cide can state is worth more than one
/// it would discover by failing.
const CHANGE_NAME_MAX: usize = 200;

/// Check a change name against OpenSpec's grammar, or explain why it is not one. (M28)
///
/// `^[a-z0-9]+(?:-[a-z0-9]+)*$`, which is `dist/core/id.js`'s `KEBAB_ID_REGEX` in the installed
/// CLI. Written as a scan rather than a regex because this crate has no regex dependency and the
/// rule is four conditions; the comment naming the upstream file is what keeps the two together.
///
/// # Why this is refused here rather than sanitised
///
/// Because the value is joined onto a path. A name carrying `/` or `..` would let a task in a
/// committed file address a directory outside `openspec/changes/`, and a *sanitising* answer —
/// silently rewriting it to something legal — would produce a task whose link points at a
/// directory that does not exist and never will, which is the failure that is hardest to see. The
/// refusal names the value, so whoever typed it can fix it.
///
/// `None` is always fine: absent means "not spec-driven", which is most tasks.
fn valid_change_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(CoreError::Io(
            "a change name cannot be empty — leave it unset to unlink the task instead".into(),
        ));
    }
    if name.len() > CHANGE_NAME_MAX {
        return Err(CoreError::Io(format!(
            "a change name is at most {CHANGE_NAME_MAX} characters, and this one is {}",
            name.len()
        )));
    }
    let legal = name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !legal || name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(CoreError::Io(format!(
            "`{name}` is not an OpenSpec change name: lowercase letters, digits and single \
             hyphens between them, like `add-dark-mode`. It names a directory under \
             openspec/changes/, which is why it cannot be anything else"
        )));
    }
    Ok(())
}

/// [`valid_change_name`] over the optional the wire actually carries.
///
/// Trims first, so a name pasted with a trailing space is accepted rather than refused for a
/// character nobody can see — the same courtesy `create` extends to a title. A value that is
/// *only* whitespace collapses to `None`: the panel's picker has an empty option, and a `<select>`
/// has no null, so "" arriving here means unlinked and must not become an empty directory name in
/// a committed file. That is the mirror of `assigneeFromDraft`'s rule one layer up.
fn validated_change(change: Option<&ChangeName>) -> Result<Option<ChangeName>> {
    let Some(change) = change else {
        return Ok(None);
    };
    let trimmed = change.as_str().trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    valid_change_name(trimmed)?;
    Ok(Some(ChangeName(trimmed.to_string())))
}

/// A comment that is not there, or is already tombstoned.
///
/// One message for both, because they are one answer to the caller: the comment you named is not
/// something you can act on. Telling them apart would let a caller probe which ids used to exist.
fn no_such_comment(id: &CommentId) -> CoreError {
    CoreError::Io(format!("no such comment: {id}"))
}

/// [`no_such_comment`]'s shape for an attachment: absent and already-tombstoned are one answer.
fn no_such_attachment(id: &TaskAttachmentId) -> CoreError {
    CoreError::Io(format!("no such attachment: {id}"))
}

/// The one place a comment is built, so the append arm and the attach-with-comment arm cannot
/// drift apart on a field. (M39)
fn new_comment(
    text: String,
    attachments: Vec<TaskAttachment>,
    author: TaskAuthor,
    now: u64,
) -> TaskComment {
    TaskComment {
        id: CommentId::new(),
        author,
        text,
        // Stamped here rather than taken from the caller: a timestamp an agent supplies is one
        // it can get wrong, and this one orders the log and decides the merge.
        at_unix_ms: now,
        edited_at_unix_ms: None,
        deleted: false,
        attachments,
    }
}

/// One attachment record, wherever it is on the task — the body, or any comment, deleted or not.
pub fn attachment_record<'a>(task: &'a Task, id: &TaskAttachmentId) -> Option<&'a TaskAttachment> {
    task.attachments
        .iter()
        .chain(task.comments.iter().flat_map(|c| c.attachments.iter()))
        .find(|a| a.id == *id)
}

fn attachment_record_mut<'a>(
    task: &'a mut Task,
    id: &TaskAttachmentId,
) -> Option<&'a mut TaskAttachment> {
    task.attachments
        .iter_mut()
        .chain(
            task.comments
                .iter_mut()
                .flat_map(|c| c.attachments.iter_mut()),
        )
        .find(|a| a.id == *id)
}

/// The refusal an agent gets for [`TaskEdit::EditComment`] or [`TaskEdit::DeleteComment`].
///
/// Reachable only through a path that should not exist: no MCP tool offers either variant, so an
/// agent has to construct one to get here. It is still checked rather than assumed, because "no
/// tool exposes it" is a fact about today's tool list and this is a fact about the log.
fn agents_may_not_rewrite() -> CoreError {
    CoreError::Io(
        "only the user can edit or delete a comment; an agent adds a correcting comment instead"
            .to_string(),
    )
}

/// The wire spelling of a link kind, for refusal sentences. (M30)
///
/// A refusal that says `BlockedBy` to a caller who typed `blockedBy` is a refusal that teaches
/// the wrong vocabulary — the sentence is read by the model that will make the next call. A
/// match rather than a `serde_json::to_value` round trip per message; the test
/// `the_link_wire_spelling_is_serdes` is what keeps it from being a second copy that drifts.
fn link_wire(link: LinkType) -> &'static str {
    match link {
        LinkType::Related => "related",
        LinkType::BlockedBy => "blockedBy",
        LinkType::SubtaskOf => "subtaskOf",
    }
}

/// The chain of live `kind` edges leading from `from` back to `back_to`, if one exists: the
/// intermediate task ids, exclusive of both ends. (M30)
///
/// [`apply_link`]'s cycle preflight. A depth-first walk with a visited set, and the set is not
/// paranoia: the file itself may already hold a cycle — a merge can lawfully create one, see
/// [`validate`] — and the walk must terminate inside it rather than follow it for ever.
fn link_chain(
    file: &TaskFile,
    kind: LinkType,
    from: &TaskId,
    back_to: &TaskId,
) -> Option<Vec<TaskId>> {
    fn walk(
        file: &TaskFile,
        kind: LinkType,
        at: &TaskId,
        back_to: &TaskId,
        visited: &mut Vec<TaskId>,
        path: &mut Vec<TaskId>,
    ) -> bool {
        if visited.contains(at) {
            return false;
        }
        visited.push(at.clone());
        let Some(task) = find(file, at) else {
            // Dangling mid-chain: the walk simply ends here, exactly as the dispatch gate's
            // one-hop read would.
            return false;
        };
        for edge in task.links.iter().filter(|l| l.link == kind && !l.deleted) {
            if &edge.target == back_to {
                return true;
            }
            path.push(edge.target.clone());
            if walk(file, kind, &edge.target, back_to, visited, path) {
                return true;
            }
            path.pop();
        }
        false
    }

    let mut visited = Vec::new();
    let mut path = Vec::new();
    walk(file, kind, from, back_to, &mut visited, &mut path).then_some(path)
}

/// [`TaskEdit::Link`], applied. A free function over the whole file, because the rules it
/// enforces live on *other* tasks: the target's existence, the cycle walk, and (for
/// [`LinkType::Related`]) the inverse edge. (M30)
///
/// The refusals, each a sentence at the gesture — see [`validate`] for why the same states are
/// legal in a file that merged or was hand-edited into them:
///
/// * the target must exist — a *gesture* never creates a dangling edge; only a later delete or
///   an out-of-process merge can,
/// * no self-link, no live duplicate (for `related`, in either direction — the pair is one
///   fact, wherever it is stored),
/// * no `blockedBy`/`subtaskOf` edge that would close a cycle, refused naming the chain.
///
/// A tombstoned edge under the same key is **revived** rather than duplicated — flipping
/// `deleted` back and re-stamping is what keeps one key per fact, which [`union_links`]' whole
/// resolution rests on.
fn apply_link(
    file: &mut TaskFile,
    id: &TaskId,
    link: LinkType,
    target: &TaskId,
    now: u64,
) -> Result<Task> {
    if find(file, id).is_none() {
        return Err(no_such_task(id));
    }
    if target == id {
        return Err(CoreError::Io(format!("{id} cannot be linked to itself")));
    }
    if find(file, target).is_none() {
        return Err(no_such_task(target));
    }

    let wire = link_wire(link);
    let live = |task: &Task, kind: LinkType, to: &TaskId| {
        task.links
            .iter()
            .any(|l| l.link == kind && &l.target == to && !l.deleted)
    };
    let already = live(find(file, id).expect("checked above"), link, target)
        || (link == LinkType::Related
            && find(file, target).is_some_and(|t| live(t, LinkType::Related, id)));
    if already {
        return Err(CoreError::Io(format!(
            "{id} is already linked: {wire} {target}"
        )));
    }

    if link != LinkType::Related
        && let Some(via) = link_chain(file, link, target, id)
    {
        let chain = if via.is_empty() {
            String::new()
        } else {
            format!(
                " (via {})",
                via.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let cost = match link {
            LinkType::BlockedBy => {
                "a blocking cycle would leave every task in it waiting on the others"
            }
            _ => "a task cannot appear inside its own decomposition",
        };
        return Err(CoreError::Io(format!(
            "cannot link {id} {wire} {target}: {target} is itself {wire} {id}{chain}, and {cost}"
        )));
    }

    let task = find_mut(file, id).expect("checked above");
    match task
        .links
        .iter_mut()
        .find(|l| l.link == link && &l.target == target)
    {
        // The tombstoned key, revived. Only ever reached with `deleted: true` — a live copy
        // was refused as a duplicate above.
        Some(edge) => {
            edge.deleted = false;
            edge.at_unix_ms = now;
        }
        None => task.links.push(TaskLink {
            link,
            target: target.clone(),
            deleted: false,
            at_unix_ms: now,
        }),
    }
    task.updated_unix_ms = now;
    Ok(task.clone())
}

/// [`TaskEdit::Unlink`], applied: tombstone one live edge. (M30)
///
/// For [`LinkType::Related`] the edge is looked for on **both** ends — it lives on whichever
/// task the linking gesture was made on, and "unlink these two" must not require the caller to
/// remember which that was. The task returned is always the one the caller named, even when the
/// edge turned out to be stored on the other one: the response is an answer about the gesture's
/// subject, not about where the bookkeeping happened to live.
///
/// One message covers "never existed" and "already tombstoned" — [`no_such_comment`]'s posture,
/// for its reason: both are one answer to the caller, and telling them apart would let a caller
/// probe what used to be linked.
fn apply_unlink(
    file: &mut TaskFile,
    id: &TaskId,
    link: LinkType,
    target: &TaskId,
    now: u64,
) -> Result<Task> {
    if find(file, id).is_none() {
        return Err(no_such_task(id));
    }

    let tombstone = |task: &mut Task, kind: LinkType, to: &TaskId| -> bool {
        let Some(edge) = task
            .links
            .iter_mut()
            .find(|l| l.link == kind && &l.target == to && !l.deleted)
        else {
            return false;
        };
        edge.deleted = true;
        edge.at_unix_ms = now;
        task.updated_unix_ms = now;
        true
    };

    let named = find_mut(file, id).expect("checked above");
    if tombstone(&mut *named, link, target) {
        return Ok(named.clone());
    }
    if link == LinkType::Related
        && let Some(other) = find_mut(file, target)
        && tombstone(other, LinkType::Related, id)
    {
        return Ok(find(file, id).expect("checked above").clone());
    }
    Err(CoreError::Io(format!(
        "no such link on {id}: {} {target}",
        link_wire(link)
    )))
}

/// The stored edges a creation's specs become, refused whole when any spec cannot stand. (M30)
///
/// Runs inside the create's `update` closure, against the file the new task is about to join.
/// Two of [`apply_link`]'s refusals are structurally impossible here and deliberately not
/// restated: a self-link would need the id this call has not minted yet, and a cycle would need
/// an edge *into* a task that does not exist. What is left is existence and duplication.
fn validated_links(specs: &[TaskLinkSpec], file: &TaskFile, now: u64) -> Result<Vec<TaskLink>> {
    let mut out: Vec<TaskLink> = Vec::with_capacity(specs.len());
    for spec in specs {
        if find(file, &spec.target).is_none() {
            return Err(no_such_task(&spec.target));
        }
        if out
            .iter()
            .any(|l| l.link == spec.link && l.target == spec.target)
        {
            return Err(CoreError::Io(format!(
                "the new task names its {} link to {} twice",
                link_wire(spec.link),
                spec.target
            )));
        }
        out.push(TaskLink {
            link: spec.link,
            target: spec.target.clone(),
            deleted: false,
            at_unix_ms: now,
        });
    }
    Ok(out)
}

// --- layer 3: the merge ---------------------------------------------------------------------

/// Fold a copy of the tracker that changed on disk into the copy this process holds.
///
/// A **pure function over two [`TaskFile`]s**, taking no path and touching no disk, because this
/// is the single most likely place in the feature for a bug to live and a rule that can only be
/// exercised by arranging a filesystem race is a rule that is never exercised. Everything in the
/// table below is a row in `merge_table` in this module's tests.
///
/// `mine` is what this process holds; `theirs` is what a fresh read of the file just returned.
///
/// # The rules, and the two failures they exist to prevent
///
/// The alternatives are both worse, and both are what a first implementation reaches for:
///
/// * **Refusing the write** when the file has moved would silently lose whatever the agents wrote
///   since — a comment that a subagent posted as its turn ended, most often, which is the exact
///   payload this whole feature exists to carry.
/// * **Last-writer-wins on the whole file** would delete the three tasks a `git pull` just added,
///   with nothing on screen and nothing in the log, and the user would discover it as "the
///   tracker forgot half the sprint".
///
/// So, per id:
///
/// | case | rule |
/// | --- | --- |
/// | on both sides | the higher `updated_unix_ms` wins the scalar fields |
/// | only on disk | **adopted** — a `git pull` or a teammate added it |
/// | only in memory | **kept** — this process created it since the last read |
/// | comments | **unioned**, by id — [`union_comments`], resolved per id by [`reconcile_comment`] |
/// | history | **unioned**, structurally — [`union_history`]; rows are immutable, so `==` suffices |
/// | links | **unioned**, by `(link, target)` — [`union_links`]; the newer stamp wins per key |
/// | one side deleted what the other edited | the task **survives**, with the edit |
///
/// ## Why comments union by id
///
/// They unioned structurally on `(author, at_unix_ms, text)` until M21, when those three fields
/// stopped being the whole value: editing changes `text`, so a stale copy's original survived
/// *beside* the edit, and a delete removed a comment the stale copy still had, so the next merge
/// put it back. [`TaskComment::id`] is the fix and [`reconcile_comment`] carries the resolution
/// rules; the structural union survives unchanged for [`union_history`], whose rows really are
/// immutable.
///
/// The alternative to collapsing at all, appending both sides unconditionally, is not merely
/// untidy: this merge runs again on the next out-of-process write, so every surviving duplicate
/// is re-duplicated, and a noisy afternoon of `git pull`s turns a five-line log into fifty.
///
/// ## Why a delete does not survive a concurrent write
///
/// Nothing in the file records that a task was deleted — there is no tombstone, and adding one
/// would be a row in a committed file whose only purpose is to remember something the user asked
/// to be forgotten. So "present only in memory" is genuinely ambiguous between *this process
/// created it* and *the other side deleted it*, and "present only on disk" between *they created
/// it* and *we deleted it*. Both resolve the same way: **keep the task.** A resurrected task is
/// one gesture to repeat and the user is looking straight at it; a lost task is work that nobody
/// knows was lost. `cide_task_delete` is deliberately not an MCP tool for the neighbouring reason
/// — deletion belongs to the user.
///
/// ## Order
///
/// `mine`'s order is preserved exactly, and disk-only tasks are appended in `theirs`' order. The
/// array *is* the order and the user is looking at `mine` right now; re-sorting the panel under
/// their cursor because a teammate pushed is the visible harm, and an adopted task is new to this
/// process, so it goes where new things go.
pub fn merge(mine: &TaskFile, theirs: &TaskFile) -> TaskFile {
    let mut tasks: Vec<Task> = Vec::with_capacity(mine.tasks.len() + theirs.tasks.len());

    for ours in &mine.tasks {
        match find(theirs, &ours.id) {
            Some(disk) => tasks.push(merge_task(ours, disk)),
            // Only in memory: created here since the last read, or deleted there. Kept, see above.
            None => tasks.push(ours.clone()),
        }
    }
    for disk in &theirs.tasks {
        if find(mine, &disk.id).is_none() {
            // Only on disk: a `git pull` or a teammate added it. Adopted, at the end.
            tasks.push(disk.clone());
        }
    }

    TaskFile {
        // Both sides are at this build's schema or the merge never runs — `read` refuses a future
        // schema before it can reach either argument.
        schema_version: TaskFile::CURRENT_SCHEMA,
        // **Past both**, not `max`. Anything holding either side's `rev` — a second cide window, a
        // panel mid-render — must see this as newer, and `TaskFile::rev`'s own doc is the contract
        // that says a receiver drops a snapshot whose rev is not newer.
        rev: mine.rev.max(theirs.rev) + 1,
        // The higher of the two, and not past both: each side is past every id it minted, so the
        // higher is past every id either minted, and — unlike `rev` — nobody compares this one
        // for freshness. Two boards that each minted `t-50` since their common ancestor collided
        // above, in `merge_task`, exactly as they did under the old rule; the counter neither
        // causes nor cures that.
        next_id: mine.next_id.max(theirs.next_id),
        tasks,
    }
}

/// One task that exists on both sides.
///
/// The scalar fields come from the newer of the two and the comments come from both, which is the
/// asymmetry worth stating out loud: a status change and a comment are different kinds of edit,
/// and a subagent that comments while the orchestrator moves the task to `Review` must not lose
/// either. Ties go to `mine` — an arbitrary choice, but a *stable* one, so the same two files
/// merge to the same result whichever side reads first.
fn merge_task(mine: &Task, theirs: &Task) -> Task {
    let mut winner = if theirs.updated_unix_ms > mine.updated_unix_ms {
        theirs.clone()
    } else {
        mine.clone()
    };

    winner.comments = union_comments(&mine.comments, &theirs.comments);
    winner.history = union_history(&mine.history, &theirs.history);
    winner.links = union_links(&mine.links, &theirs.links);
    winner.attachments = union_attachments(&mine.attachments, &theirs.attachments);
    // The creation stamp is the earlier of the two by definition: a task cannot have been created
    // twice, and if the two disagree one of them was hand-edited. The earlier is the safer read.
    winner.created_unix_ms = mine.created_unix_ms.min(theirs.created_unix_ms);
    /*
     * The creator does **not** follow the winner, and the rule is directional rather than a tie.
     *
     * A task is created once, so the two sides can only disagree about `created_by` for a reason
     * that is not an edit: one of them passed through a build that did not know the field, or a
     * hand edit dropped it. Both of those decay to `TaskAuthor::User` — so `User` is the value that
     * means *nobody recorded one*, and it must never overwrite a side that did. The other
     * direction cannot lose anything: the only thing it discards is a decay.
     *
     * Ties — and the case where both sides name a creator — go to `mine`, which is this
     * function's standing convention and is stated in its doc.
     */
    winner.created_by = if mine.created_by == TaskAuthor::User {
        theirs.created_by.clone()
    } else {
        mine.created_by.clone()
    };
    // `updated_unix_ms` follows the winner already, but a comment adopted from the loser is itself
    // an update, and a merged task whose stamp predates its own newest comment would lose the next
    // merge it takes part in. A history row adopted from the loser is an update by the identical
    // argument.
    if let Some(newest) = winner.comments.iter().map(|c| c.at_unix_ms).max() {
        winner.updated_unix_ms = winner.updated_unix_ms.max(newest);
    }
    if let Some(newest) = winner.history.iter().map(|c| c.at_unix_ms).max() {
        winner.updated_unix_ms = winner.updated_unix_ms.max(newest);
    }
    // A link edge adopted from the loser is an update by the same argument — and here it is
    // load-bearing twice over, because an adopted edge's stamp is what wins it the *next* merge.
    if let Some(newest) = winner.links.iter().map(|l| l.at_unix_ms).max() {
        winner.updated_unix_ms = winner.updated_unix_ms.max(newest);
    }
    // An attachment adopted from the loser — on the body or on a comment — is an update by the
    // same argument.
    if let Some(newest) = winner
        .attachments
        .iter()
        .chain(winner.comments.iter().flat_map(|c| c.attachments.iter()))
        .map(|a| a.added_unix_ms)
        .max()
    {
        winner.updated_unix_ms = winner.updated_unix_ms.max(newest);
    }
    winner
}

/// Both sides' comments, oldest first, with exact duplicates collapsed.
///
/// A linear scan rather than a hash set: [`TaskAuthor`] derives `PartialEq` and not `Hash`, and a
/// task's log is a handful of lines — a set would buy nothing and would force a `Hash` derive onto
/// a wire type this crate does not own.
fn union_comments(mine: &[TaskComment], theirs: &[TaskComment]) -> Vec<TaskComment> {
    let mut out: Vec<TaskComment> = mine.to_vec();
    for comment in theirs {
        match out.iter_mut().find(|c| c.id == comment.id) {
            Some(ours) => *ours = reconcile_comment(ours, comment),
            None => out.push(comment.clone()),
        }
    }
    // A **stable** sort, so two comments sharing a millisecond keep the order they were written in
    // — mine before theirs — rather than being shuffled by an unstable sort's pivot choice. The
    // DTO's contract is oldest first, and the panel and every agent read it that way. Sorted on
    // `at_unix_ms` and never on the edit stamp: editing a comment must not move it in the log.
    out.sort_by_key(|c| c.at_unix_ms);
    out
}

/// Both sides' status transitions, oldest first, with exact duplicates collapsed. (M27)
///
/// Structural identity, and that is sufficient where a comment needed an id: a history row is
/// **immutable** — no `TaskEdit` variant edits or deletes one — so the two hazards that broke
/// the comments' structural union (an edit changing the key, a delete the stale copy resurrects)
/// cannot arise. Two sides that each recorded their own transitions keep both sets, which is the
/// point: the orchestrator moving a task to `Review` while a stale worktree copy still carries
/// the `Todo → Doing` row must lose neither.
fn union_history(mine: &[TaskStatusChange], theirs: &[TaskStatusChange]) -> Vec<TaskStatusChange> {
    let mut out: Vec<TaskStatusChange> = mine.to_vec();
    for change in theirs {
        if !out.contains(change) {
            out.push(change.clone());
        }
    }
    // Stable, on the stamp, for `union_comments`' reason: rows sharing a millisecond keep
    // mine-before-theirs rather than an unstable pivot's choice.
    out.sort_by_key(|c| c.at_unix_ms);
    out
}

/// Both sides' link edges, resolved per `(link, target)` key: **the newer stamp wins, ties go to
/// mine.** (M30)
///
/// # Why not whole-vector last-writer-wins
///
/// Doing nothing would inherit [`merge_task`]'s rule — the loser's entire scalar set is
/// discarded. Then: side A unlinks `blockedBy t-3` at 10:00, side B's stale copy takes an
/// unrelated *comment* at 10:01, B wins the scalars, and the unlink is resurrected — silently,
/// and a resurrected `blockedBy` silently re-gates dispatch. A subagent commenting is the single
/// most common write in the file, so this is not a corner; it is the ordinary afternoon.
/// [`reconcile_comment`] documents the identical hazard for comments, which is why edges carry a
/// tombstone and a stamp at all.
///
/// # Why not the comments' rule (deleted wins absolutely)
///
/// [`TaskComment::deleted`] can be one-way because a deleted comment is *replaced* — a new
/// comment gets a new id, so the tombstoned key is never written to again. A link's identity is
/// its `(link, target)` pair: re-linking after an unlink recreates the **same** key, so
/// "either side deleted ⇒ deleted" would make every unlink permanent after any merge with a
/// stale file. The tombstone is therefore a *toggle*, resolved by the stamp — which is
/// `reconcile_comment`'s rule 2 where its rule 1 cannot apply. The cost — both ends toggling one
/// edge inside clock skew resolves by wall clock — is bounded by what links are: low-frequency,
/// deliberate gestures, where the stamp order and the intent order agree in practice and a wrong
/// resolution is one visible chip and one gesture to repeat.
fn union_links(mine: &[TaskLink], theirs: &[TaskLink]) -> Vec<TaskLink> {
    let mut out: Vec<TaskLink> = mine.to_vec();
    for edge in theirs {
        match out
            .iter_mut()
            .find(|l| l.link == edge.link && l.target == edge.target)
        {
            // Strictly newer, so a tie keeps mine — this function's side of `merge_task`'s
            // stated stability convention.
            Some(ours) => {
                if edge.at_unix_ms > ours.at_unix_ms {
                    *ours = edge.clone();
                }
            }
            None => out.push(edge.clone()),
        }
    }
    // Stable, on the stamp, for `union_comments`' reason.
    out.sort_by_key(|l| l.at_unix_ms);
    out
}

/// Both sides' attachment records, resolved per id: **either side deleted ⇒ deleted, otherwise
/// mine.** (M39)
///
/// [`TaskComment::deleted`]'s rule and not [`union_links`]' toggle, for the reason that doc gives
/// for why a comment can have the simpler one: a record is never written to again under its id.
/// There is no "re-attach the same attachment" — a person who wants the file back attaches it
/// again and gets a new id — so a tombstone is final and no stamp needs consulting. Every other
/// field is immutable, so when neither side deleted, the two copies are the same record and
/// keeping mine is not a choice, only the stable convention written down.
fn union_attachments(mine: &[TaskAttachment], theirs: &[TaskAttachment]) -> Vec<TaskAttachment> {
    let mut out: Vec<TaskAttachment> = mine.to_vec();
    for attachment in theirs {
        match out.iter_mut().find(|a| a.id == attachment.id) {
            Some(ours) => {
                if attachment.deleted {
                    ours.deleted = true;
                }
            }
            None => out.push(attachment.clone()),
        }
    }
    // Stable, on the stamp, for `union_comments`' reason.
    out.sort_by_key(|a| a.added_unix_ms);
    out
}

/// Two copies of one comment, resolved.
///
/// # Why identity moved off the content (M21)
///
/// The union used to be structural: `(author, at_unix_ms, text)` were the only three fields
/// there were, so `==` said exactly what the rule said. Editing breaks that in both directions —
/// an edit changes `text`, so the stale copy's original survives *beside* the new one, and a
/// delete removes a comment the stale copy still has, so the next merge puts it back. Neither
/// failure raises anything; the file simply grows a comment the user thought they had fixed.
///
/// So the union is by [`CommentId`] and this decides the rest. Two rules, in this order:
///
/// 1. **Deleted wins.** `deleted` only ever goes false → true, so there is no ordering question
///    and no way for a stale file to resurrect a tombstone. The text goes with it — a delete
///    whose words stayed in the file would not be a delete.
/// 2. **Otherwise the newer edit wins**, where "never edited" counts as the comment's own
///    `at_unix_ms`. An edited copy therefore always beats an untouched one, whichever file it
///    came from, and two edits resolve by wall clock — the same authority `at_unix_ms` already
///    has for ordering the log.
///
/// Both rules pick a *whole* copy, and one field is then re-derived from both sides:
/// `attachments`, through [`union_attachments`]. Without that, an edit to the text on one side
/// while a screenshot was attached to the same comment on the other would drop the screenshot
/// with nothing logged — the loser's attachment list is not a rival version of the winner's, it
/// is a set the two sides each added to. (M39)
fn reconcile_comment(mine: &TaskComment, theirs: &TaskComment) -> TaskComment {
    let attachments = union_attachments(&mine.attachments, &theirs.attachments);
    if mine.deleted || theirs.deleted {
        let mut out = if mine.deleted {
            mine.clone()
        } else {
            theirs.clone()
        };
        out.deleted = true;
        out.text = String::new();
        out.attachments = attachments;
        return out;
    }
    let stamp = |c: &TaskComment| c.edited_at_unix_ms.unwrap_or(c.at_unix_ms);
    let mut out = if stamp(theirs) > stamp(mine) {
        theirs.clone()
    } else {
        mine.clone()
    };
    out.attachments = attachments;
    out
}

// --- layer 2: reading, repairing, and never failing -----------------------------------------

/// What a read of the file found.
///
/// Four outcomes rather than `Result<Option<TaskFile>>`, because the three failure shapes are
/// acted on differently and telling them apart by matching on an error string is how that stops
/// being true. This is [`TaskBoard`]'s argument one layer down: an empty list cannot say which of
/// them happened.
#[derive(Debug)]
pub enum ReadOutcome {
    /// No file. Not an error — most projects have never had a tracker.
    Absent,
    /// Read, understood, and already through `repair`.
    Ready {
        file: TaskFile,
        /// Stamped **before** the bytes were read, deliberately. See [`read`].
        stamp: Option<FileStamp>,
    },
    /// The file is there and this build must not touch it: a schema from the future, a file in the
    /// middle of a merge, or a read that failed for a reason that will probably not recur.
    ///
    /// Nothing renames it, nothing overwrites it, and [`TaskStore::update`] refuses while it holds.
    Refused { error: String },
    /// The bytes are not a task file at all.
    ///
    /// `conflicted` is the one distinction that changes what happens next — see `load`.
    Unparseable { error: String, conflicted: bool },
}

/// Read the tracker. Never renames anything, never fails.
///
/// # Why the stamp is taken before the read and not after
///
/// The window between the `stat` and the `read` is unavoidable, but its *direction* is a choice.
/// Stamping first means a file rewritten inside that window leaves this store holding newer bytes
/// under an older stamp: the next write preflight sees a mismatch, re-reads and merges — one
/// redundant merge, which is idempotent. Stamping afterwards would leave newer bytes under a
/// *newer* stamp, so the preflight would see agreement and quietly overwrite the change. One
/// direction costs a wasted merge, the other loses data.
pub fn read(path: &Path) -> ReadOutcome {
    let stamp = document::stamp_at(path);

    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return ReadOutcome::Absent,
        // A read that failed for any other reason — EACCES under the wrong user on a shared
        // checkout, EIO, a directory where a file should be — is `Refused` rather than
        // `Unparseable`, and the difference is that nothing will rename it. `persist::load` draws
        // the same line and states the reason: a transient failure turned into a rename is
        // permanent data loss presented to the user as a reset they did not ask for.
        Err(error) => {
            return ReadOutcome::Refused {
                error: format!("could not read {}: {error}", path.display()),
            };
        }
    };

    // Checked before parsing, because a conflicted file is *also* unparseable and the two answers
    // differ. See `load` for what rides on it.
    let conflicted = looks_conflicted(&raw);

    let value: Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            return ReadOutcome::Unparseable {
                error: error.to_string(),
                conflicted,
            };
        }
    };

    // Read `schemaVersion` out of the raw document *before* committing to a shape the file may not
    // have — `persist::read_workspace`'s move, and for the same reason: deserialising straight into
    // `TaskFile` would report a future schema as "missing field", which is a message that sends the
    // user looking for the wrong problem.
    let Some(version) = value.get("schemaVersion").and_then(Value::as_u64) else {
        return ReadOutcome::Unparseable {
            error: "not a task file: no schemaVersion".to_string(),
            conflicted,
        };
    };
    let current = u64::from(TaskFile::CURRENT_SCHEMA);
    if version != current {
        // Both directions refuse, and neither renames. A **newer** schema is a teammate on a newer
        // build whose file this one would silently downgrade on the next flush, dropping every
        // field it does not know — the failure is total and it lands in their repository. An
        // **older** one has no migration ladder yet; when schema 2 arrives, the ladder goes here,
        // modelled on `persist::migrate`, and this arm shrinks to the `v > current` half.
        let direction = if version > current {
            "newer than this build's"
        } else {
            "older than this build's, and no migration exists for it:"
        };
        return ReadOutcome::Refused {
            error: format!("task file schema {version} is {direction} {current}"),
        };
    }

    match serde_json::from_str::<TaskFile>(&raw) {
        Ok(mut file) => {
            repair(&mut file);
            ReadOutcome::Ready { file, stamp }
        }
        Err(error) => ReadOutcome::Unparseable {
            error: error.to_string(),
            conflicted,
        },
    }
}

/// Whether the file still has both sides of a merge in it.
///
/// Cheap and deliberately crude: a line beginning `<<<<<<<` or `>>>>>>>` is a conflict marker and
/// nothing else, because JSON has no syntax that starts a line that way and the tracker's own
/// values are inside quotes. `=======` is not tested for — it is the one marker that could
/// plausibly appear inside a task body as a rule of dashes.
fn looks_conflicted(raw: &str) -> bool {
    raw.lines()
        .any(|line| line.starts_with("<<<<<<<") || line.starts_with(">>>>>>>"))
}

/// What state the tracker on disk is in, from the store's point of view.
///
/// Distinct from [`TaskBoard`] because the panel does not need — and must not be given — the
/// distinction between the last two: both read "the tracker is unreadable, here is why", and only
/// this crate cares that one of them has left the path free to write.
#[derive(Debug, Clone)]
enum DiskState {
    /// No file — and the next flush creates one **only if there is a task to put in it**.
    ///
    /// See the guard in [`TaskStore::write_now`]: a project whose tracker was never used must
    /// not gain a committed empty file for having been opened and closed.
    Absent,
    /// Read and understood.
    Ready,
    /// Present, and this build must not touch it. **Every write is refused while this holds.**
    ///
    /// This is the state `TaskBoard::Unreadable`'s doc is written for: *"the panel can offer
    /// nothing that writes … an app that recovers from an unparseable tracker by overwriting it
    /// has destroyed the user's data in order to fix its own display"*. Enforced here rather than
    /// in the panel, for the reason `CoreError::UnsavedChanges` gives: the frontend's restraint is
    /// a courtesy and a courtesy is not a guard — an MCP tool call from an agent never sees the
    /// panel at all.
    Unreadable { error: String },
    /// The bytes were not a task file and have been moved aside; the path is free.
    ///
    /// Reported to the panel as `Unreadable` too, with a message naming where the bytes went, so
    /// that the user is told once, loudly, before *New task* becomes available again — which it
    /// does after [`TaskStore::reload`], the panel's Retry.
    Quarantined { error: String },
}

/// [`read`], plus the one repair that touches the filesystem.
///
/// An **unparseable** file that carries no conflict markers is moved aside as
/// `tasks.corrupt-<n>.json` and the store starts empty, exactly as `persist::load` does for
/// `workspace.json`, and for the same reason: the next launch must not be stuck rejecting the same
/// bytes for ever. Nothing is destroyed — the bytes keep their content under a new name, beside
/// the original, and `git checkout -- .cide/tasks.json` restores the tracked copy.
///
/// **A conflicted file is left exactly where it is.** This is the one place this module departs
/// from `persist::load`, and it departs because the files differ: `workspace.json` lives in
/// `$XDG_STATE_HOME` and is nobody's working tree, while this one is inside a git repository and
/// the overwhelmingly likely cause of it not parsing is a merge in progress. Renaming a path out
/// from under `git merge` breaks `git checkout --theirs`, breaks `git mergetool`, and presents the
/// user with a deletion plus an untracked file at the exact moment they are least able to reason
/// about it. `TaskBoard::Unreadable`'s doc names this case by name; the conflict check is what
/// makes both that doc and `persist::load`'s discipline true at once.
fn load(path: &Path) -> (TaskFile, DiskState, Option<FileStamp>) {
    match read(path) {
        ReadOutcome::Absent => (TaskFile::default(), DiskState::Absent, None),
        ReadOutcome::Ready { file, stamp } => (file, DiskState::Ready, stamp),
        ReadOutcome::Refused { error } => {
            tracing::warn!(path = %path.display(), %error, "task file left untouched");
            (
                TaskFile::default(),
                DiskState::Unreadable { error },
                // Deliberately no stamp. A `None` here means the next write preflight sees a
                // mismatch against anything and re-reads, which is what should happen: the file is
                // one this build refused, and the moment it changes is the moment to look again.
                None,
            )
        }
        ReadOutcome::Unparseable {
            error,
            conflicted: true,
        } => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "task file has unresolved merge conflict markers; leaving it for git"
            );
            (
                TaskFile::default(),
                DiskState::Unreadable {
                    error: format!("{error} (the file still contains merge conflict markers)"),
                },
                None,
            )
        }
        ReadOutcome::Unparseable {
            error,
            conflicted: false,
        } => {
            let moved = quarantine(path);
            let error = match &moved {
                Some(to) => format!("{error} — the file was moved aside to {}", to.display()),
                None => error,
            };
            (TaskFile::default(), DiskState::Quarantined { error }, None)
        }
    }
}

/// Move an unreadable file aside so it is recoverable and no longer in the way.
///
/// The naming scheme — `<stem>.corrupt-<n>.<ext>`, first free `n` from 1, a path that cannot be
/// `stat`ed treated as taken so an earlier copy is never overwritten — is `persist`'s, reproduced
/// rather than called because `persist::quarantine` and its two helpers are private and this crate
/// does not own that file. **Two naming schemes for one concept is the drift this comment exists to
/// prevent**: a user who has learned to look for `workspace.corrupt-1.json` must find
/// `tasks.corrupt-1.json` beside their tracker and not `tasks.json.bak.2`. Widening those three
/// functions to `pub` in `cide-core` and deleting this one is the right end state.
///
/// Best-effort: a rename that fails leaves the file in place and says so, and the caller still
/// starts empty.
fn quarantine(path: &Path) -> Option<PathBuf> {
    let Some(target) = free_quarantine_path(path) else {
        tracing::warn!(
            path = %path.display(),
            "no free quarantine name; leaving the unusable task file in place"
        );
        return None;
    };
    match fs::rename(path, &target) {
        Ok(()) => {
            tracing::warn!(
                from = %path.display(),
                to = %target.display(),
                "moved the unusable task file aside"
            );
            Some(target)
        }
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "could not move the unusable task file aside"
            );
            None
        }
    }
}

/// The first unused `tasks.corrupt-<n>.json` beside `path`.
///
/// Bounded for `persist::free_quarantine_path`'s reason: a directory that somehow holds every name
/// should end the search rather than spin.
fn free_quarantine_path(path: &Path) -> Option<PathBuf> {
    (1..=999u32).map(|n| quarantine_path(path, n)).find(|c| {
        // A path we cannot even `stat` is treated as taken: overwriting it could destroy an
        // earlier quarantined copy.
        matches!(c.try_exists(), Ok(false))
    })
}

fn quarantine_path(path: &Path, n: u32) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tasks".into());
    let name = match path.extension() {
        Some(ext) => format!("{stem}.corrupt-{n}.{}", ext.to_string_lossy()),
        None => format!("{stem}.corrupt-{n}"),
    };
    path.with_file_name(name)
}

// --- layer 4: the atomic write, at a mode a team can read ------------------------------------

/// Publish `json` at `path` atomically, at a mode the user's team can read.
///
/// One call to [`persist::write_atomic_with_mode`], and a named function rather than that call
/// spelled out at its call site, because the name is the part that says *this file is shared,
/// deliberately* — and because the argument for the mode has to live somewhere a reader wondering
/// why the tracker is not 0600 will find it.
///
/// # Why the mode differs from `workspace.json`'s
///
/// [`persist::write_atomic`] creates at [`persist::PRIVATE_MODE`], and that is *correct there and
/// wrong here*. It is correct there because `workspace.json` gained `Settings::proxy` and can hold
/// `http://user:hunter2@proxy:3128`; its doc records the incident that put it there, a
/// `screens.json` written with a plain `fs::write` that published a screenful of the user's shell
/// output at 0644. But this file is **committed and shared**: on a checkout two developers use, or
/// a build agent running as another user, 0600 means the tracker exists and cannot be read, and the
/// failure looks like a missing feature rather than a permission problem. Hence
/// [`persist::SHARED_MODE`].
///
/// # 0644 is a request, not a floor — and neither is `.cide/` itself
///
/// `OpenOptions::mode` is masked by the process umask, so 0644 under the usual `022` lands on 0644
/// and under a restrictive `077` lands on 0600. That asymmetry with `write_atomic` is deliberate,
/// and `write_atomic_with_mode`'s own doc argues it at length: there, 0600 is a ceiling the umask
/// can only lower and the guarantee is "never wider than this"; here the guarantee is "no wider
/// than the user's own umask would make a file they created", which is the right thing to promise
/// about a file that goes into their repository. Nothing widens the mode past the umask, and an
/// earlier attempt to do exactly that is the mistake that doc exists to record.
///
/// The same holds one level up, for the directory. `write_atomic_with_mode` creates the parent —
/// which is what brings `.cide/` into being on the first task in a project — and `create_dir_all`
/// is umask-masked too, so the directory lands at 0755 under the usual umask and at 0700 under a
/// tight one. Worth stating rather than assuming, because a directory a teammate cannot enter
/// hides the file just as thoroughly as a mode 0600 file would; it is the same promise as the
/// file's, and the user's umask is no more cide's to override there than here.
///
/// The temp file it publishes through is a dot-prefixed sibling, which matters more here than it
/// does for `workspace.json`: this one lands in the user's working tree, where a visible
/// `tasks.json.12345.0.tmp` would sit in `git status` for as long as a failed write left it behind.
pub fn write_shared(path: &Path, json: &[u8]) -> Result<()> {
    persist::write_atomic_with_mode(path, json, persist::SHARED_MODE)
}

// --- layer 1: the single owning actor --------------------------------------------------------

/// The one thing in the process allowed to write `.cide/tasks.json`.
///
/// Shaped after `cide_app::WorkspaceState` down to the field names, because the two do the same
/// job and a reviewer who has read one should not have to read the other from scratch: a path, a
/// `Mutex` around the durable value, and a `Debouncer` deciding when a burst of mutations becomes
/// a write. [`Self::update`] mirrors `WorkspaceState::update` step for step.
///
/// # Locks, and the order they are taken in
///
/// Three, and the order is always `inner` → `state` → `stamp`. Nothing here calls back into the
/// store, and [`Self::update`]'s closure must not either — it runs under `inner`, so anything that
/// re-enters deadlocks, which is the same contract `WorkspaceState::with` states.
///
/// `state` is the field the four-field sketch of this struct did not have, and it is here because
/// [`TaskBoard`] has three variants. `Absent` and `Ready` can be derived from a `stat` and the
/// in-memory file, but `Unreadable` carries the parser's own message and there is nowhere else to
/// keep it — and a `TaskBoard` variant that nothing can ever produce is the dead-control failure
/// this repository has already deleted one instance of (`SplitIntent::Diff`).
pub struct TaskStore {
    path: PathBuf,
    inner: Mutex<TaskFile>,
    debounce: Debouncer,
    /// The stamp of the bytes this store last wrote or read.
    ///
    /// `cide_ipc::FileStamp`, whose doc argues exactly this case one layer up: a cheap "is this
    /// still the file I read" token, mtime **and** length because a coarse filesystem can produce
    /// two writes inside one mtime tick. `None` means "no file, or nothing worth comparing", and
    /// it compares unequal to any real stamp on purpose — the conservative answer is to re-read.
    stamp: Mutex<Option<FileStamp>>,
    state: Mutex<DiskState>,
    /// The project root the file was opened under: where `.cide/attachments/` is. (M39)
    ///
    /// Stored, not derived by walking `path` up two parents — that would silently break the day
    /// [`TASKS_RELATIVE`] grows a component, and nothing would say so.
    root: PathBuf,
}

impl TaskStore {
    /// Open the tracker for a project rooted at `project_root`. Never fails.
    ///
    /// A file that will not parse costs the tracker, not the project: `load` answers with an empty
    /// file and a state the panel can explain, which is `persist::load`'s discipline and the reason
    /// it exists — a broken layout must not become a launch loop, and a broken tracker must not
    /// become a project that cannot open.
    pub fn open(project_root: &Path) -> Self {
        let path = tasks_path(project_root);
        let (file, state, stamp) = load(&path);

        // `repair` is total, so this cannot fire; it is here because if it ever does, every
        // subsequent `update` would roll back and the tracker would silently refuse every edit,
        // which is a failure with no symptom other than this line.
        if let Err(error) = validate(&file) {
            tracing::error!(
                path = %path.display(),
                %error,
                "a repaired task file still fails validation — this is a bug in cide_tasks::repair"
            );
        }

        Self {
            path,
            inner: Mutex::new(file),
            debounce: Debouncer::new(persist::SAVE_DEBOUNCE),
            stamp: Mutex::new(stamp),
            state: Mutex::new(state),
            root: project_root.to_path_buf(),
        }
    }

    /// The project root this tracker belongs to — the base of every attachment's path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the whole file. Clones, for `WorkspaceState::snapshot`'s reason: a `MutexGuard` must
    /// not be handed to serde while another mutation waits on it.
    pub fn snapshot(&self) -> TaskFile {
        self.inner.lock().clone()
    }

    /// One task, by id.
    pub fn get(&self, id: &TaskId) -> Option<Task> {
        find(&self.inner.lock(), id).cloned()
    }

    /// Every task, in file order — which is the order the panel renders and an agent reads.
    pub fn list(&self) -> Vec<Task> {
        self.inner.lock().tasks.clone()
    }

    /// What the panel should say, in one of three shapes.
    ///
    /// `Absent` is reserved for "there is genuinely nothing to show": no file *and* nothing created
    /// since. A task created a moment ago and still inside the 500 ms debounce has no file behind it
    /// yet and must still render, or the panel would blink empty for half a second after the gesture
    /// that filled it.
    pub fn board(&self) -> TaskBoard {
        let file = self.inner.lock();
        let state = self.state.lock();
        match &*state {
            // The panel is given one sentence for both, and deliberately: "the tracker is
            // unreadable, here is the parser's own message". That one of them has already moved the
            // bytes aside is this crate's business — it changes whether a *later* write is allowed,
            // not what the user is told now.
            DiskState::Unreadable { error } | DiskState::Quarantined { error } => {
                TaskBoard::Unreadable {
                    path: self.path.clone(),
                    error: error.clone(),
                }
            }
            DiskState::Absent if file.tasks.is_empty() && file.rev == 0 => TaskBoard::Absent {
                // Built here, where the fact is known, rather than in the webview — and it says out
                // loud that the file is committed, because a button that quietly adds a tracked file
                // to somebody's repository is a surprise commit and the surprise is the part that
                // has to go. The path itself rides beside it and the panel prints it in full.
                hint: "This project has no task tracker yet. Creating the first task writes a new \
                       file, which is committed with your code and read by everyone — and every \
                       agent — working in it."
                    .to_string(),
                path: self.path.clone(),
            },
            _ => TaskBoard::Ready {
                tasks: file.tasks.clone(),
                rev: file.rev,
            },
        }
    }

    /// Re-read the file from disk. The panel's *Retry*.
    ///
    /// Merges rather than replaces whenever this store has state of its own, because a reload must
    /// not be a way to lose the four tasks an agent created inside the current debounce window. A
    /// store that has not been touched since it was opened takes the disk copy outright, so that the
    /// common case — Retry after fixing a merge conflict — does not leave `rev` one ahead of the
    /// file for no reason.
    pub fn reload(&self) -> TaskBoard {
        {
            let mut file = self.inner.lock();
            let mut state = self.state.lock();
            let (disk, disk_state, stamp) = load(&self.path);
            if matches!(disk_state, DiskState::Ready) {
                *file = if *file == TaskFile::default() {
                    disk
                } else {
                    merge(&file, &disk)
                };
            }
            *state = disk_state;
            *self.stamp.lock() = stamp;
        }
        self.board()
    }

    /// Mutate under the lock, then validate, bump `rev` and mark the file dirty.
    ///
    /// `WorkspaceState::update`'s shape, deliberately, and the differences are these two:
    ///
    /// * **It refuses outright while the file on disk is one this build must not touch.** That is
    ///   `TaskBoard::Unreadable`'s "offer nothing that writes", enforced in the domain rather than
    ///   in the panel — an MCP tool call from a subagent never sees the panel.
    /// * **`rev` is bumped here** rather than by the operation, as `cide_core::workspace`'s are.
    ///   There, the operations are free functions shared with `cide-headless` and tested on their
    ///   own; here every mutation in the system funnels through this one method, so bumping in one
    ///   place is strictly better than five places that can each forget.
    ///
    /// A mutation that leaves the file invalid is rolled back and reported. Because `repair` has
    /// already made whatever was loaded valid, a failure here can only have been caused by *this*
    /// mutation — an empty title from a `SetTitle`, most likely.
    ///
    /// The closure runs under the lock and must not block, mutate anything else that could take
    /// this lock, or call back into the store.
    pub fn update<T>(&self, f: impl FnOnce(&mut TaskFile) -> Result<T>) -> Result<T> {
        let mut guard = self.inner.lock();

        if let DiskState::Unreadable { error } = &*self.state.lock() {
            return Err(CoreError::Io(format!(
                "refusing to change {} while it is unreadable: {error}",
                self.path.display()
            )));
        }

        let before = guard.clone();

        let outcome = f(&mut guard);
        if outcome.is_ok()
            && let Err(error) = validate(&guard)
        {
            *guard = before;
            tracing::error!(%error, "rejected a mutation that broke a task file invariant");
            return Err(error);
        }
        if outcome.is_err() {
            // An operation that reports failure should not have changed anything, but restoring
            // costs one clone and removes the question.
            *guard = before;
            return outcome;
        }

        guard.rev += 1;
        drop(guard);
        self.debounce.note_change();
        outcome
    }

    /// Add a task, minting its id.
    ///
    /// `req.project` is ignored: this store already knows which project it is, from the path it was
    /// opened with. The field is on the wire because the MCP tool and the frontend command have to
    /// name a project to reach the right store, and it is checked there — carrying it further would
    /// be a second source of truth for the same fact.
    ///
    /// # `author` is recorded, and no longer seeds a comment (M21)
    ///
    /// This used to answer *who asked for this task* by writing one line into the log — `created
    /// this task`, from the agent or the orchestrator — because [`Task`] had no creator field. It
    /// has one now, [`Task::created_by`], and that field's doc has the argument for the change.
    ///
    /// The seeding is gone rather than kept beside it. Two records of one fact is a card that
    /// prints the creator in its head and repeats it as the first line of the conversation
    /// underneath, and the log is short enough that a redundant line in it is expensive. Files
    /// written by an older build keep their seeded comment — nothing rewrites them — and [`repair`]
    /// reads the creator back out of it, so those tasks draw exactly as they always did plus a head
    /// that now agrees with the log.
    pub fn create(&self, req: &TaskNew, author: TaskAuthor) -> Result<Task> {
        let now = persist::now_ms();
        // Before the lock and before the write: a refusal must not be able to leave a
        // half-created task behind, and the caller wants the sentence, not a poisoned file.
        let change = validated_change(req.change.as_ref())?;
        self.update(move |file| {
            // Inside the closure, unlike `change` above, because two of its refusals need the
            // file (existence) — and a refusal from in here rolls the update back whole, so
            // nothing half-creates either way. Landing the edges *in* the create matters beyond
            // convenience: the auto-dispatch trigger reads the task this mutation leaves behind,
            // and a create-then-link would hand it a task whose `blockedBy` did not exist yet —
            // `TaskNew::links` carries the argument.
            let links = validated_links(req.links.as_deref().unwrap_or(&[]), file, now)?;
            let task = Task {
                id: mint_id(file),
                // Trimmed, because a title is one line drawn in a 320px panel and trailing space is
                // invisible there but not in the file's diff.
                title: req.title.trim().to_string(),
                body: req.body.clone().unwrap_or_default(),
                // `Todo` unless the caller named one, which only the user's compose dialog does —
                // see `TaskNew::status` for why that is the user's field and structurally not an
                // agent's.
                status: req.status.unwrap_or(cide_ipc::TaskStatus::Todo),
                agent: req.agent.clone(),
                // Validated before it can reach the file, because this value names a directory:
                // `cide-spec` joins it onto `openspec/changes/`, and a task carrying `../..`
                // would be a path traversal written into the user's committed tracker.
                change: change.clone(),
                // A task is never born attached to a session: handing work to a live
                // conversation is a gesture somebody makes, and `TaskNew` has no shape for it
                // for `status`' reason — a creation road that could is a road an agent could
                // take without anybody choosing.
                session: None,
                links,
                comments: Vec::new(),
                // Files, if any, arrive through `attach` once the id exists — `cmd::tasks::task_new`
                // makes the two one broadcast. (M39)
                attachments: Vec::new(),
                // Empty even when the caller named a starting status: the history records
                // *changes*, and a task born in `Doing` did not move there — `created_by` and
                // the status itself already carry that fact.
                history: Vec::new(),
                // Stamped from the caller's identity, which arrived with the connection rather
                // than in the payload — the same rule `TaskEdit::Comment` states for a comment's
                // author, and for the same reason: an agent that could name a creator could name
                // the user as one.
                created_by: author,
                created_unix_ms: now,
                updated_unix_ms: now,
            };
            // At the end: the array *is* the order, and new work goes at the bottom of the list
            // rather than jumping the queue the user is reading top to bottom.
            file.tasks.push(task.clone());
            Ok(task)
        })
    }

    /// Apply one change to one task.
    ///
    /// `author` is used by exactly one variant — [`TaskEdit::Comment`] — and it is a parameter
    /// rather than a field of that variant for the reason the variant's own doc gives: identity
    /// comes from the connection the call arrived on, never from a payload the caller composes, or
    /// an agent could sign a comment as the user.
    pub fn edit(&self, id: &TaskId, edit: TaskEdit, author: TaskAuthor) -> Result<Task> {
        let now = persist::now_ms();
        // Remembered outside the closure: the bytes go *after* the tombstone has landed, and
        // outside the lock — `update`'s closure does no I/O. See `TaskEdit::DetachAttachment`.
        let detaching = match &edit {
            TaskEdit::DetachAttachment { attachment } => Some(attachment.clone()),
            _ => None,
        };
        // A deleted comment takes its files with it: they were part of what it said, and bytes
        // nobody can reach from the panel or from `cide_task_get` are bytes in the repository
        // for no reason. Tombstoned in the closure, removed after it, like a detach.
        let deleting_comment = match &edit {
            TaskEdit::DeleteComment { id } => Some(id.clone()),
            _ => None,
        };
        let task = self.update(move |file| {
            // The two link arms route through free functions over the whole file before the
            // single-task borrow below is taken: the rules they enforce — the target's
            // existence, the cycle walk, `related`'s inverse edge — live on *other* tasks.
            // No author gate, unlike the comment arms: an edge is recorded intent, not a
            // rewrite of the record, and what keeps a run's `blockedBy` from dispatching
            // anything is `autodispatch`'s author gate downstream, not ownership here.
            let edit = match edit {
                TaskEdit::Link { link, target } => {
                    return apply_link(file, id, link, &target, now);
                }
                TaskEdit::Unlink { link, target } => {
                    return apply_unlink(file, id, link, &target, now);
                }
                other => other,
            };
            let task = find_mut(file, id).ok_or_else(|| no_such_task(id))?;
            match edit {
                TaskEdit::SetTitle { title } => task.title = title.trim().to_string(),
                TaskEdit::SetBody { body } => task.body = body,
                // Records the transition beside applying it. (M27) Only on an actual change: a
                // `SetStatus` naming the status the task already has is a no-op the panel's
                // segment can send on a double click, and a history row saying `Doing → Doing`
                // would be a log entry about nothing. `from` is read off the task here, never
                // taken from the caller; `by` is the connection's identity, exactly as a
                // comment's author is — see `TaskStatusChange`.
                TaskEdit::SetStatus { status } => {
                    if task.status != status {
                        task.history.push(TaskStatusChange {
                            from: task.status,
                            to: status,
                            by: author.clone(),
                            at_unix_ms: now,
                        });
                        task.status = status;
                    }
                }
                /*
                 * Assigning a role clears the session, and handing to a session clears the role.
                 *
                 * The invariant lives here because this is the only writer: "who is on this" is
                 * one fact carried in two shapes, and a task claiming both would be a card that
                 * cannot answer the question it exists to answer. Doing it in the store rather
                 * than at each call site is what makes it true for the panel, the MCP tools and
                 * any future caller alike.
                 */
                TaskEdit::Assign { agent } => {
                    task.agent = agent;
                    task.session = None;
                }
                TaskEdit::SetSession { session } => {
                    task.session = session;
                    task.agent = None;
                }
                // Validated here for `create`'s reason — this is the other door into the same
                // field, and a check on only one of them is a check that is not there.
                TaskEdit::SetChange { change } => {
                    task.change = validated_change(change.as_ref())?;
                }
                // **Appends.** There is no edit and no delete, here or on the wire, and that single
                // restriction is what makes the log a channel between agents rather than a
                // scratchpad — see `TaskComment`.
                TaskEdit::Comment { text } => {
                    task.comments
                        .push(new_comment(text, Vec::new(), author, now))
                }
                /*
                 * The two the **user** may do, and nobody else. (M21)
                 *
                 * The guard is on `author`, which is not a payload field — it is decided by the
                 * app from the connection the call arrived on, exactly as `Comment` decides whose
                 * name goes on a new line. That is what makes this unforgeable rather than merely
                 * checked: an agent cannot claim to be the user, because the identity it would
                 * have to claim is the one it never supplies.
                 *
                 * `remove` is the precedent — a capability the app has and no MCP tool exposes —
                 * and the reasoning is `TaskComment`'s: an agent that can rewrite a comment after
                 * the fact leaves no reader able to tell that what they are acting on is not what
                 * was written. A person with a keyboard fixing their own typo is not that.
                 */
                TaskEdit::EditComment { id, text } => {
                    if author != TaskAuthor::User {
                        return Err(agents_may_not_rewrite());
                    }
                    let comment = task
                        .comments
                        .iter_mut()
                        .find(|c| c.id == id && !c.deleted)
                        .ok_or_else(|| no_such_comment(&id))?;
                    comment.text = text;
                    comment.edited_at_unix_ms = Some(now);
                }
                TaskEdit::DeleteComment { id } => {
                    if author != TaskAuthor::User {
                        return Err(agents_may_not_rewrite());
                    }
                    let comment = task
                        .comments
                        .iter_mut()
                        .find(|c| c.id == id && !c.deleted)
                        .ok_or_else(|| no_such_comment(&id))?;
                    comment.deleted = true;
                    // The tombstone persists; the words do not. See `TaskComment::deleted`.
                    comment.text = String::new();
                    comment.edited_at_unix_ms = Some(now);
                    for attachment in &mut comment.attachments {
                        attachment.deleted = true;
                    }
                }
                // The third the user may do and nobody else, on the two arms' exact terms:
                // an attachment is part of the record, and an agent removing one leaves a
                // reader unable to tell what the comment was written with. (M39)
                TaskEdit::DetachAttachment { attachment } => {
                    if author != TaskAuthor::User {
                        return Err(agents_may_not_rewrite());
                    }
                    let record = attachment_record_mut(task, &attachment)
                        .filter(|a| !a.deleted)
                        .ok_or_else(|| no_such_attachment(&attachment))?;
                    record.deleted = true;
                }
                TaskEdit::Link { .. } | TaskEdit::Unlink { .. } => {
                    unreachable!("returned through apply_link/apply_unlink above")
                }
            }
            task.updated_unix_ms = now;
            Ok(task.clone())
        })?;
        if let Some(attachment) = detaching
            && let Some(record) = attachment_record(&task, &attachment)
        {
            attachments::remove(&self.root, id, record);
        }
        if let Some(comment) = deleting_comment
            && let Some(gone) = task.comments.iter().find(|c| c.id == comment)
        {
            for record in &gone.attachments {
                attachments::remove(&self.root, id, record);
            }
        }
        Ok(task)
    }

    /// Copy files under the task and record them where `target` says. (M39)
    ///
    /// The bytes first, the record second — a record naming a file that was never written is
    /// worse than the reverse, which is why this is not a [`TaskEdit`] arm: `update`'s closure
    /// does no I/O. The target is checked *before* the copy so a refusal — no such task, no such
    /// comment — costs nothing on disk, and a refusal from the record step removes the copies.
    ///
    /// `author` is who attached, from the connection — [`TaskEdit::Comment`]'s rule — and, for
    /// [`AttachTarget::NewComment`], who the comment is from.
    pub fn attach(
        &self,
        id: &TaskId,
        target: AttachTarget,
        sources: &[PathBuf],
        author: TaskAuthor,
    ) -> Result<Task> {
        self.check_target(id, &target)?;
        let records = attachments::import(&self.root, id, sources, &author)?;
        self.record_attachments(id, target, records, author)
    }

    /// [`Self::attach`] for bytes the caller already holds: the clipboard road, where there is
    /// no source path and writing one only to read it back would be a temp file for nothing.
    pub fn attach_bytes(
        &self,
        id: &TaskId,
        target: AttachTarget,
        name: &str,
        bytes: &[u8],
        author: TaskAuthor,
    ) -> Result<Task> {
        self.check_target(id, &target)?;
        let record = attachments::import_bytes(&self.root, id, name, bytes, &author)?;
        self.record_attachments(id, target, vec![record], author)
    }

    /// The refusals `attach` can give without touching the disk.
    fn check_target(&self, id: &TaskId, target: &AttachTarget) -> Result<()> {
        let guard = self.inner.lock();
        let task = find(&guard, id).ok_or_else(|| no_such_task(id))?;
        if let AttachTarget::Comment { id: comment } = target
            && !task.comments.iter().any(|c| c.id == *comment && !c.deleted)
        {
            return Err(no_such_comment(comment));
        }
        Ok(())
    }

    /// The record half of `attach`: land the records, or remove their bytes and say why.
    fn record_attachments(
        &self,
        id: &TaskId,
        target: AttachTarget,
        records: Vec<TaskAttachment>,
        author: TaskAuthor,
    ) -> Result<Task> {
        let now = persist::now_ms();
        let written = records.clone();
        let outcome = self.update(move |file| {
            let task = find_mut(file, id).ok_or_else(|| no_such_task(id))?;
            match target {
                AttachTarget::Task => task.attachments.extend(records),
                AttachTarget::Comment { id: comment } => {
                    let live = task
                        .comments
                        .iter_mut()
                        .find(|c| c.id == comment && !c.deleted)
                        .ok_or_else(|| no_such_comment(&comment))?;
                    live.attachments.extend(records);
                }
                // A comment may be nothing but its files — "here is the screenshot" — so the
                // text is not required to be non-empty here, unlike the MCP tool's `text`.
                AttachTarget::NewComment { text } => {
                    task.comments.push(new_comment(text, records, author, now));
                }
            }
            task.updated_unix_ms = now;
            Ok(task.clone())
        });
        if outcome.is_err() {
            for record in &written {
                attachments::remove(&self.root, id, record);
            }
        }
        outcome
    }

    /// Remove a task.
    ///
    /// The id is **not** returned to the pool — see [`TaskFile::next_id`]. Deliberately not reachable
    /// from an MCP tool either: the file is the shared record of what happened, and deletion belongs
    /// to the user.
    ///
    /// Links pointing at the removed task are left **dangling, deliberately** — see [`validate`]
    /// for why a scrub could not survive the merge and why a dangling edge is safe to keep.
    pub fn delete(&self, id: &TaskId) -> Result<()> {
        self.update(|file| {
            let before = file.tasks.len();
            file.tasks.retain(|task| &task.id != id);
            if file.tasks.len() == before {
                return Err(no_such_task(id));
            }
            Ok(())
        })
    }

    /// Write if the debounce has elapsed. Called from whatever loop the app is already ticking.
    ///
    /// Returns the merged file when an out-of-process change was folded in, so the caller can
    /// broadcast it — that is the one moment the board changes without anybody in this process
    /// having asked, and nothing else would notice.
    pub fn flush_if_due(&self) -> Option<TaskFile> {
        if self.debounce.take() {
            self.write_now()
        } else {
            None
        }
    }

    /// Write unconditionally, after reconciling with whatever is on disk.
    ///
    /// Used on quit and on project close, where waiting out a debounce would lose the last change
    /// the user or an agent made.
    ///
    /// The `inner` lock is held across the re-read, the merge and the write, and that is the point
    /// rather than an oversight: a mutation landing between the merge and the rename would be
    /// written over by bytes encoded before it existed, and it would be written over silently.
    pub fn write_now(&self) -> Option<TaskFile> {
        let mut guard = self.inner.lock();
        let mut state = self.state.lock();

        // Layer 3, before every write and not only on the debounced path: a `git pull` between two
        // explicit saves is the same race as one between two ticks.
        let merged = self.reconcile(&mut guard, &mut state);

        if let DiskState::Unreadable { error } = &*state {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "not writing the task tracker over a file this build cannot read"
            );
            // Re-arm, per `Debouncer::take`'s contract that a caller whose write fails owns the
            // retry. Without this the change waits on disk for the next unrelated mutation — and
            // for this file "unreadable" is usually a merge in progress, which ends.
            self.debounce.note_change();
            return merged;
        }

        /*
         * **An empty tracker is not written into a project that has none.** (M21)
         *
         * `write_now` runs on project close and on quit, unconditionally, which is right for a
         * tracker that has something in it and wrong for one that never did: merely opening a
         * project and closing it left `{"schemaVersion":1,"rev":0,"tasks":[]}` in somebody's
         * repository — a tracked file appearing in `git status` because of a panel they may not
         * even have looked at. That is precisely the surprise the `absent` screen's own hint is
         * written to prevent one step earlier: *"Creating the first task writes a new file, which
         * is committed with your code"*. The file is the user's to create, by creating a task.
         *
         * The two halves of the condition are both load-bearing. `tasks.is_empty()` alone would
         * refuse to persist the deletion of the last task — the file would keep the row for ever
         * and a delete would not delete. `DiskState::Absent` alone is not a state a write should
         * skip: an in-memory task with no file yet is exactly what the first flush after *New
         * task* has to create. Only together do they mean "there is nothing to record and nothing
         * on disk to keep in step with".
         *
         * Not re-armed, unlike the refusal above: that one is a write this build declines to
         * attempt and owns the retry for, this one is a write with no content. `note_change` runs
         * on the next mutation, and the next mutation is what makes the file worth having.
         */
        if guard.tasks.is_empty() && matches!(*state, DiskState::Absent) {
            return merged;
        }

        let mut bytes = match serde_json::to_vec_pretty(&*guard) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::error!(%error, "could not encode the task file");
                return merged;
            }
        };
        // `to_vec_pretty` stops at the closing brace. A committed text file without a trailing
        // newline makes `git diff` print "\ No newline at end of file" on every hunk that reaches
        // the end, for ever, in a file whose diffs people read.
        bytes.push(b'\n');

        match write_shared(&self.path, &bytes) {
            Ok(()) => {
                *state = DiskState::Ready;
                // Stamped after the rename, so the next preflight compares against what this write
                // produced. The microscopic window between the two — a third party writing inside
                // it, whose bytes we would then record as ours — is the same one
                // `document::write_if_unchanged` documents and declines to close: shutting it needs
                // a lock the rest of the system does not honour.
                *self.stamp.lock() = document::stamp_at(&self.path);
            }
            Err(error) => {
                tracing::error!(path = %self.path.display(), %error, "failed to save the task file");
                self.debounce.note_change();
            }
        }
        merged
    }

    /// The watcher's question: did the file move under us, ignoring our own writes?
    ///
    /// This is *not* [`Self::reload`], and the difference is the whole reason it exists. `reload`
    /// is the panel's Retry — an unconditional re-read-and-merge that also heals `Unreadable` —
    /// and calling it on every filesystem event would merge on every echo of our own debounced
    /// write. This wraps [`Self::reconcile`], whose stamp compare is the echo suppression: the
    /// stamp is recorded after every `write_shared`, so a notification caused by our own rename
    /// compares equal and costs one `stat`. (The microscopic stamp-races that survive resolve
    /// through `reconcile`'s `disk == *file` content check.)
    ///
    /// Returns the merged file when the board actually changed, so the caller can broadcast it —
    /// the same contract as [`Self::flush_if_due`], for the same reason: this is a moment the
    /// board changes without anybody in this process having asked.
    ///
    /// On a real change the debounce is re-armed: the merge may have kept tasks the disk copy did
    /// not have, and those must reach the file — the same convergence `write_now` performs on
    /// quit, here left to the ordinary flusher tick rather than done inline on a watcher thread.
    pub fn refresh_from_disk(&self) -> Option<TaskFile> {
        let mut guard = self.inner.lock();
        let mut state = self.state.lock();
        let merged = self.reconcile(&mut guard, &mut state);
        if merged.is_some() {
            self.debounce.note_change();
        }
        merged
    }

    /// Layer 3: re-`stat`, and reload-and-merge when the file has moved under us.
    ///
    /// Returns the merged file when the in-memory copy actually changed, and `None` when there was
    /// nothing to do — which is the overwhelmingly common case, one `stat` per write.
    fn reconcile(&self, file: &mut TaskFile, state: &mut DiskState) -> Option<TaskFile> {
        let known = *self.stamp.lock();
        let current = document::stamp_at(&self.path);

        // The second half of the condition is what lets the store heal. While the file is
        // unreadable its stamp is deliberately `None`, so an unchanged broken file compares unequal
        // and is re-read on every attempt — which means the moment the user resolves the merge
        // conflict, the next flush notices and writes. Without it, "unreadable" would be terminal
        // until something called `reload`.
        if current == known && !matches!(state, DiskState::Unreadable { .. }) {
            return None;
        }

        match read(&self.path) {
            ReadOutcome::Absent => {
                // Deleted out from under us — a `git checkout` of a branch that predates the
                // tracker, most likely. Ours is now the whole truth and the write below recreates
                // the file; there is nothing to merge with.
                *state = DiskState::Absent;
                *self.stamp.lock() = None;
                None
            }
            ReadOutcome::Ready { file: disk, stamp } => {
                *self.stamp.lock() = stamp;
                *state = DiskState::Ready;
                if disk == *file {
                    // The stamp moved but the content did not: someone rewrote the file with the
                    // same bytes, or `git checkout` restored what was already there.
                    return None;
                }

                // Counted before the merge, so the log names what actually happened rather than
                // what the result looks like afterwards.
                let adopted = disk
                    .tasks
                    .iter()
                    .filter(|task| find(file, &task.id).is_none())
                    .count();
                let kept = file
                    .tasks
                    .iter()
                    .filter(|task| find(&disk, &task.id).is_none())
                    .count();
                let out = merge(file, &disk);
                tracing::info!(
                    path = %self.path.display(),
                    adopted,
                    kept,
                    tasks = out.tasks.len(),
                    rev = out.rev,
                    "the task file changed outside this process; merged it"
                );
                *file = out.clone();
                Some(out)
            }
            // Never quarantines. The rename in `load` belongs to the deliberate "open this
            // project's tracker" moment, where the user is present and the panel is about to tell
            // them; a background flush that renamed a file in somebody's working tree while they
            // were typing would be the same surprise with nobody watching.
            ReadOutcome::Refused { error } | ReadOutcome::Unparseable { error, .. } => {
                *state = DiskState::Unreadable { error };
                *self.stamp.lock() = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::File;
    use std::sync::atomic::{AtomicU64, Ordering};

    use cide_ipc::{AgentId, TaskStatus};

    /// A scratch directory, from `std::env::temp_dir()` keyed by pid and a counter.
    ///
    /// `cide-core` builds its test directories the same way and this crate has no
    /// dev-dependencies for the reason its `Cargo.toml` records: a temp-dir crate would be the
    /// workspace's first, for something eight lines already do. The pid separates two `cargo test`
    /// runs, the counter separates the threads of one — `cargo test` is parallel by default, and a
    /// shared scratch path is not a flaky test but a wrong one.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "cide-tasks-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn root(&self) -> &Path {
            &self.0
        }

        fn tasks(&self) -> PathBuf {
            tasks_path(&self.0)
        }

        /// Put bytes at `.cide/tasks.json` as if git or a teammate had.
        fn plant(&self, contents: &str) {
            let path = self.tasks();
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir .cide");
            fs::write(&path, contents).expect("plant");
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.0.join(".cide"))
                .expect("read .cide")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn a_task(id: &str, title: &str, updated: u64) -> Task {
        Task {
            id: TaskId(id.to_string()),
            title: title.to_string(),
            body: String::new(),
            status: TaskStatus::Todo,
            agent: None,
            comments: Vec::new(),
            change: None,
            links: Vec::new(),
            session: None,
            attachments: Vec::new(),
            history: Vec::new(),
            created_by: TaskAuthor::User,
            created_unix_ms: 1_000,
            updated_unix_ms: updated,
        }
    }

    /// An attachment record with a fixed id, so two calls describe the *same* attachment —
    /// which is what the union tests are about.
    fn an_attachment(id: &str, name: &str, at: u64) -> TaskAttachment {
        TaskAttachment {
            id: TaskAttachmentId(id.to_string()),
            name: name.to_string(),
            bytes: 10,
            kind: cide_ipc::AttachmentKind::File,
            added_by: TaskAuthor::User,
            added_unix_ms: at,
            deleted: false,
        }
    }

    /// A comment carrying the id `repair` would derive for it.
    ///
    /// Derived rather than minted, so two calls with the same `(text, at)` produce the *same*
    /// comment — which is what the union tests are about, and what a fresh uuid per call would
    /// quietly stop being true of.
    fn a_comment(text: &str, at: u64) -> TaskComment {
        let author = TaskAuthor::Agent {
            agent: AgentId("developer".into()),
            label: "Developer".into(),
        };
        TaskComment {
            id: TaskComment::legacy_id(&author, at, text),
            author,
            text: text.to_string(),
            at_unix_ms: at,
            edited_at_unix_ms: None,
            deleted: false,
            attachments: Vec::new(),
        }
    }

    fn a_file(rev: u64, tasks: Vec<Task>) -> TaskFile {
        let mut file = TaskFile {
            schema_version: TaskFile::CURRENT_SCHEMA,
            rev,
            next_id: 0,
            tasks,
        };
        settle_next_id(&mut file);
        file
    }

    fn titles(file: &TaskFile) -> Vec<&str> {
        file.tasks.iter().map(|t| t.title.as_str()).collect()
    }

    fn ids(file: &TaskFile) -> Vec<&str> {
        file.tasks.iter().map(|t| t.id.as_str()).collect()
    }

    // --- layer 3: the merge, one documented row per rule -------------------------------------

    /// One row of the merge table.
    ///
    /// `why` is not decoration: each row exists because a specific alternative implementation
    /// would produce a specific wrong answer, and a row whose name and reason are not written down
    /// is one a later "simplification" deletes as redundant.
    struct Case {
        name: &'static str,
        why: &'static str,
        mine: TaskFile,
        theirs: TaskFile,
        check: fn(&TaskFile),
    }

    // --- the user may edit and delete; agents may not (M21) ----------------------------------

    /// The two merges that made ids necessary, each of which used to lose the user's change.
    #[test]
    fn an_edit_and_a_delete_both_survive_a_merge_with_a_stale_file() {
        let original = a_comment("worktree merged clean", 10);

        // The stale copy on disk still has the comment exactly as it was written.
        let stale = a_file(1, vec![with_comments("t-1", 1, vec![original.clone()])]);

        // Ours has it edited.
        let mut edited = original.clone();
        edited.text = "worktree merged cleanly".into();
        edited.edited_at_unix_ms = Some(20);
        let mine = a_file(2, vec![with_comments("t-1", 1, vec![edited.clone()])]);

        let merged = merge(&mine, &stale);
        let comments = &merged.tasks[0].comments;
        assert_eq!(
            comments.len(),
            1,
            "one comment, not two. Under structural equality the stale copy's original text was \
             a different comment and both survived — the edit appeared to duplicate the line"
        );
        assert_eq!(comments[0].text, "worktree merged cleanly");
        assert_eq!(
            comments[0].edited_at_unix_ms,
            Some(20),
            "and the newer edit won, whichever side it came from"
        );

        // Now the delete, against the same stale file.
        let mut gone = original.clone();
        gone.deleted = true;
        gone.text = String::new();
        gone.edited_at_unix_ms = Some(30);
        let after = merge(
            &a_file(3, vec![with_comments("t-1", 1, vec![gone])]),
            &stale,
        );
        let comments = &after.tasks[0].comments;
        assert_eq!(comments.len(), 1, "the tombstone is kept, not the comment");
        assert!(
            comments[0].deleted,
            "deleted wins over a stale copy that still has it. Removing the entry instead would \
             let the very next merge put the comment back, which is a delete that does not delete"
        );
        assert_eq!(comments[0].text, "", "and the words go with it");
    }

    /// Deleted wins in **both** directions, which is what makes the rule ordering-free.
    #[test]
    fn a_tombstone_wins_whichever_side_it_is_on() {
        let live = a_comment("note", 10);
        let mut dead = live.clone();
        dead.deleted = true;
        dead.text = String::new();

        for (mine, theirs) in [(&live, &dead), (&dead, &live)] {
            let merged = merge(
                &a_file(1, vec![with_comments("t-1", 1, vec![mine.clone()])]),
                &a_file(1, vec![with_comments("t-1", 1, vec![theirs.clone()])]),
            );
            assert!(
                merged.tasks[0].comments[0].deleted,
                "`deleted` only ever goes false to true, so neither side can un-delete"
            );
        }
    }

    /// A file written before ids existed reads back with stable, derived ones.
    #[test]
    fn legacy_comments_get_the_same_id_from_every_reader() {
        let author = TaskAuthor::Orchestrator;
        let raw = TaskComment {
            id: CommentId::default(),
            author: author.clone(),
            text: "written before ids".into(),
            at_unix_ms: 42,
            edited_at_unix_ms: None,
            deleted: false,
            attachments: Vec::new(),
        };
        let mut a = a_file(1, vec![with_comments("t-1", 1, vec![raw.clone()])]);
        let mut b = a_file(1, vec![with_comments("t-1", 1, vec![raw])]);
        repair(&mut a);
        repair(&mut b);

        let id = &a.tasks[0].comments[0].id;
        assert!(!id.is_empty(), "repair fills in every empty id");
        assert_eq!(
            id, &b.tasks[0].comments[0].id,
            "and derives the SAME one in two processes. A minted uuid here would give each \
             reader a different id, which is a merge that duplicates every comment it touches — \
             the bug ids were added to prevent, reintroduced by the fix for it"
        );
        assert_eq!(
            id,
            &TaskComment::legacy_id(&author, 42, "written before ids"),
            "…derived from the OLD merge key, so two legacy comments collide exactly when \
             structural equality already called them one"
        );

        // And the merge is unchanged for a file that never had ids.
        let merged = merge(&a, &b);
        assert_eq!(
            merged.tasks[0].comments.len(),
            1,
            "two readings of one legacy file merge to one comment, as they always did"
        );
    }

    #[test]
    fn only_the_user_may_edit_or_delete_a_comment() {
        let dir = TempDir::new("comment-authority");
        let store = TaskStore::open(&dir.0);
        let task = store
            .create(
                &TaskNew {
                    project: cide_ipc::ProjectId::new(),
                    title: "t".into(),
                    body: None,
                    agent: None,
                    status: None,
                    change: None,
                    links: None,
                    attachments: None,
                },
                TaskAuthor::User,
            )
            .expect("create");
        let posted = store
            .edit(
                &task.id,
                TaskEdit::Comment {
                    text: "from the developer".into(),
                },
                TaskAuthor::Agent {
                    agent: AgentId("developer".into()),
                    label: "Developer".into(),
                },
            )
            .expect("comment");
        let id = posted.comments.last().expect("a comment").id.clone();

        for author in [
            TaskAuthor::Orchestrator,
            TaskAuthor::Agent {
                agent: AgentId("developer".into()),
                label: "Developer".into(),
            },
        ] {
            assert!(
                store
                    .edit(
                        &task.id,
                        TaskEdit::EditComment {
                            id: id.clone(),
                            text: "quietly rewritten".into()
                        },
                        author.clone()
                    )
                    .is_err(),
                "an agent cannot rewrite a comment — including one it wrote itself. The author \
                 comes from the connection, so this is unforgeable rather than merely checked"
            );
            assert!(
                store
                    .edit(
                        &task.id,
                        TaskEdit::DeleteComment { id: id.clone() },
                        author.clone()
                    )
                    .is_err(),
                "…and cannot delete one either"
            );
        }

        // The user may do both, including to the agent's comment — which is the whole point of
        // the boundary being the caller rather than the comment's author.
        let edited = store
            .edit(
                &task.id,
                TaskEdit::EditComment {
                    id: id.clone(),
                    text: "from the developer (tidied)".into(),
                },
                TaskAuthor::User,
            )
            .expect("the user may edit an agent's comment");
        let c = edited
            .comments
            .iter()
            .find(|c| c.id == id)
            .expect("still there");
        assert_eq!(c.text, "from the developer (tidied)");
        assert!(
            c.edited_at_unix_ms.is_some(),
            "and it is stamped as edited. A user rewriting an AGENT's comment is the one case \
             that could still mislead a later reader, so the mark is what keeps the log honest"
        );

        let after = store
            .edit(
                &task.id,
                TaskEdit::DeleteComment { id: id.clone() },
                TaskAuthor::User,
            )
            .expect("the user may delete");
        let c = after
            .comments
            .iter()
            .find(|c| c.id == id)
            .expect("tombstoned, not removed");
        assert!(c.deleted && c.text.is_empty());

        assert!(
            store
                .edit(&task.id, TaskEdit::DeleteComment { id }, TaskAuthor::User)
                .is_err(),
            "and a second delete of the same comment is refused rather than silently accepted"
        );
    }

    #[test]
    fn merge_table() {
        let cases = vec![
            Case {
                name: "a task changed on both sides keeps the newer side's fields",
                why: "the rule that makes concurrent edits converge: without it the answer would \
                      depend on which side happened to be read first",
                mine: a_file(4, vec![a_task("t-1", "mine, newer", 200)]),
                theirs: a_file(4, vec![a_task("t-1", "theirs, older", 100)]),
                check: |out| assert_eq!(titles(out), ["mine, newer"]),
            },
            Case {
                name: "…and symmetrically, when disk is the newer side",
                why: "the same rule with the arguments swapped; an implementation that always \
                      preferred the in-memory copy would pass the row above and fail this one",
                mine: a_file(4, vec![a_task("t-1", "mine, older", 100)]),
                theirs: a_file(4, vec![a_task("t-1", "theirs, newer", 200)]),
                check: |out| assert_eq!(titles(out), ["theirs, newer"]),
            },
            Case {
                name: "a task present only on disk is adopted",
                why: "a `git pull` added it. Last-writer-wins on the whole file would delete it, \
                      which is the failure this merge exists to prevent",
                mine: a_file(4, vec![a_task("t-1", "ours", 100)]),
                theirs: a_file(
                    9,
                    vec![a_task("t-1", "ours", 100), a_task("t-2", "pulled", 50)],
                ),
                check: |out| assert_eq!(titles(out), ["ours", "pulled"]),
            },
            Case {
                name: "a task present only in memory is kept",
                why: "this process created it since the last read. Refusing the write, or taking \
                      the disk copy wholesale, would silently discard it",
                mine: a_file(
                    4,
                    vec![
                        a_task("t-1", "ours", 100),
                        a_task("t-9", "just created", 900),
                    ],
                ),
                theirs: a_file(9, vec![a_task("t-1", "ours", 100)]),
                check: |out| assert_eq!(titles(out), ["ours", "just created"]),
            },
            Case {
                name: "an identical comment on both sides appears once",
                why: "the union rule. `TaskComment` has no id, so identity is `(author, \
                      at_unix_ms, text)` — and appending unconditionally would re-duplicate the \
                      duplicate on every later merge",
                mine: a_file(
                    4,
                    vec![with_comments(
                        "t-1",
                        100,
                        vec![a_comment("merged clean", 50)],
                    )],
                ),
                theirs: a_file(
                    4,
                    vec![with_comments(
                        "t-1",
                        100,
                        vec![a_comment("merged clean", 50)],
                    )],
                ),
                check: |out| assert_eq!(out.tasks[0].comments.len(), 1),
            },
            Case {
                name: "a different comment added on each side: both survive",
                why: "the case the whole merge exists for. A subagent's comment lands while the \
                      orchestrator is writing its own, and losing either one loses the message \
                      this feature is built to carry",
                mine: a_file(
                    4,
                    vec![with_comments(
                        "t-1",
                        300,
                        vec![a_comment("from memory", 300)],
                    )],
                ),
                theirs: a_file(
                    4,
                    vec![with_comments("t-1", 200, vec![a_comment("from disk", 200)])],
                ),
                check: |out| {
                    let texts: Vec<&str> = out.tasks[0]
                        .comments
                        .iter()
                        .map(|c| c.text.as_str())
                        .collect();
                    // Oldest first, which is the order the panel renders and an agent reads —
                    // so the comment from the *losing* side still sorts into its right place.
                    assert_eq!(texts, ["from disk", "from memory"]);
                },
            },
            Case {
                name: "both sides edited the same task: newer fields, unioned comments",
                why: "a status change and a comment are different kinds of edit; taking the whole \
                      winning task would drop the loser's comment",
                mine: a_file(
                    4,
                    vec![Task {
                        status: TaskStatus::Review,
                        ..with_comments("t-1", 400, vec![a_comment("ready for review", 400)])
                    }],
                ),
                theirs: a_file(
                    7,
                    vec![Task {
                        status: TaskStatus::Doing,
                        ..with_comments("t-1", 200, vec![a_comment("picked it up", 200)])
                    }],
                ),
                check: |out| {
                    assert_eq!(
                        out.tasks[0].status,
                        TaskStatus::Review,
                        "the newer side's status"
                    );
                    assert_eq!(out.tasks[0].comments.len(), 2, "both comments");
                },
            },
            Case {
                name: "one side deleted a task the other edited: the task survives, with the edit",
                why: "nothing in the file records a deletion, so 'only in memory' is ambiguous \
                      between created-here and deleted-there. Both resolve by keeping the task: a \
                      resurrected task is one gesture to repeat, a lost task is work nobody knows \
                      was lost",
                mine: a_file(
                    4,
                    vec![
                        a_task("t-1", "kept", 100),
                        a_task("t-2", "edited here", 500),
                    ],
                ),
                theirs: a_file(9, vec![a_task("t-1", "kept", 100)]),
                check: |out| assert_eq!(titles(out), ["kept", "edited here"]),
            },
            Case {
                name: "a recorded creator beats the `User` an absent field decays to",
                why: "a task is created once, so the sides can only disagree because one of them                       passed through a build that did not know `created_by` — and that decays to                       `User`. Taking the winner's copy would credit the user with an agent's work                       every time the older build happened to hold the newer task",
                mine: a_file(4, vec![a_task("t-1", "mine, newer", 200)]),
                theirs: a_file(
                    4,
                    vec![Task {
                        created_by: TaskAuthor::Orchestrator,
                        ..a_task("t-1", "theirs, older", 100)
                    }],
                ),
                check: |out| {
                    assert_eq!(
                        titles(out),
                        ["mine, newer"],
                        "the scalar fields still follow the newer side"
                    );
                    assert_eq!(out.tasks[0].created_by, TaskAuthor::Orchestrator);
                },
            },
            Case {
                name: "…and it does not matter which side the decay is on",
                why: "the rule is directional rather than a tie-break, so swapping the arguments                       must give the same answer; an implementation that read only `mine` would                       pass the row above and fail this one",
                mine: a_file(
                    4,
                    vec![Task {
                        created_by: TaskAuthor::Orchestrator,
                        ..a_task("t-1", "mine, older", 100)
                    }],
                ),
                theirs: a_file(4, vec![a_task("t-1", "theirs, newer", 200)]),
                check: |out| {
                    assert_eq!(titles(out), ["theirs, newer"]);
                    assert_eq!(out.tasks[0].created_by, TaskAuthor::Orchestrator);
                },
            },
            Case {
                name: "an unlink survives a merge with a stale file, even when the stale side \
                       wins the scalars",
                why: "links merge per (link, target) key by stamp, never with the scalar fields: \
                      side A unlinked at 50, side B merely commented at 60. Whole-vector \
                      last-writer-wins would take B's links wholesale and resurrect the edge — \
                      and a resurrected blockedBy silently re-gates dispatch. A subagent \
                      commenting is the most common write in the file, so this is the ordinary \
                      afternoon, not a corner",
                mine: a_file(
                    4,
                    vec![with_links(
                        "t-1",
                        50,
                        vec![an_edge(LinkType::BlockedBy, "t-2", true, 50)],
                    )],
                ),
                theirs: a_file(
                    4,
                    vec![Task {
                        comments: vec![a_comment("stale side comments", 60)],
                        ..with_links(
                            "t-1",
                            60,
                            vec![an_edge(LinkType::BlockedBy, "t-2", false, 10)],
                        )
                    }],
                ),
                check: |out| {
                    let task = &out.tasks[0];
                    assert_eq!(task.comments.len(), 1, "the stale side's comment is kept");
                    assert!(
                        task.links[0].deleted,
                        "and the newer unlink still wins its own key"
                    );
                },
            },
            Case {
                name: "…and a re-link beats the unlink it reversed, because the tombstone is a \
                       toggle",
                why: "a link's identity is its (link, target) key, so re-linking recreates the \
                      same key. The comments' deleted-wins-for-ever rule here would make every \
                      unlink permanent after any merge with a stale file",
                mine: a_file(
                    4,
                    vec![with_links(
                        "t-1",
                        70,
                        vec![an_edge(LinkType::Related, "t-2", false, 70)],
                    )],
                ),
                theirs: a_file(
                    4,
                    vec![with_links(
                        "t-1",
                        50,
                        vec![an_edge(LinkType::Related, "t-2", true, 50)],
                    )],
                ),
                check: |out| assert!(!out.tasks[0].links[0].deleted),
            },
            Case {
                name: "a link added on each side: both survive",
                why: "the union half of the rule — two windows each recording one edge in the \
                      same debounce window must lose neither, which is the comments' oldest \
                      argument applied to edges",
                mine: a_file(
                    4,
                    vec![with_links(
                        "t-1",
                        300,
                        vec![an_edge(LinkType::Related, "t-2", false, 300)],
                    )],
                ),
                theirs: a_file(
                    4,
                    vec![with_links(
                        "t-1",
                        200,
                        vec![an_edge(LinkType::BlockedBy, "t-3", false, 200)],
                    )],
                ),
                check: |out| assert_eq!(out.tasks[0].links.len(), 2),
            },
        ];

        for case in cases {
            let out = merge(&case.mine, &case.theirs);
            (case.check)(&out);
            assert_eq!(
                out.rev,
                case.mine.rev.max(case.theirs.rev) + 1,
                "{}: rev must land past both sides, or a window holding either one keeps its \
                 stale snapshot ({})",
                case.name,
                case.why
            );
            assert_eq!(
                out.schema_version,
                TaskFile::CURRENT_SCHEMA,
                "{}: the merge writes this build's schema",
                case.name
            );
            assert!(
                validate(&out).is_ok(),
                "{}: a merge must never produce a file this build refuses to write ({})",
                case.name,
                case.why
            );
        }
    }

    fn with_comments(id: &str, updated: u64, comments: Vec<TaskComment>) -> Task {
        Task {
            comments,
            ..a_task(id, "a task", updated)
        }
    }

    fn with_links(id: &str, updated: u64, links: Vec<TaskLink>) -> Task {
        Task {
            links,
            ..a_task(id, "a task", updated)
        }
    }

    fn an_edge(link: LinkType, target: &str, deleted: bool, at: u64) -> TaskLink {
        TaskLink {
            link,
            target: TaskId(target.to_string()),
            deleted,
            at_unix_ms: at,
        }
    }

    /// The merge runs again on every out-of-process write, so it must be a fixed point: merging a
    /// result back against either input may only move `rev`.
    ///
    /// Not a hypothetical. A `git pull` on a busy afternoon fires this repeatedly, and a merge
    /// that duplicated one comment per pass would turn a five-line log into fifty.
    #[test]
    fn merging_twice_changes_nothing_but_the_rev() {
        let mine = a_file(
            4,
            vec![with_comments("t-1", 300, vec![a_comment("a", 300)])],
        );
        let theirs = a_file(
            6,
            vec![with_comments("t-1", 200, vec![a_comment("b", 200)])],
        );

        let once = merge(&mine, &theirs);
        let twice = merge(&once, &theirs);
        assert_eq!(
            once.tasks, twice.tasks,
            "the merge is idempotent over content"
        );
        assert!(twice.rev > once.rev);
    }

    /// The in-memory order survives, because the user is looking at it.
    #[test]
    fn the_merge_keeps_our_order_and_appends_what_it_adopted() {
        let mine = a_file(
            1,
            vec![a_task("t-3", "third", 10), a_task("t-1", "first", 10)],
        );
        let theirs = a_file(
            1,
            vec![a_task("t-1", "first", 10), a_task("t-7", "pulled", 10)],
        );
        assert_eq!(ids(&merge(&mine, &theirs)), ["t-3", "t-1", "t-7"]);
    }

    // --- layer 2: reading, repairing, never failing -------------------------------------------

    #[test]
    fn a_project_with_no_tracker_is_absent_not_empty() {
        let dir = TempDir::new("absent");
        let store = TaskStore::open(dir.root());
        // The distinction `TaskBoard` exists for: three sentences, three sets of buttons.
        assert!(matches!(store.board(), TaskBoard::Absent { .. }));
        assert!(store.list().is_empty());
    }

    #[test]
    fn garbage_is_moved_aside_under_persists_own_naming_scheme() {
        let dir = TempDir::new("garbage");
        dir.plant("this is not json {{{");

        let store = TaskStore::open(dir.root());
        match store.board() {
            TaskBoard::Unreadable { error, .. } => {
                assert!(
                    error.contains("moved aside"),
                    "the user is told where their bytes went: {error}"
                );
            }
            other => panic!("expected Unreadable, got {other:?}"),
        }
        // The name a user who has met `workspace.corrupt-1.json` already knows to look for.
        assert_eq!(dir.entries(), ["tasks.corrupt-1.json"]);

        // And the bytes are intact, not merely renamed out of the way.
        assert_eq!(
            fs::read_to_string(dir.root().join(".cide/tasks.corrupt-1.json")).expect("read"),
            "this is not json {{{"
        );
    }

    /// The one deliberate departure from `persist::load`, and the reason `TaskBoard::Unreadable`'s
    /// doc names merge conflicts by name: this file lives in a git working tree.
    #[test]
    fn a_file_mid_merge_is_left_exactly_where_git_put_it() {
        let dir = TempDir::new("conflict");
        let conflicted =
            "{\n<<<<<<< HEAD\n  \"rev\": 4\n=======\n  \"rev\": 9\n>>>>>>> theirs\n}\n";
        dir.plant(conflicted);

        let store = TaskStore::open(dir.root());
        assert!(matches!(store.board(), TaskBoard::Unreadable { .. }));
        assert_eq!(dir.entries(), ["tasks.json"], "nothing was renamed");
        assert_eq!(fs::read_to_string(dir.tasks()).expect("read"), conflicted);

        // And nothing may write over it — not the panel, and not an MCP tool call from an agent
        // that never sees the panel.
        let refused = store.create(&new_task("anything"), TaskAuthor::User);
        assert!(
            refused.is_err(),
            "a write over a conflicted tracker was accepted"
        );
        store.write_now();
        assert_eq!(fs::read_to_string(dir.tasks()).expect("read"), conflicted);
    }

    /// A teammate on a newer build. Downgrading their file would drop every field this build does
    /// not know, in their repository, silently.
    #[test]
    fn a_schema_from_the_future_is_refused_rather_than_guessed_at() {
        let dir = TempDir::new("future");
        let newer = r#"{"schemaVersion":99,"rev":3,"tasks":[],"sprints":[]}"#;
        dir.plant(newer);

        let store = TaskStore::open(dir.root());
        match store.board() {
            TaskBoard::Unreadable { error, .. } => assert!(error.contains("99"), "{error}"),
            other => panic!("expected Unreadable, got {other:?}"),
        }
        assert_eq!(
            dir.entries(),
            ["tasks.json"],
            "a newer file is never renamed"
        );
        assert_eq!(fs::read_to_string(dir.tasks()).expect("read"), newer);
    }

    #[test]
    fn a_duplicate_id_keeps_the_first_and_the_file_still_validates() {
        let dir = TempDir::new("dupe");
        dir.plant(
            r#"{"schemaVersion":1,"rev":2,"tasks":[
                {"id":"t-1","title":"first","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":1,"updatedUnixMs":1},
                {"id":"t-1","title":"second","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":2,"updatedUnixMs":2}]}"#,
        );

        let store = TaskStore::open(dir.root());
        assert_eq!(titles(&store.snapshot()), ["first"]);
        assert!(validate(&store.snapshot()).is_ok());
    }

    /// Both repairs that rewrite rather than drop, in one file. Neither loses a task.
    #[test]
    fn an_unusable_id_is_reminted_and_an_empty_title_gets_a_placeholder() {
        let dir = TempDir::new("repair");
        dir.plant(
            r#"{"schemaVersion":1,"rev":5,"tasks":[
                {"id":"t 5","title":"typed by hand","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":1,"updatedUnixMs":1},
                {"id":"t-2","title":"  ","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":2,"updatedUnixMs":2}]}"#,
        );

        let file = TaskStore::open(dir.root()).snapshot();
        assert_eq!(file.tasks.len(), 2, "a repair must not lose a task");
        assert!(well_formed_id(&file.tasks[0].id), "{}", file.tasks[0].id);
        // Settled past `rev` (5) as well as past `t-2`, so the re-mint is `t-6` — and the counter
        // has moved past it, because a re-mint is a mint.
        assert_eq!(file.tasks[0].id.as_str(), "t-6");
        assert_eq!(file.next_id, 7);
        assert_eq!(file.tasks[1].title, "(untitled)");
        // The whole point of repairing: `update` validates and rolls back, so a file that stayed
        // invalid would refuse every later edit with nothing on screen saying why.
        assert!(validate(&file).is_ok());
    }

    // --- layer 1: the operations ---------------------------------------------------------------

    #[test]
    fn a_change_name_that_is_not_openspecs_kebab_is_refused() {
        // The grammar is `dist/core/id.js`'s KEBAB_ID_REGEX in the installed CLI, restated.
        for good in ["add-dark-mode", "auth", "m28", "a-b-c", "v2-cache"] {
            assert!(valid_change_name(good).is_ok(), "{good} is a legal name");
        }
        // The four that matter, and the first two are the reason this is a refusal and not a
        // sanitisation: the value is joined onto `openspec/changes/`.
        for bad in [
            "../../etc/passwd",
            "changes/nested",
            "Add-Dark-Mode",
            "add--dark",
            "-leading",
            "trailing-",
            "has space",
            "",
        ] {
            assert!(valid_change_name(bad).is_err(), "{bad} must be refused");
        }
        assert!(valid_change_name(&"a".repeat(CHANGE_NAME_MAX)).is_ok());
        assert!(valid_change_name(&"a".repeat(CHANGE_NAME_MAX + 1)).is_err());
    }

    #[test]
    fn a_blank_change_collapses_to_unlinked_rather_than_to_an_empty_directory_name() {
        // A `<select>` has no null, so its empty option arrives as "". Writing that through as
        // `Some("")` would put an empty directory name in a committed file that no board could
        // ever match — `assigneeFromDraft`'s rule, one layer down.
        assert_eq!(
            validated_change(Some(&ChangeName("".into()))).unwrap(),
            None
        );
        assert_eq!(
            validated_change(Some(&ChangeName("   ".into()))).unwrap(),
            None
        );
        assert_eq!(validated_change(None).unwrap(), None);
        assert_eq!(
            validated_change(Some(&ChangeName("  add-dark-mode  ".into()))).unwrap(),
            Some(ChangeName("add-dark-mode".into())),
            "trimmed, so a name pasted with a trailing space is not refused for a character \
             nobody can see"
        );
    }

    #[test]
    fn the_change_link_is_written_read_back_and_can_be_unlinked() {
        let dir = TempDir::new("change-link");
        let store = TaskStore::open(&dir.0);
        let mut req = new_task("Add dark mode");
        req.change = Some(ChangeName("add-dark-mode".into()));
        let task = store.create(&req, TaskAuthor::User).expect("created");
        assert_eq!(task.change, Some(ChangeName("add-dark-mode".into())));

        let unlinked = store
            .edit(
                &task.id,
                TaskEdit::SetChange { change: None },
                TaskAuthor::User,
            )
            .expect("unlinked");
        assert_eq!(
            unlinked.change, None,
            "null unlinks — a change that was proposed and abandoned must not need the task \
             deleted and retyped"
        );

        // And a refusal is a refusal, not a silently rewritten value.
        assert!(
            store
                .edit(
                    &task.id,
                    TaskEdit::SetChange {
                        change: Some(ChangeName("../escape".into())),
                    },
                    TaskAuthor::User,
                )
                .is_err()
        );
        assert_eq!(
            store.get(&task.id).expect("still there").change,
            None,
            "and the refused write left the field where it was"
        );
    }

    #[test]
    fn a_task_file_written_before_the_change_link_still_parses() {
        // `.cide/tasks.json` is committed. A build that refused last week's file over a field it
        // added would be the tracker locking the team out of its own repository.
        let json = r#"{"schemaVersion":1,"rev":3,"tasks":[{"id":"t-1","title":"old",
            "body":"","status":"todo","agent":null,"comments":[],
            "createdUnixMs":1,"updatedUnixMs":1}]}"#;
        let file: TaskFile = serde_json::from_str(json).expect("an older file still parses");
        assert_eq!(file.tasks[0].change, None);
        assert!(
            file.tasks[0].links.is_empty(),
            "and no links, same posture (M30)"
        );
    }

    // --- typed links (M30) ---------------------------------------------------------------------

    #[test]
    fn the_link_wire_spelling_is_serdes() {
        // `link_wire` is a match, not a serde round trip, so it *could* drift; this is the pin.
        // A refusal that spells a kind differently from the wire teaches the model that reads it
        // the wrong vocabulary for its next call.
        for kind in [LinkType::Related, LinkType::BlockedBy, LinkType::SubtaskOf] {
            let wire = serde_json::to_value(kind).expect("serialize");
            assert_eq!(wire.as_str().expect("a string"), link_wire(kind));
        }
    }

    #[test]
    fn a_link_is_written_read_back_unlinked_and_relinked() {
        let dir = TempDir::new("links");
        let store = TaskStore::open(&dir.0);
        let blocker = store
            .create(&new_task("the loader"), TaskAuthor::User)
            .expect("create");
        let blocked = store
            .create(&new_task("the panel"), TaskAuthor::User)
            .expect("create");

        let linked = store
            .edit(
                &blocked.id,
                TaskEdit::Link {
                    link: LinkType::BlockedBy,
                    target: blocker.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect("linked");
        assert_eq!(linked.links.len(), 1);
        assert!(!linked.links[0].deleted);
        assert_eq!(linked.links[0].target, blocker.id);

        let unlinked = store
            .edit(
                &blocked.id,
                TaskEdit::Unlink {
                    link: LinkType::BlockedBy,
                    target: blocker.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect("unlinked");
        assert_eq!(
            unlinked.links.len(),
            1,
            "an unlink tombstones rather than removes — a removed edge is one the next merge \
             with a stale file puts back"
        );
        assert!(unlinked.links[0].deleted);

        let relinked = store
            .edit(
                &blocked.id,
                TaskEdit::Link {
                    link: LinkType::BlockedBy,
                    target: blocker.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect("relinked");
        assert_eq!(
            relinked.links.len(),
            1,
            "a re-link revives the tombstoned key rather than duplicating it — one entry per \
             (link, target) is the invariant the merge's per-key resolution rests on"
        );
        assert!(!relinked.links[0].deleted);
    }

    #[test]
    fn a_related_link_can_be_unlinked_from_either_end() {
        let dir = TempDir::new("related-ends");
        let store = TaskStore::open(&dir.0);
        let a = store
            .create(&new_task("a"), TaskAuthor::User)
            .expect("create");
        let b = store
            .create(&new_task("b"), TaskAuthor::User)
            .expect("create");

        store
            .edit(
                &a.id,
                TaskEdit::Link {
                    link: LinkType::Related,
                    target: b.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect("linked");
        // The gesture names the pair from the *other* end than the one that stored it.
        store
            .edit(
                &b.id,
                TaskEdit::Unlink {
                    link: LinkType::Related,
                    target: a.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect("unlink from the end that does not hold the edge");
        let holder = store.get(&a.id).expect("still there");
        assert!(
            holder.links[0].deleted,
            "the edge was tombstoned where it lives, not where it was named"
        );
    }

    #[test]
    fn a_self_link_a_duplicate_and_a_missing_target_are_refused_at_the_gesture() {
        let dir = TempDir::new("link-refusals");
        let store = TaskStore::open(&dir.0);
        let a = store
            .create(&new_task("a"), TaskAuthor::User)
            .expect("create");
        let b = store
            .create(&new_task("b"), TaskAuthor::User)
            .expect("create");

        let link = |from: &TaskId, kind: LinkType, to: &TaskId| {
            store.edit(
                from,
                TaskEdit::Link {
                    link: kind,
                    target: to.clone(),
                },
                TaskAuthor::User,
            )
        };

        assert!(link(&a.id, LinkType::Related, &a.id).is_err(), "self-link");
        assert!(
            link(&a.id, LinkType::BlockedBy, &TaskId("t-99".into())).is_err(),
            "a gesture never creates a dangling edge; only a later delete or a merge can"
        );

        link(&a.id, LinkType::Related, &b.id).expect("first link");
        assert!(
            link(&a.id, LinkType::Related, &b.id).is_err(),
            "a live duplicate is refused"
        );
        assert!(
            link(&b.id, LinkType::Related, &a.id).is_err(),
            "…and for related, in either direction: the pair is one fact, wherever it is stored"
        );

        // Unlinking what is not there is a sentence, and one sentence for never-existed and
        // already-tombstoned alike — `no_such_comment`'s posture.
        let err = store
            .edit(
                &a.id,
                TaskEdit::Unlink {
                    link: LinkType::SubtaskOf,
                    target: b.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect_err("nothing to unlink");
        assert!(
            err.to_string().contains("subtaskOf"),
            "the refusal spells the wire vocabulary: {err}"
        );
    }

    #[test]
    fn a_blocking_cycle_is_refused_at_the_edit_and_names_the_chain() {
        let dir = TempDir::new("cycle");
        let store = TaskStore::open(&dir.0);
        let t1 = store
            .create(&new_task("one"), TaskAuthor::User)
            .expect("create");
        let t2 = store
            .create(&new_task("two"), TaskAuthor::User)
            .expect("create");
        let t3 = store
            .create(&new_task("three"), TaskAuthor::User)
            .expect("create");

        let block = |from: &TaskId, on: &TaskId| {
            store.edit(
                from,
                TaskEdit::Link {
                    link: LinkType::BlockedBy,
                    target: on.clone(),
                },
                TaskAuthor::User,
            )
        };
        block(&t1.id, &t2.id).expect("t-1 waits on t-2");
        block(&t2.id, &t3.id).expect("t-2 waits on t-3");

        let err = block(&t3.id, &t1.id).expect_err("would close the cycle");
        assert!(
            err.to_string().contains(&format!("via {}", t2.id)),
            "the refusal names the chain, because the caller cannot see it: {err}"
        );

        // Related never cycles — it gates nothing, so the same shape is legal.
        store
            .edit(
                &t3.id,
                TaskEdit::Link {
                    link: LinkType::Related,
                    target: t1.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect("related is not a dependency");
    }

    #[test]
    fn a_merged_or_hand_edited_cycle_does_not_freeze_the_tracker() {
        // `validate` deliberately has no cycle rule: a merge can lawfully union two acyclic
        // sides into a cycle, and `reconcile` swaps merge output in without re-validating — a
        // rule a merge can break would make every later mutation roll back with nothing on
        // screen saying why. This plants the cycle a merge would have produced and proves an
        // unrelated edit still lands.
        let dir = TempDir::new("cycle-tolerated");
        dir.plant(
            r#"{"schemaVersion":1,"rev":5,"tasks":[
                {"id":"t-1","title":"one","body":"","status":"todo","agent":null,
                 "links":[{"link":"blockedBy","target":"t-2","atUnixMs":10}],
                 "comments":[],"createdUnixMs":1,"updatedUnixMs":1},
                {"id":"t-2","title":"two","body":"","status":"todo","agent":null,
                 "links":[{"link":"blockedBy","target":"t-1","atUnixMs":10}],
                 "comments":[],"createdUnixMs":2,"updatedUnixMs":2}]}"#,
        );
        let store = TaskStore::open(dir.root());
        store
            .edit(
                &TaskId("t-1".into()),
                TaskEdit::SetTitle {
                    title: "still editable".into(),
                },
                TaskAuthor::User,
            )
            .expect("a cyclic file wedges nothing");
    }

    #[test]
    fn repair_repoints_links_at_a_reminted_id_and_drops_what_it_cannot() {
        let dir = TempDir::new("link-repair");
        dir.plant(
            r#"{"schemaVersion":1,"rev":5,"tasks":[
                {"id":"t 5","title":"typed by hand","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":1,"updatedUnixMs":1},
                {"id":"t-2","title":"linked","body":"","status":"todo","agent":null,
                 "links":[
                    {"link":"related","target":"t 5","atUnixMs":10},
                    {"link":"related","target":"t-2","atUnixMs":11},
                    {"link":"blockedBy","target":"t-9","atUnixMs":20},
                    {"link":"blockedBy","target":"t-9","atUnixMs":30,"deleted":true}],
                 "comments":[],"createdUnixMs":2,"updatedUnixMs":2}]}"#,
        );

        let file = TaskStore::open(dir.root()).snapshot();
        let reminted = &file.tasks[0].id;
        assert!(well_formed_id(reminted));
        let links = &file.tasks[1].links;
        assert!(
            links.iter().any(|l| &l.target == reminted),
            "the link followed the re-mint — a structured reference can be re-pointed where a \
             prose one could only break: {links:?}"
        );
        assert!(
            !links.iter().any(|l| l.target.as_str() == "t-2"),
            "the self-link is gone"
        );
        assert_eq!(
            links
                .iter()
                .filter(|l| l.target.as_str() == "t-9")
                .collect::<Vec<_>>()
                .len(),
            1,
            "duplicate keys collapse"
        );
        assert!(
            links
                .iter()
                .find(|l| l.target.as_str() == "t-9")
                .expect("kept")
                .deleted,
            "…keeping the newest stamp, which is the copy the merge would have chosen"
        );
        assert!(validate(&file).is_ok());
    }

    #[test]
    fn deleting_a_task_leaves_links_to_it_dangling_and_the_file_valid() {
        let dir = TempDir::new("dangling");
        let store = TaskStore::open(&dir.0);
        let blocker = store
            .create(&new_task("the loader"), TaskAuthor::User)
            .expect("create");
        let blocked = store
            .create(&new_task("the panel"), TaskAuthor::User)
            .expect("create");
        store
            .edit(
                &blocked.id,
                TaskEdit::Link {
                    link: LinkType::BlockedBy,
                    target: blocker.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect("linked");

        store.delete(&blocker.id).expect("deleted");
        let survivor = store.get(&blocked.id).expect("still there");
        assert_eq!(
            survivor.links.len(),
            1,
            "delete scrubs nothing — a scrub could not survive the merge, which has no task \
             tombstones and would re-adopt the stale side's edge"
        );
        assert!(validate(&store.snapshot()).is_ok(), "and dangling is legal");
    }

    #[test]
    fn create_lands_compose_time_links_before_the_task_is_visible() {
        let dir = TempDir::new("create-links");
        let store = TaskStore::open(&dir.0);
        let blocker = store
            .create(&new_task("first"), TaskAuthor::User)
            .expect("create");

        let mut req = new_task("second");
        req.links = Some(vec![TaskLinkSpec {
            link: LinkType::BlockedBy,
            target: blocker.id.clone(),
        }]);
        let task = store
            .create(&req, TaskAuthor::User)
            .expect("created linked");
        assert_eq!(
            task.links.len(),
            1,
            "the edge is on the task the create returned"
        );
        assert!(!task.links[0].deleted);

        // Refused whole: a bad spec must not leave a half-created task behind.
        let before = store.list().len();
        let mut bad = new_task("third");
        bad.links = Some(vec![TaskLinkSpec {
            link: LinkType::Related,
            target: TaskId("t-99".into()),
        }]);
        assert!(store.create(&bad, TaskAuthor::User).is_err());
        assert_eq!(store.list().len(), before, "nothing half-created");
    }

    fn new_task(title: &str) -> TaskNew {
        TaskNew {
            project: cide_ipc::ProjectId::new(),
            title: title.to_string(),
            body: None,
            agent: None,
            status: None,
            change: None,
            links: None,
            attachments: None,
        }
    }

    #[test]
    fn create_edit_comment_and_delete_round_trip_through_the_file() {
        let dir = TempDir::new("ops");
        let store = TaskStore::open(dir.root());

        let task = store
            .create(&new_task("Add the retry bar"), TaskAuthor::User)
            .expect("create");
        assert_eq!(task.id.as_str(), "t-1");
        assert_eq!(task.status, TaskStatus::Todo);
        assert!(
            task.comments.is_empty(),
            "a task the user made needs no line saying so"
        );

        store
            .edit(
                &task.id,
                TaskEdit::SetStatus {
                    status: TaskStatus::Doing,
                },
                TaskAuthor::Orchestrator,
            )
            .expect("status");
        let commented = store
            .edit(
                &task.id,
                TaskEdit::Comment {
                    text: "worktree merged clean".into(),
                },
                TaskAuthor::Agent {
                    agent: AgentId("developer".into()),
                    label: "Developer".into(),
                },
            )
            .expect("comment");
        assert_eq!(commented.comments.len(), 1);
        assert!(matches!(
            commented.comments[0].author,
            TaskAuthor::Agent { .. }
        ));

        // Everything is still in memory: the debounce has not elapsed, and the panel must render
        // the task anyway.
        assert!(matches!(store.board(), TaskBoard::Ready { .. }));
        store.write_now();

        // A second store over the same path is the next launch, or the other cide window.
        let reopened = TaskStore::open(dir.root());
        let back = reopened.get(&task.id).expect("still there");
        assert_eq!(back.status, TaskStatus::Doing);
        assert_eq!(back.comments.len(), 1);
        assert_eq!(reopened.list().len(), 1);

        reopened.delete(&task.id).expect("delete");
        assert!(reopened.get(&task.id).is_none());
        assert!(
            reopened.delete(&task.id).is_err(),
            "deleting twice is an error, not a no-op"
        );
    }

    /// `SetStatus` records who moved the task, from where to where, and when — and only on an
    /// actual change. (M27)
    ///
    /// The status log the card renders collapsed is this vector verbatim, so a hop that went
    /// unrecorded is a transition the user can never audit, and a row for a no-op set — which
    /// the panel's segment can send on a repeated click — would be a log entry about nothing.
    #[test]
    fn a_status_change_is_recorded_with_its_author_and_a_no_op_set_is_not() {
        let dir = TempDir::new("history");
        let store = TaskStore::open(dir.root());
        let task = store
            .create(&new_task("history"), TaskAuthor::User)
            .expect("create");
        assert!(task.history.is_empty(), "creation is not a change");

        let moved = store
            .edit(
                &task.id,
                TaskEdit::SetStatus {
                    status: TaskStatus::Doing,
                },
                TaskAuthor::Orchestrator,
            )
            .expect("move");
        assert_eq!(moved.history.len(), 1);
        assert_eq!(moved.history[0].from, TaskStatus::Todo);
        assert_eq!(moved.history[0].to, TaskStatus::Doing);
        assert_eq!(moved.history[0].by, TaskAuthor::Orchestrator);
        assert_eq!(
            moved.history[0].at_unix_ms, moved.updated_unix_ms,
            "stamped where the mutation is applied, from the same clock read"
        );

        let same = store
            .edit(
                &task.id,
                TaskEdit::SetStatus {
                    status: TaskStatus::Doing,
                },
                TaskAuthor::User,
            )
            .expect("no-op");
        assert_eq!(
            same.history.len(),
            1,
            "a set to the status the task already has records nothing"
        );

        // The record survives the file: a second store over the same path is the next launch.
        store.write_now();
        let reopened = TaskStore::open(dir.root());
        assert_eq!(
            reopened.get(&task.id).expect("there").history,
            moved.history
        );
    }

    /// Both sides' transitions survive a merge, ordered by stamp, duplicates collapsed. (M27)
    ///
    /// The scenario is `union_comments`' one status over: the orchestrator moves a task to
    /// `Review` while a stale worktree copy still carries only the `Todo → Doing` hop — and the
    /// merged history must hold both, or the audit log loses whichever side read second.
    #[test]
    fn a_merge_unions_status_history() {
        let hop = |from: TaskStatus, to: TaskStatus, at: u64| TaskStatusChange {
            from,
            to,
            by: TaskAuthor::Orchestrator,
            at_unix_ms: at,
        };
        let mut mine = a_task("t-1", "merge me", 5_000);
        mine.history = vec![
            hop(TaskStatus::Todo, TaskStatus::Doing, 2_000),
            hop(TaskStatus::Doing, TaskStatus::Review, 9_000),
        ];
        let mut theirs = a_task("t-1", "merge me", 4_000);
        theirs.history = vec![hop(TaskStatus::Todo, TaskStatus::Doing, 2_000)];

        let file = |task: &Task| TaskFile {
            schema_version: TaskFile::CURRENT_SCHEMA,
            rev: 1,
            next_id: 2,
            tasks: vec![task.clone()],
        };
        let merged = merge(&file(&mine), &file(&theirs));
        let task = &merged.tasks[0];
        assert_eq!(
            task.history, mine.history,
            "the shared hop collapsed, the newer one adopted, oldest first"
        );
        assert!(
            task.updated_unix_ms >= 9_000,
            "a history row adopted from either side is itself an update, or this merge loses the next one"
        );
    }

    /// The failure [`TaskId`]'s doc is written about: a second `t-2` would silently re-point every
    /// comment, prompt and commit message that ever named the first one.
    #[test]
    fn an_id_is_never_reused_after_a_delete() {
        let dir = TempDir::new("ids");
        let store = TaskStore::open(dir.root());

        let first = store
            .create(&new_task("one"), TaskAuthor::User)
            .expect("one");
        let second = store
            .create(&new_task("two"), TaskAuthor::User)
            .expect("two");
        assert_eq!((first.id.as_str(), second.id.as_str()), ("t-1", "t-2"));

        store.delete(&second.id).expect("delete");
        let third = store
            .create(&new_task("three"), TaskAuthor::User)
            .expect("three");
        assert_eq!(
            third.id.as_str(),
            "t-3",
            "the id of a deleted task came back, or the delete cost a number"
        );

        // And it survives a restart, which is what a counter *in the file* buys: a mark read
        // from the task list alone would drop back to 1 the moment `t-2` was removed.
        store.write_now();
        let reopened = TaskStore::open(dir.root());
        let fourth = reopened
            .create(&new_task("four"), TaskAuthor::User)
            .expect("four");
        assert_eq!(
            fourth.id.as_str(),
            "t-4",
            "the counter did not survive the reopen"
        );
    }

    /// The M61 report: a board with agents on it went `t-772`, `t-774`, `t-776`, because the mark
    /// folded `rev` in and `rev` moves on every comment and status change.
    #[test]
    fn ids_are_contiguous_across_every_other_kind_of_mutation() {
        let dir = TempDir::new("contiguous");
        let store = TaskStore::open(dir.root());
        let first = store
            .create(&new_task("one"), TaskAuthor::User)
            .expect("one");
        store
            .edit(
                &first.id,
                TaskEdit::SetStatus {
                    status: TaskStatus::Doing,
                },
                TaskAuthor::User,
            )
            .expect("status");
        store
            .edit(
                &first.id,
                TaskEdit::Comment {
                    text: "on it".into(),
                },
                TaskAuthor::User,
            )
            .expect("comment");
        store
            .edit(
                &first.id,
                TaskEdit::SetTitle {
                    title: "one, renamed".into(),
                },
                TaskAuthor::User,
            )
            .expect("title");
        let second = store
            .create(&new_task("two"), TaskAuthor::User)
            .expect("two");
        assert_eq!(
            second.id.as_str(),
            "t-2",
            "a mutation between two creates spent a number"
        );
        assert!(
            store.snapshot().rev > 2,
            "the test only tests while rev has run ahead of the ids"
        );

        store.write_now();
        let reopened = TaskStore::open(dir.root());
        let third = reopened
            .create(&new_task("three"), TaskAuthor::User)
            .expect("three");
        assert_eq!(third.id.as_str(), "t-3");
    }

    /// A file written before the counter existed is settled under the rule it was minted under,
    /// once: past `rev`, because the numbers between the largest surviving id and `rev` may be
    /// deleted tasks the file cannot name. Contiguous from there, and written back with the
    /// counter so the next open has nothing to settle.
    #[test]
    fn a_file_without_a_counter_starts_past_its_rev_and_is_contiguous_from_there() {
        let dir = TempDir::new("legacy-counter");
        dir.plant(
            r#"{"schemaVersion":1,"rev":12,"tasks":[
                {"id":"t-3","title":"old","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":1,"updatedUnixMs":1}]}"#,
        );
        let store = TaskStore::open(dir.root());
        assert_eq!(store.snapshot().next_id, 13);
        let a = store.create(&new_task("a"), TaskAuthor::User).expect("a");
        store
            .edit(
                &a.id,
                TaskEdit::Comment {
                    text: "spends nothing".into(),
                },
                TaskAuthor::User,
            )
            .expect("comment");
        let b = store.create(&new_task("b"), TaskAuthor::User).expect("b");
        assert_eq!((a.id.as_str(), b.id.as_str()), ("t-13", "t-14"));

        store.write_now();
        let raw = fs::read_to_string(dir.tasks()).expect("read");
        assert!(raw.contains("\"nextId\": 15"), "{raw}");
    }

    /// The other bad input: a counter behind the largest id — a hand edit, or a file copied from
    /// another board. Raised past it rather than trusted, because a mint from below would fill a
    /// gap that may be a deleted task.
    #[test]
    fn a_counter_behind_the_largest_id_is_raised_past_it() {
        let dir = TempDir::new("low-counter");
        dir.plant(
            r#"{"schemaVersion":1,"rev":2,"nextId":2,"tasks":[
                {"id":"t-9","title":"pasted in","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":1,"updatedUnixMs":1}]}"#,
        );
        let store = TaskStore::open(dir.root());
        let file = store.snapshot();
        assert_eq!(file.next_id, 10);
        assert!(validate(&file).is_ok());
        let next = store
            .create(&new_task("next"), TaskAuthor::User)
            .expect("create");
        assert_eq!(next.id.as_str(), "t-10");
    }

    #[test]
    fn a_counter_behind_a_live_id_is_refused_by_validate() {
        let mut file = a_file(1, vec![a_task("t-4", "four", 1)]);
        assert!(validate(&file).is_ok());
        file.next_id = 4;
        assert!(
            validate(&file).is_err(),
            "a counter that is not past every id will mint one of them again"
        );
    }

    #[test]
    fn the_merge_takes_the_higher_counter_from_either_side() {
        let mut mine = a_file(5, vec![a_task("t-1", "one", 1)]);
        let mut theirs = a_file(5, vec![a_task("t-1", "one", 1)]);
        mine.next_id = 7;
        theirs.next_id = 40;
        assert_eq!(merge(&mine, &theirs).next_id, 40);
        assert_eq!(merge(&theirs, &mine).next_id, 40);
    }

    #[test]
    fn a_mutation_that_breaks_an_invariant_is_rolled_back_whole() {
        let dir = TempDir::new("rollback");
        let store = TaskStore::open(dir.root());
        let task = store
            .create(&new_task("keeps its title"), TaskAuthor::User)
            .expect("create");
        let before = store.snapshot();

        let refused = store.edit(
            &task.id,
            TaskEdit::SetTitle {
                title: "   ".into(),
            },
            TaskAuthor::User,
        );
        assert!(refused.is_err(), "an empty title was accepted");
        // Rolled back *whole*, `rev` included — the same guarantee `WorkspaceState::update` gives,
        // and the reason it snapshots rather than trying to undo the individual field.
        assert_eq!(store.snapshot(), before);
    }

    /// Every task records who asked for it, and nothing is seeded into the log to say so. (M21)
    #[test]
    fn a_task_an_agent_created_says_who_asked_for_it() {
        let dir = TempDir::new("author");
        let store = TaskStore::open(dir.root());
        let theirs = store
            .create(
                &new_task("write the tests"),
                TaskAuthor::Agent {
                    agent: AgentId("qa".into()),
                    label: "QA".into(),
                },
            )
            .expect("create");
        assert_eq!(
            theirs.created_by,
            TaskAuthor::Agent {
                agent: AgentId("qa".into()),
                label: "QA".into(),
            }
        );
        // The log opens **empty**. It used to open with `created this task`, which is now the head
        // of the card instead — two records of one fact is a conversation whose first line repeats
        // what is written above it.
        assert!(
            theirs.comments.is_empty(),
            "the log was seeded: {:?}",
            theirs.comments
        );

        // And the user's own tasks say so rather than saying nothing, which is the half the old
        // arrangement could not express: it inferred "the user" from a *missing* record.
        let ours = store
            .create(&new_task("read the tests"), TaskAuthor::User)
            .expect("create");
        assert_eq!(ours.created_by, TaskAuthor::User);
        assert!(ours.comments.is_empty());
    }

    /// A task written before `Task::created_by` existed keeps its creator, read back out of the
    /// comment the old build seeded. (M21) See `repair`.
    #[test]
    fn the_creator_of_a_task_from_an_older_build_is_recovered_from_its_seeded_comment() {
        let dir = TempDir::new("legacy-creator");
        let planted = r#"{"schemaVersion":1,"rev":9,"tasks":[
                {"id":"t-1","title":"an agent asked for this","body":"","status":"todo",
                 "agent":null,
                 "comments":[{"author":{"kind":"agent","agent":"qa","label":"QA"},
                              "text":"created this task","atUnixMs":1000}],
                 "createdUnixMs":1000,"updatedUnixMs":1000},
                {"id":"t-2","title":"the user asked for this","body":"","status":"todo",
                 "agent":null,"comments":[],"createdUnixMs":2000,"updatedUnixMs":2000},
                {"id":"t-3","title":"an ordinary comment is not a creation record","body":"",
                 "status":"todo","agent":null,
                 "comments":[{"author":{"kind":"orchestrator"},
                              "text":"created this task","atUnixMs":3001}],
                 "createdUnixMs":3000,"updatedUnixMs":3001}]}"#;
        dir.plant(planted);
        let store = TaskStore::open(dir.root());
        let list = store.list();

        assert_eq!(
            list[0].created_by,
            TaskAuthor::Agent {
                agent: AgentId("qa".into()),
                label: "QA".into(),
            },
            "the seeded line is the only record of who asked, and it is still in the file"
        );
        assert_eq!(
            list[1].created_by,
            TaskAuthor::User,
            "no seeded line meant the user, under the rule that build wrote by"
        );
        assert_eq!(
            list[2].created_by,
            TaskAuthor::User,
            "a comment a millisecond after the task is a comment, not the seeding — the timestamp \
             is what stops an ordinary line being adopted as provenance"
        );

        // Reading does not rewrite. Deriving the creator happens in memory, so a teammate who
        // merely opens the project produces no diff in a file their repository tracks; the field
        // reaches disk with the next real mutation, like every other repair this function makes.
        assert_eq!(
            fs::read_to_string(dir.tasks()).expect("read"),
            planted,
            "opening a project rewrote a tracked file"
        );
    }

    // --- layers 3 and 4, through a real disk ----------------------------------------------------

    /// The whole point of layer 3, exercised the way it actually happens: this process writes, a
    /// `git pull` rewrites the file behind its back, and this process writes again.
    #[test]
    fn a_write_after_someone_else_touched_the_file_merges_instead_of_clobbering() {
        let dir = TempDir::new("outofprocess");
        let store = TaskStore::open(dir.root());
        store
            .create(&new_task("ours"), TaskAuthor::User)
            .expect("create");
        store.write_now();

        // A teammate's commit, arriving as a `git pull` would leave it: their task, and a comment
        // on ours that this process has never seen.
        dir.plant(
            r#"{"schemaVersion":1,"rev":40,"tasks":[
                {"id":"t-1","title":"ours","body":"","status":"todo","agent":null,
                 "comments":[{"author":{"kind":"user"},"text":"from the teammate","atUnixMs":77}],
                 "createdUnixMs":1,"updatedUnixMs":77},
                {"id":"t-2","title":"theirs","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":2,"updatedUnixMs":2}]}"#,
        );

        // Now this process makes its own change and flushes.
        store
            .edit(
                &TaskId("t-1".into()),
                TaskEdit::Comment {
                    text: "from this process".into(),
                },
                TaskAuthor::Orchestrator,
            )
            .expect("comment");
        let merged = store.write_now();
        assert!(
            merged.is_some(),
            "the caller is told the board moved under it"
        );

        let on_disk = match read(&dir.tasks()) {
            ReadOutcome::Ready { file, .. } => file,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert_eq!(
            titles(&on_disk),
            ["ours", "theirs"],
            "the pulled task was adopted"
        );
        let texts: Vec<&str> = on_disk.tasks[0]
            .comments
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["from the teammate", "from this process"],
            "neither side's comment was lost"
        );
        assert!(
            on_disk.rev > 40,
            "rev landed past both sides: {}",
            on_disk.rev
        );
    }

    /// The watcher's half of layer 3: `.cide/tasks.json` moved under us with no local write in
    /// flight — a `git pull`, a teammate, an agent in another process — and the fs router asks
    /// this rather than the panel's Retry, because `reload` merges unconditionally and would do
    /// so on every echo of our own flush.
    #[test]
    fn refresh_adopts_an_external_rewrite_and_converges() {
        let dir = TempDir::new("refresh");
        let store = TaskStore::open(dir.root());
        store
            .create(&new_task("ours"), TaskAuthor::User)
            .expect("create");
        store.write_now();

        dir.plant(
            r#"{"schemaVersion":1,"rev":40,"tasks":[
                {"id":"t-1","title":"ours","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":1,"updatedUnixMs":1},
                {"id":"t-2","title":"theirs","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":2,"updatedUnixMs":2}]}"#,
        );

        let merged = store
            .refresh_from_disk()
            .expect("the change is reported so the caller can broadcast it");
        assert_eq!(titles(&merged), ["ours", "theirs"]);
        assert!(
            store.refresh_from_disk().is_none(),
            "asked twice, the second look found something new in an unchanged file"
        );

        // The merge left the in-memory board ahead of the file (`merge` lands `rev` past both
        // sides), and `refresh_from_disk` re-armed the debounce so the ordinary flusher tick —
        // not the watcher thread — writes the convergence.
        std::thread::sleep(persist::SAVE_DEBOUNCE);
        store.flush_if_due();
        let on_disk = match read(&dir.tasks()) {
            ReadOutcome::Ready { file, .. } => file,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert_eq!(
            on_disk,
            store.snapshot(),
            "the flusher converged the file to the merged board"
        );
    }

    /// Our own flush is not an event. The stamp recorded after `write_shared`'s rename is the
    /// echo suppression, and without it every debounced write would come back around through the
    /// filesystem watcher as a merge — this is the difference between `refresh_from_disk` and
    /// `reload`, and the reason the fs router must never call the latter.
    #[test]
    fn refresh_ignores_the_stores_own_write() {
        let dir = TempDir::new("refresh-echo");
        let store = TaskStore::open(dir.root());
        store
            .create(&new_task("ours"), TaskAuthor::User)
            .expect("create");
        store.write_now();
        assert!(
            store.refresh_from_disk().is_none(),
            "the store's own bytes read as an external change"
        );
    }

    /// An external change landing while a local edit is still inside the debounce window merges
    /// rather than replaces — the guarantee `write_now` makes on the flush path, made again on
    /// the watcher path, where the local edit has not reached disk yet and a replace would be
    /// exactly the silent loss the merge exists to prevent.
    #[test]
    fn refresh_merges_with_an_edit_still_in_the_debounce_window() {
        let dir = TempDir::new("refresh-pending");
        let store = TaskStore::open(dir.root());
        store
            .create(&new_task("ours"), TaskAuthor::User)
            .expect("create");
        store.write_now();

        store
            .edit(
                &TaskId("t-1".into()),
                TaskEdit::Comment {
                    text: "pending".into(),
                },
                TaskAuthor::User,
            )
            .expect("comment");
        dir.plant(
            r#"{"schemaVersion":1,"rev":40,"tasks":[
                {"id":"t-1","title":"ours","body":"","status":"todo","agent":null,
                 "comments":[{"author":{"kind":"user"},"text":"from the teammate","atUnixMs":77}],
                 "createdUnixMs":1,"updatedUnixMs":77},
                {"id":"t-2","title":"theirs","body":"","status":"todo","agent":null,
                 "comments":[],"createdUnixMs":2,"updatedUnixMs":2}]}"#,
        );

        let merged = store.refresh_from_disk().expect("merged");
        assert_eq!(titles(&merged), ["ours", "theirs"]);
        let texts: Vec<&str> = merged.tasks[0]
            .comments
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["from the teammate", "pending"],
            "one of the two comments was lost to the refresh"
        );
    }

    /// A store whose file went unreadable heals on its own once the file parses again — no
    /// `reload` needed, because the stamp of a refused file is deliberately `None` and so compares
    /// unequal to everything.
    #[test]
    fn a_resolved_merge_conflict_unblocks_the_next_write() {
        let dir = TempDir::new("heal");
        dir.plant("<<<<<<< HEAD\n{}\n>>>>>>> theirs\n");
        let store = TaskStore::open(dir.root());
        assert!(
            store
                .create(&new_task("blocked"), TaskAuthor::User)
                .is_err()
        );

        // The user resolves it.
        dir.plant(r#"{"schemaVersion":1,"rev":2,"tasks":[]}"#);
        assert!(matches!(store.reload(), TaskBoard::Ready { .. }));
        let task = store
            .create(&new_task("now it works"), TaskAuthor::User)
            .expect("create");
        store.write_now();
        assert!(TaskStore::open(dir.root()).get(&task.id).is_some());
    }

    /// The file is committed and read by a teammate on a shared checkout, so 0600 — which is right
    /// for `workspace.json` and its proxy password — would make the tracker exist and be
    /// unreadable, a failure that looks like a missing feature rather than a permission problem.
    #[test]
    fn the_tracker_is_written_at_a_mode_a_teammate_can_read() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("mode");
        let store = TaskStore::open(dir.root());
        store
            .create(&new_task("committed"), TaskAuthor::User)
            .expect("create");
        store.write_now();

        // The umask is the process's, not this test's to choose, so the expectation is derived
        // from it: `File::create` asks for 0666, so a probe file's mode *is* `0666 & ~umask`, and
        // `0644 & probe` is exactly what a 0644 request must land on. Asserting a bare 0644 would
        // fail on a machine with `umask 077` for a reason that has nothing to do with this code.
        let probe = dir.root().join("probe");
        File::create(&probe).expect("probe");
        let probe_mode = fs::metadata(&probe).expect("stat").permissions().mode() & 0o777;

        let mode = fs::metadata(dir.tasks())
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644 & probe_mode, "probe mode {probe_mode:o}");

        // And the contrast that matters, whenever the umask allows it to be visible at all.
        if probe_mode & 0o044 == 0o044 {
            let private = dir.root().join("private.json");
            persist::write_atomic(&private, b"{}").expect("write_atomic");
            let private_mode = fs::metadata(&private).expect("stat").permissions().mode() & 0o777;
            assert_eq!(
                private_mode & 0o044,
                0,
                "persist's 0600 is untouched by this crate"
            );
            assert_eq!(
                mode & 0o044,
                0o044,
                "the team must be able to read the tracker"
            );
        }
    }

    /// Written with `to_vec_pretty` and a trailing newline, because this file's diffs are read by
    /// people: one line per change rather than one whole-file hunk, and no "\ No newline at end of
    /// file" on every commit that touches the last task.
    #[test]
    fn the_file_is_written_for_a_human_to_read_in_a_diff() {
        let dir = TempDir::new("pretty");
        let store = TaskStore::open(dir.root());
        store
            .create(&new_task("readable"), TaskAuthor::User)
            .expect("create");
        store.write_now();

        let raw = fs::read_to_string(dir.tasks()).expect("read");
        assert!(raw.ends_with("}\n"), "no trailing newline: {raw:?}");
        assert!(
            raw.lines().count() > 5,
            "one-line JSON makes every change a whole-file diff"
        );
        assert!(raw.contains("\"schemaVersion\": 1"));
    }

    /// A starting status the user chose in the compose dialog is honoured; absence is `Todo`.
    /// (M21) See `TaskNew::status` for why this is the user's field and not an agent's.
    #[test]
    fn a_task_may_be_created_into_a_status_other_than_todo() {
        let dir = TempDir::new("start-status");
        let store = TaskStore::open(dir.root());

        let started = store
            .create(
                &TaskNew {
                    status: Some(TaskStatus::Doing),
                    change: None,
                    ..new_task("already under way")
                },
                TaskAuthor::User,
            )
            .expect("create");
        assert_eq!(started.status, TaskStatus::Doing);

        // And the default is unchanged for every caller that names none — which is every caller
        // but the dialog, the MCP path included.
        let plain = store
            .create(&new_task("not started"), TaskAuthor::User)
            .expect("create");
        assert_eq!(plain.status, TaskStatus::Todo);
    }

    /// The debounce is `workspace.json`'s, and a store with nothing owed writes nothing — which is
    /// what keeps a 500 ms tick from rewriting an untouched project's tracker for ever.
    #[test]
    fn nothing_is_written_until_something_changes() {
        let dir = TempDir::new("debounce");
        let store = TaskStore::open(dir.root());
        assert!(store.flush_if_due().is_none());
        assert!(
            !dir.tasks().exists(),
            "an idle store created a file in the user's repository"
        );
    }

    /// The same claim for the **unconditional** write, which is the one that was creating the
    /// file. (M21)
    ///
    /// `flush_if_due` above is gated by the debouncer, so an untouched store never reached the
    /// writer at all; `write_now` deliberately is not gated — project close and quit call it so
    /// the last change is not lost to a debounce that never elapsed — and it wrote an empty
    /// tracker into every project that was merely opened. The reported symptom is a `.cide/`
    /// directory and a tracked `tasks.json` in `git status` for a feature the user never used.
    #[test]
    fn closing_a_project_that_never_had_a_tracker_creates_no_file() {
        let dir = TempDir::new("empty-close");
        let store = TaskStore::open(dir.root());
        assert!(store.write_now().is_none());
        assert!(
            !dir.tasks().exists(),
            "opening and closing a project created a committed file in the user's repository"
        );
        // And the panel still offers the first-run screen rather than a confident empty list.
        assert!(matches!(store.board(), TaskBoard::Absent { .. }));
    }

    /// Created and dropped again inside one debounce: still nothing worth a committed file.
    #[test]
    fn a_task_created_and_deleted_before_the_first_write_leaves_no_file() {
        let dir = TempDir::new("empty-churn");
        let store = TaskStore::open(dir.root());
        let task = store
            .create(&new_task("mistake"), TaskAuthor::User)
            .expect("create");
        store.delete(&task.id).expect("delete");
        store.write_now();
        assert!(!dir.tasks().exists(), "{:?}", dir.entries());
    }

    /// The other half of the guard, and the reason it is not `tasks.is_empty()` alone: once the
    /// file exists, emptying it **is** the change to write. A store that skipped this would leave
    /// the last task in the file for ever — a delete that does not delete.
    #[test]
    fn deleting_the_last_task_of_a_tracker_that_exists_is_still_written() {
        let dir = TempDir::new("empty-again");
        let store = TaskStore::open(dir.root());
        let task = store
            .create(&new_task("real work"), TaskAuthor::User)
            .expect("create");
        store.write_now();
        assert!(dir.tasks().exists(), "the first task creates the file");

        store.delete(&task.id).expect("delete");
        store.write_now();
        let raw = fs::read_to_string(dir.tasks()).expect("read");
        assert!(
            raw.contains("\"tasks\": []"),
            "the deletion of the last task was not persisted: {raw}"
        );
    }

    // --- attachments (M39) --------------------------------------------------------------------

    /// Eight bytes that `image::sniff` calls a PNG, and enough more to be a file.
    fn png_bytes() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0u8; 64]);
        bytes
    }

    fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).expect("source");
        path
    }

    fn a_store_with_one_task(dir: &TempDir) -> (TaskStore, TaskId) {
        let store = TaskStore::open(dir.root());
        let task = store
            .create(&new_task("with files"), TaskAuthor::User)
            .expect("create");
        (store, task.id)
    }

    /// The whole road: a copy lands under the task, its kind is sniffed, a detach tombstones the
    /// record and removes the bytes, and the file round-trips through disk with the tombstone.
    #[test]
    fn an_attachment_is_copied_under_its_task_and_removed_on_detach() {
        let dir = TempDir::new("attach");
        let sources = dir.root().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let png = write_source(&sources, "shot.png", &png_bytes());
        let log = write_source(&sources, "build.log", b"error: nope\n");
        let (store, id) = a_store_with_one_task(&dir);

        let task = store
            .attach(
                &id,
                AttachTarget::Task,
                &[png.clone(), log.clone()],
                TaskAuthor::User,
            )
            .expect("attach to the body");
        assert_eq!(task.attachments.len(), 2);
        let shot = &task.attachments[0];
        assert_eq!(shot.name, "shot.png");
        assert_eq!(shot.kind, cide_ipc::AttachmentKind::Image);
        assert_eq!(shot.bytes, png_bytes().len() as u64);
        assert_eq!(task.attachments[1].kind, cide_ipc::AttachmentKind::File);
        let on_disk = attachments::path_of(dir.root(), &id, shot);
        assert_eq!(fs::read(&on_disk).expect("copied"), png_bytes());
        assert!(
            on_disk.starts_with(dir.root().join(".cide/attachments").join(id.as_str())),
            "{}",
            on_disk.display()
        );
        // The source is a copy's source, not a move's: it is still where it was.
        assert!(png.exists());

        // A comment born with a file, and a file added to a comment that exists.
        let author = TaskAuthor::Agent {
            agent: cide_ipc::AgentId("developer".into()),
            label: "Developer".into(),
        };
        let task = store
            .attach(
                &id,
                AttachTarget::NewComment {
                    text: "here it is".into(),
                },
                std::slice::from_ref(&log),
                author.clone(),
            )
            .expect("attach with a new comment");
        let comment = task.comments.last().expect("the comment landed");
        assert_eq!(comment.text, "here it is");
        assert_eq!(comment.author, author);
        assert_eq!(comment.attachments.len(), 1);
        assert_eq!(comment.attachments[0].added_by, author);
        let task = store
            .attach(
                &id,
                AttachTarget::Comment {
                    id: comment.id.clone(),
                },
                std::slice::from_ref(&png),
                TaskAuthor::User,
            )
            .expect("attach to an existing comment");
        assert_eq!(task.comments.last().unwrap().attachments.len(), 2);
        // Bytes the caller holds land the same way.
        let task = store
            .attach_bytes(
                &id,
                AttachTarget::Task,
                "Pasted image.png",
                &png_bytes(),
                TaskAuthor::User,
            )
            .expect("attach bytes");
        assert_eq!(task.attachments.len(), 3);
        assert_eq!(task.attachments[2].name, "Pasted image.png");

        // Agents may not detach; the user may, and the bytes go with the tombstone.
        let refused = store.edit(
            &id,
            TaskEdit::DetachAttachment {
                attachment: shot.id.clone(),
            },
            author,
        );
        assert!(refused.is_err());
        assert!(
            on_disk.exists(),
            "a refused detach must not have touched the disk"
        );
        let task = store
            .edit(
                &id,
                TaskEdit::DetachAttachment {
                    attachment: shot.id.clone(),
                },
                TaskAuthor::User,
            )
            .expect("the user detaches");
        assert!(task.attachments[0].deleted);
        assert!(!on_disk.exists(), "the bytes must go with the tombstone");
        assert!(
            attachments::dir(dir.root(), &id).exists(),
            "the task's directory stays while it holds other attachments"
        );
        // Twice is a refusal, not a second tombstone.
        assert!(
            store
                .edit(
                    &id,
                    TaskEdit::DetachAttachment {
                        attachment: shot.id.clone(),
                    },
                    TaskAuthor::User,
                )
                .is_err()
        );

        // Through the disk and back: every record, tombstone included.
        store.write_now();
        let reopened = TaskStore::open(dir.root());
        let back = reopened.get(&id).expect("the task is in the file");
        assert_eq!(back.attachments, task.attachments);
        assert_eq!(back.comments, task.comments);
    }

    /// The first refusal wins and nothing has been copied: a comment with three screenshots lands
    /// with three or not at all, and a caller told the second path is wrong finds the tracker as
    /// it was.
    #[test]
    fn a_bad_source_refuses_before_anything_is_copied() {
        let dir = TempDir::new("attach-refuse");
        let sources = dir.root().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let good = write_source(&sources, "ok.txt", b"fine");
        let missing = sources.join("gone.txt");
        let (store, id) = a_store_with_one_task(&dir);

        let error = store
            .attach(
                &id,
                AttachTarget::NewComment {
                    text: "report".into(),
                },
                &[good.clone(), missing.clone()],
                TaskAuthor::User,
            )
            .expect_err("a missing source refuses");
        let text = error.to_string();
        assert!(text.contains("gone.txt"), "{text}");
        assert!(text.contains("no such file"), "{text}");
        assert!(
            !dir.root().join(".cide/attachments").exists(),
            "nothing may have been copied"
        );
        let task = store.get(&id).unwrap();
        assert!(task.comments.is_empty(), "no comment may have been written");

        // Over the cap, without writing 32 MiB: a sparse file reports the length the cap reads.
        let huge = sources.join("huge.bin");
        let file = File::create(&huge).unwrap();
        file.set_len(attachments::MAX_ATTACHMENT_BYTES + 1).unwrap();
        drop(file);
        let text = store
            .attach(&id, AttachTarget::Task, &[huge], TaskAuthor::User)
            .expect_err("over the cap refuses")
            .to_string();
        assert!(text.contains("32 MiB"), "{text}");
        assert!(text.contains("huge.bin"), "{text}");

        // A directory is not a file, and an empty file is not an attachment.
        let text = store
            .attach(
                &id,
                AttachTarget::Task,
                std::slice::from_ref(&sources),
                TaskAuthor::User,
            )
            .expect_err("a directory refuses")
            .to_string();
        assert!(text.contains("not a regular file"), "{text}");
        let empty = write_source(&sources, "empty", b"");
        assert!(
            store
                .attach(&id, AttachTarget::Task, &[empty], TaskAuthor::User)
                .is_err()
        );
        // And a target that is not there refuses before the disk is touched.
        let text = store
            .attach(
                &id,
                AttachTarget::Comment {
                    id: CommentId("nope".into()),
                },
                &[good],
                TaskAuthor::User,
            )
            .expect_err("no such comment")
            .to_string();
        assert!(text.contains("no such comment"), "{text}");
        assert!(!dir.root().join(".cide/attachments").exists());
    }

    /// A source under the staging root is consumed — copied under the task and then gone, with
    /// its own directory — and an ordinary source beside it is left alone.
    #[test]
    fn a_staged_source_is_consumed_and_an_ordinary_one_is_not() {
        let dir = TempDir::new("attach-staged");
        let staging = dir.root().join("staging");
        let slot = staging.join("some-uuid");
        fs::create_dir_all(&slot).unwrap();
        let staged = write_source(&slot, "Pasted image.png", &png_bytes());
        let plain = write_source(dir.root(), "plain.txt", b"kept");
        let (store, id) = a_store_with_one_task(&dir);

        let records = attachments::import_with(
            dir.root(),
            &id,
            &[staged.clone(), plain.clone()],
            &TaskAuthor::User,
            &staging,
        )
        .expect("import");
        assert_eq!(records.len(), 2);
        assert!(!staged.exists(), "the staged file was consumed");
        assert!(!slot.exists(), "and so was its slot directory");
        assert!(staging.exists(), "but never the staging root itself");
        assert!(plain.exists(), "an ordinary source is copied, not moved");
        for record in &records {
            assert!(attachments::path_of(dir.root(), &id, record).exists());
        }
        drop(store);
    }

    /// The id is a path component under the task, so it is unique across the body and every
    /// comment — and `repair` drops rather than re-mints what breaks that, because the bytes are
    /// filed under the old id.
    #[test]
    fn a_broken_attachment_record_is_refused_by_validate_and_dropped_by_repair() {
        let mut task = a_task("t-1", "files", 100);
        task.attachments.push(an_attachment("a-1", "one.txt", 10));
        let mut comment = a_comment("with the same id", 20);
        comment
            .attachments
            .push(an_attachment("a-1", "two.txt", 20));
        task.comments.push(comment);
        let file = a_file(1, vec![task.clone()]);
        let text = validate(&file)
            .expect_err("duplicate across body and comment")
            .to_string();
        assert!(text.contains("two attachments"), "{text}");

        let mut repaired = file.clone();
        repair(&mut repaired);
        assert!(validate(&repaired).is_ok());
        assert_eq!(
            repaired.tasks[0].attachments.len(),
            1,
            "the first copy is kept"
        );
        assert_eq!(
            repaired.tasks[0].attachments[0].id.as_str(),
            "a-1",
            "and keeps its id"
        );
        assert!(repaired.tasks[0].comments[0].attachments.is_empty());

        // A malformed id or name is dropped, and the surviving record keeps the id it had.
        let mut task = a_task("t-2", "files", 100);
        task.attachments.push(an_attachment("", "no-id.txt", 10));
        task.attachments.push(an_attachment("a-2", "../escape", 20));
        task.attachments.push(an_attachment("a-3", "fine.txt", 30));
        let mut file = a_file(1, vec![task]);
        assert!(validate(&file).is_err());
        repair(&mut file);
        assert!(validate(&file).is_ok());
        let kept: Vec<&str> = file.tasks[0]
            .attachments
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(kept, ["a-3"]);
    }

    /// The merge rows for attachments: a record survives a stale side, a tombstone is final, and
    /// a comment's attachment survives a text edit to that comment on the other side.
    #[test]
    fn attachments_merge_table() {
        fn with_body(mut task: Task, attachment: TaskAttachment) -> Task {
            task.attachments.push(attachment);
            task
        }
        fn with_comment(mut task: Task, comment: TaskComment) -> Task {
            task.comments.push(comment);
            task
        }
        let cases = vec![
            Case {
                name: "an attachment on the stale side is adopted",
                why: "a teammate's screenshot pulled in from git must not be discarded because \
                      this process's copy was the newer scalar set",
                mine: a_file(4, vec![a_task("t-1", "mine, newer", 200)]),
                theirs: a_file(
                    4,
                    vec![with_body(
                        a_task("t-1", "theirs, older", 100),
                        an_attachment("a-1", "shot.png", 150),
                    )],
                ),
                check: |out| {
                    assert_eq!(titles(out), ["mine, newer"]);
                    assert_eq!(out.tasks[0].attachments.len(), 1);
                    assert!(
                        out.tasks[0].updated_unix_ms >= 150,
                        "an adopted attachment is an update"
                    );
                },
            },
            Case {
                name: "a tombstone beats a stale live copy",
                why: "a detach must survive a merge with a file written before it, or the bytes \
                      are gone and the record says they are there",
                mine: a_file(
                    4,
                    vec![with_body(a_task("t-1", "t", 100), {
                        let mut a = an_attachment("a-1", "shot.png", 50);
                        a.deleted = true;
                        a
                    })],
                ),
                theirs: a_file(
                    4,
                    vec![with_body(
                        a_task("t-1", "t", 300),
                        an_attachment("a-1", "shot.png", 50),
                    )],
                ),
                check: |out| {
                    assert!(out.tasks[0].attachments[0].deleted, "deleted wins");
                },
            },
            Case {
                name: "a comment's attachment survives a text edit on the other side",
                why: "reconcile_comment picks a whole copy; without the union the winner's list \
                      would silently drop the loser's screenshot",
                mine: a_file(
                    4,
                    vec![with_comment(a_task("t-1", "t", 100), {
                        let mut c = a_comment("report", 50);
                        c.text = "report, corrected".into();
                        c.edited_at_unix_ms = Some(400);
                        c
                    })],
                ),
                theirs: a_file(
                    4,
                    vec![with_comment(a_task("t-1", "t", 100), {
                        let mut c = a_comment("report", 50);
                        c.attachments.push(an_attachment("a-9", "shot.png", 300));
                        c
                    })],
                ),
                check: |out| {
                    let comment = &out.tasks[0].comments[0];
                    assert_eq!(comment.text, "report, corrected", "the edit wins the text");
                    assert_eq!(comment.attachments.len(), 1, "and the attachment survives");
                },
            },
        ];
        for case in cases {
            let out = merge(&case.mine, &case.theirs);
            (case.check)(&out);
            assert!(
                validate(&out).is_ok(),
                "{}: the merge must produce a file this build can write ({})",
                case.name,
                case.why
            );
        }
    }
}
