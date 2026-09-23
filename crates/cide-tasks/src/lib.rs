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

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cide_core::persist::{self, Debouncer};
use cide_core::{CoreError, Result, document};
use cide_ipc::{
    AttachTarget, ChangeName, CommentId, FileStamp, LinkType, Task, TaskAttachment,
    TaskAttachmentId, TaskAuthor, TaskBoard, TaskComment, TaskContent, TaskDetail, TaskEdit,
    TaskFile, TaskId, TaskLink, TaskLinkSpec, TaskNew, TaskRow, TaskStatusChange,
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

/// The directory holding one subdirectory per task: `.cide/tasks`. (M68)
///
/// A sibling of [`TASKS_RELATIVE`] rather than a parent of it, so the index keeps the path every
/// existing project, every doc and `cide-hook`'s own matcher already know. Moving the index into
/// this directory would make an older cide build see **no tracker at all** — and then offer *New
/// task*, and write a fresh schema-1 file beside the new one. Two trackers in one repository is a
/// worse failure than the one the move would tidy, and a v2 file at the old path already produces
/// the right behaviour from an old build: `unreadable`, with nothing that writes.
pub const TASKS_DIR: &str = ".cide/tasks";

/// One task's content, inside its own directory: `task.json`.
pub const CONTENT_FILE: &str = "task.json";

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
    }
    Ok(())
}

/// The invariants one task's [`TaskContent`] must hold for this build to write it. (M68)
///
/// `validate`'s rule 4, moved to the side of the split that can see the records. It runs where the
/// content does: at load, and after any mutation that touches it — never over the index, which no
/// longer carries an attachment anywhere and so cannot be asked.
///
/// The task's id is a parameter purely so the message can name it. Everything asserted here is a
/// fact about one file.
pub fn validate_content(id: &TaskId, content: &TaskContent) -> Result<()> {
    // Attachment ids are unique **within the task, across the body and every comment**: an id is a
    // path component under the task's own directory, so two records sharing one would alias two
    // files onto one path — a stronger claim than a duplicate comment id, which only breaks a merge
    // key. Tombstoned records count, as with links. (M39)
    let mut seen: Vec<&str> = Vec::new();
    for attachment in content
        .attachments
        .iter()
        .chain(content.comments.iter().flat_map(|c| c.attachments.iter()))
    {
        if !well_formed_attachment_id(&attachment.id) {
            return Err(CoreError::Invariant(format!(
                "task {} carries an attachment whose id {:?} is not usable as a path",
                id,
                attachment.id.as_str()
            )));
        }
        if !well_formed_attachment_name(&attachment.name) {
            return Err(CoreError::Invariant(format!(
                "task {} carries an attachment whose name {:?} is not one path component",
                id, attachment.name
            )));
        }
        if seen.contains(&attachment.id.as_str()) {
            return Err(CoreError::Invariant(format!(
                "task {} carries two attachments with the id {}",
                id, attachment.id
            )));
        }
        seen.push(attachment.id.as_str());
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

    let mut kept: Vec<TaskRow> = Vec::with_capacity(file.tasks.len());
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

/// [`repair`]'s content half: the passes that need a task's own file. (M68)
///
/// Called wherever content is loaded, before anything reads it — the same posture `repair` has for
/// the index, and for the same reason: everything downstream of here, the merge above all, is
/// entitled to assume these invariants hold.
///
/// **In memory only.** It does not mark the content dirty and does not cause a write. Opening a
/// project must not rewrite a teammate's tracker for having been read by a newer build, and
/// `the_creator_of_a_task_from_an_older_build_is_recovered_from_its_seeded_comment` asserts exactly
/// that byte-for-byte. The next real mutation persists whatever this derived, along with everything
/// else, and from then on it finds nothing to do.
pub fn repair_content(id: &TaskId, content: &mut TaskContent) {
    /*
     * Every comment leaves this function with an id. (M21)
     *
     * A file written before `TaskComment::id` existed has none, and `#[serde(default)]` gives the
     * empty string. An empty id must never reach `union_comments`, which merges *by* id: every
     * comment in the file would be "the same comment" and the merge would collapse the whole log to
     * one line. Repair is the seam that guarantees it cannot happen, which is the job it already has
     * for task ids on the index side.
     *
     * Derived and not minted. `legacy_id` is a pure function of the old merge key, so two readers of
     * the same file agree without either of them writing — and the file is not rewritten just for
     * having been read by a newer build. A minted uuid here would be a different id per reader,
     * which is a merge that duplicates every comment it touches: exactly the bug ids were added to
     * prevent, introduced by the fix for it.
     */
    for comment in &mut content.comments {
        if comment.id.is_empty() {
            comment.id = TaskComment::legacy_id(&comment.author, comment.at_unix_ms, &comment.text);
        }
    }
    /*
     * An attachment record that cannot stand is **dropped, never re-minted.** (M39)
     *
     * The task and comment passes re-mint or derive an id, because a task id or a comment id is only
     * a key. An attachment id is a *directory name*: the bytes are filed under it. Re-minting the
     * record would leave the file under the old name — an orphan with no symptom but a thumbnail
     * that never loads, since `task_attachment_image` would look under the new one. Dropping the
     * record leaves the bytes where they are for a person to find with `ls`, and says so in the log.
     */
    let mut seen: Vec<String> = Vec::new();
    let mut keep = |list: &mut Vec<TaskAttachment>| {
        list.retain(|attachment| {
            if !well_formed_attachment_id(&attachment.id)
                || !well_formed_attachment_name(&attachment.name)
            {
                tracing::warn!(
                    id = %id,
                    attachment = %attachment.id,
                    name = %attachment.name,
                    "attachment record is not usable as a path; dropped it (its bytes, if any, are left on disk)"
                );
                return false;
            }
            if seen.contains(&attachment.id.0) {
                tracing::warn!(id = %id, attachment = %attachment.id, "duplicate attachment id; keeping the first");
                return false;
            }
            seen.push(attachment.id.0.clone());
            true
        });
    };
    keep(&mut content.attachments);
    for comment in &mut content.comments {
        keep(&mut comment.attachments);
    }
}

/// The creator of a task written before `Task::created_by` existed, read back out of its log. (M21)
///
/// # Why this is its own function now
///
/// It is the one repair that needs **both halves** of a task: the row carries `created_by` and
/// `created_unix_ms`, the content carries the seeded comment. Before M68 both were in one struct and
/// this was six lines inside `repair`; the split makes it a join, and a join with one caller is a
/// function rather than a comment explaining which loop it has to live in.
///
/// That build recorded the answer by *seeding the log*: an agent-made task opened with one comment
/// saying `created this task`, stamped at the same millisecond as the task, and a user-made one
/// opened with nothing. So the fact is still in the file, and this reads it back rather than letting
/// every such task decay to `User` — which is what the serde default gives, and which would credit
/// the user with six subagents' work.
///
/// Three conditions, all three needed. `User` on the row means *either* a file that had no field or
/// a task genuinely created by the user, and the seeding rule says those two are the same set — so a
/// task this build wrote with a real creator is never overwritten. The text and the timestamp
/// together are what stop an ordinary comment being adopted: an agent would have to write those
/// exact three words in the millisecond the task was created, which is the same shape of coincidence
/// [`TaskComment::legacy_id`] already accepts.
///
/// `None` means *nothing to recover*, which is the overwhelmingly common answer. The caller applies
/// it to the row **in memory** and marks nothing dirty; see [`repair_content`].
#[must_use]
pub fn recovered_creator(row: &TaskRow, content: &TaskContent) -> Option<TaskAuthor> {
    if row.created_by != TaskAuthor::User {
        return None;
    }
    let seed = content.comments.first()?;
    if seed.author != TaskAuthor::User
        && seed.text == LEGACY_CREATE_NOTE
        && seed.at_unix_ms == row.created_unix_ms
    {
        return Some(seed.author.clone());
    }
    None
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
pub fn find<'a>(file: &'a TaskFile, id: &TaskId) -> Option<&'a TaskRow> {
    file.tasks.iter().find(|task| &task.id == id)
}

fn find_mut<'a>(file: &'a mut TaskFile, id: &TaskId) -> Option<&'a mut TaskRow> {
    file.tasks.iter_mut().find(|task| &task.id == id)
}

/// The other half of [`TaskRow::of`]: a task's content, projected out of it. (M68)
///
/// The pair is what lets every mutation go on operating on a whole [`Task`] — the shape each arm of
/// `TaskEdit` was written against — while the store keeps the two halves in two files. `TaskRow::of` and
/// this together are a lossless split, and [`task_of`] is the join; the round trip is asserted.
#[must_use]
pub fn content_of(task: &Task) -> TaskContent {
    TaskContent {
        body: task.body.clone(),
        comments: task.comments.clone(),
        history: task.history.clone(),
        attachments: task.attachments.clone(),
    }
}

/// A row and its content, joined back into the [`Task`] every mutation is written against. (M68)
///
/// The counts on the row are **not** consulted: they are a cache of what the content says, and the
/// content is here. Reading them instead would be the one way this join could hand back a `Task`
/// that disagrees with itself — and `TaskRow::of` on the result recomputes them, which is what makes the
/// round trip lossless rather than merely reversible.
#[must_use]
pub fn task_of(row: &TaskRow, content: &TaskContent) -> Task {
    Task {
        id: row.id.clone(),
        title: row.title.clone(),
        status: row.status,
        agent: row.agent.clone(),
        session: row.session,
        change: row.change.clone(),
        links: row.links.clone(),
        created_by: row.created_by.clone(),
        created_unix_ms: row.created_unix_ms,
        updated_unix_ms: row.updated_unix_ms,
        body: content.body.clone(),
        comments: content.comments.clone(),
        attachments: content.attachments.clone(),
        history: content.history.clone(),
    }
}

/// One whole task as `task_get` answers it: its row, joined to everything the row leaves out. (M68)
///
/// The row comes from [`TaskRow::of`] rather than being spelled again here, which is the point of the
/// nesting [`TaskDetail`] documents: the two counts have one producer, and a task's card and a
/// task's row can never disagree about how many comments it has.
///
/// Everything below the row is copied verbatim, tombstones included. Dropping them here was
/// considered and rejected: `adapt.ts` is where a tombstone stops existing, deliberately — it is
/// the one seam that knows the panel has no use for one, and a second filter in Rust would mean two
/// answers to *what is a live comment* with the merge's correctness resting on the other one.
#[must_use]
pub fn detail_of(task: &Task) -> TaskDetail {
    TaskDetail {
        row: TaskRow::of(task),
        body: task.body.clone(),
        comments: task.comments.clone(),
        attachments: task.attachments.clone(),
        history: task.history.clone(),
    }
}

/// Which tasks match a search box's query: the ids, in the file's own order. (M68)
///
/// # Why this is in Rust, when it was a `filter` in the webview
///
/// Because the text it searches stopped being in the webview. The rule is the one
/// `ui/src/sidebar/TasksPanel/model.ts`'s `matchesQuery` documents and it is deliberate on both
/// counts: the **id, the title and the body**, because that is what somebody half-remembering a
/// task recalls — and **not the comments**, because a query would then match a task on a word an
/// agent used in a progress note, which is not what the person asking meant.
///
/// The body is the reason it moved. A board that carried every body was a 2.19 MB payload
/// `eval`ed per window per mutation (see [`TaskRow`]); a board that does not carry them cannot
/// filter on them, and a search box that quietly stopped matching bodies would answer *nothing
/// matched* — a confident false negative, which is precisely the "my tasks are gone" reading the
/// panel's empty screens are written to avoid.
///
/// A free function over a whole [`TaskFile`] so it is testable without a disk, on this section's
/// stated idiom. Case-insensitive, and an empty or whitespace-only query matches **everything**:
/// the caller's "no filter" state must not be spelled as a query that matches nothing.
///
/// # Why the body arrives through a closure
///
/// Because since M68 it is in another file, and this function must not be the thing that decides
/// how many of them to open. `body_of` lets the **store** own that policy — it loads and caches
/// content, and a search is the one operation that genuinely wants all of it — while the rule
/// itself stays a pure function over injected text, which is what keeps the corpus test able to
/// state *id, title, body, never the comments* without a disk anywhere near it.
///
/// `None` from `body_of` means *that body could not be read*, and it is treated as a miss rather
/// than as an error: one unreadable content file must not turn a search into a refusal, because the
/// refusal would be indistinguishable from "nothing matched" at the only place it is seen.
#[must_use]
pub fn search(
    file: &TaskFile,
    body_of: &dyn Fn(&TaskId) -> Option<String>,
    query: &str,
) -> Vec<TaskId> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return file.tasks.iter().map(|task| task.id.clone()).collect();
    }
    file.tasks
        .iter()
        .filter(|task| {
            let hit = |text: &str| text.to_lowercase().contains(&needle);
            hit(task.id.as_str())
                || hit(task.title.as_str())
                // Asked **last**, and only when the row itself missed: the body is in another file,
                // so every call to this is a read. Two thirds of a real query's hits are on the id
                // or the title, and the cheap half of the rule is free.
                || body_of(&task.id).is_some_and(|body| hit(&body))
        })
        .map(|task| task.id.clone())
        .collect()
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
) -> Result<TaskRow> {
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
    let live = |task: &TaskRow, kind: LinkType, to: &TaskId| {
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
) -> Result<TaskRow> {
    if find(file, id).is_none() {
        return Err(no_such_task(id));
    }

    let tombstone = |task: &mut TaskRow, kind: LinkType, to: &TaskId| -> bool {
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
    let mut tasks: Vec<TaskRow> = Vec::with_capacity(mine.tasks.len() + theirs.tasks.len());

    for ours in &mine.tasks {
        match find(theirs, &ours.id) {
            Some(disk) => tasks.push(merge_row(ours, disk)),
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

/// One task's **row**, where the task exists on both sides. (M68)
///
/// The scalar fields come from the newer of the two and the *links* come from both, which is the
/// asymmetry worth stating out loud: a status change and a link are different kinds of edit, and a
/// subagent that links while the orchestrator moves the task to `Review` must not lose either. Ties
/// go to `mine` — an arbitrary choice, but a *stable* one, so the same two files merge to the same
/// result whichever side reads first.
///
/// The comments, the history and the attachment records used to be merged here too. They live in the
/// task's own file now, so [`merge_content`] has them — and the split is what bounds the work:
/// `reconcile` merges the index always and content only for tasks this process actually touched.
fn merge_row(mine: &TaskRow, theirs: &TaskRow) -> TaskRow {
    let mut winner = if theirs.updated_unix_ms > mine.updated_unix_ms {
        theirs.clone()
    } else {
        mine.clone()
    };

    winner.links = union_links(&mine.links, &theirs.links);
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
    // A link edge adopted from the loser is itself an update, and a merged row whose stamp predates
    // its own newest edge would lose the next merge it takes part in — load-bearing twice over,
    // because an adopted edge's stamp is what wins it that next merge.
    if let Some(newest) = winner.links.iter().map(|l| l.at_unix_ms).max() {
        winner.updated_unix_ms = winner.updated_unix_ms.max(newest);
    }
    /*
     * The two counts follow the **winner** and are not recomputed here, which is the one place this
     * function knowingly hands back a row that may disagree with the content beside it.
     *
     * It cannot do better: the content is in another file that this merge has deliberately not
     * opened, so there is nothing to count. Both alternatives are worse. Taking the *larger* of the
     * two would invent a number neither side ever held. Loading the content to count it would make
     * every index merge open every task's file, which is the cost the split removed — and it would
     * do so on the write path, where the merge already holds the store's lock.
     *
     * The disagreement is bounded and self-healing: it can only exist for a task whose content this
     * process has not loaded, and the moment anything loads it — `compose`, a search, a card opening
     * — `TaskRow::of` recomputes both numbers from the file. Invariant 2 in one sentence: the counts are
     * a cache, and the content is the truth.
     */
    winner
}

/// One task's **content**, where both sides have a copy. (M68)
///
/// Every rule here predates the split and none of them changed: the four unions are per-vector, and
/// what they defend against — a tombstone that must not be resurrected, a comment edited on one side
/// while a file was attached on the other — is a property of the vectors rather than of the file they
/// happened to be stored in. That is the reason this is a new *caller* and not new logic.
///
/// The row's `updated_unix_ms` is **not** set here, because a row is not in scope. The caller raises
/// it through [`newest_content_stamp`]; skipping that is invariant 4, and breaking it makes a merged
/// task quietly lose the next merge it takes part in and quietly stop sorting to the top of its group.
#[must_use]
pub fn merge_content(mine: &TaskContent, theirs: &TaskContent) -> TaskContent {
    // The body follows neither side by stamp, because content carries no stamp of its own: the row
    // does. `mine` wins, which is this module's standing convention for a tie — and the case where
    // the two bodies genuinely differ is a concurrent edit of the same prose, which no rule can
    // resolve well and which `TaskEdit::SetBody` makes a deliberate, visible gesture on both sides.
    TaskContent {
        body: mine.body.clone(),
        comments: union_comments(&mine.comments, &theirs.comments),
        history: union_history(&mine.history, &theirs.history),
        attachments: union_attachments(&mine.attachments, &theirs.attachments),
    }
}

/// The newest moment anything in a task's content happened, for invariant 4. (M68)
///
/// A comment, a history row or an attachment adopted from the losing side of a merge is *an update*,
/// and a row whose `updated_unix_ms` predates its own newest comment loses the next merge it takes
/// part in — and sorts below tasks nothing has happened to, since that field is also the panel's
/// in-group order. Before the split `merge_task` raised the stamp itself; now the row and the content
/// are merged in different places, so the fact has to travel.
///
/// `None` for content with nothing in it at all, which is an ordinary task nobody has commented on.
#[must_use]
pub fn newest_content_stamp(content: &TaskContent) -> Option<u64> {
    content
        .comments
        .iter()
        .map(|c| c.at_unix_ms)
        .chain(content.history.iter().map(|h| h.at_unix_ms))
        .chain(
            content
                .attachments
                .iter()
                .chain(content.comments.iter().flat_map(|c| c.attachments.iter()))
                .map(|a| a.added_unix_ms),
        )
        .max()
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
        /// The content a schema-1 → 2 migration lifted out of the index, or `None` for a file that
        /// was already current. (M68)
        ///
        /// `Some` means **this content exists in memory and nowhere on disk**, so the store's `open`
        /// has to flush it rather than wait for a mutation. It travels here rather than as a field of
        /// [`TaskFile`] because a field would be a second, permanent place a task's content could
        /// live, and the first hand-edited file that used it would have two answers for one task.
        migrated: Option<HashMap<TaskId, TaskContent>>,
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
    // A **newer** schema refuses, and does not rename. It is a teammate on a newer build whose file
    // this one would silently downgrade on the next flush, dropping every field it does not know —
    // the failure is total and it lands in their repository.
    if version > current {
        return ReadOutcome::Refused {
            error: format!("task file schema {version} is newer than this build's {current}"),
        };
    }
    if version < u64::from(TaskFile::OLDEST_READABLE_SCHEMA) {
        return ReadOutcome::Refused {
            error: format!(
                "task file schema {version} is older than this build can read ({})",
                TaskFile::OLDEST_READABLE_SCHEMA
            ),
        };
    }

    // An **older** one is migrated. (M68) One arm per step, each rewriting the document one version
    // forward — `persist::migrate`'s ladder, and the reason it works on a `Value`: the document no
    // longer has the shape the current `TaskFile` describes, and the version has to be readable
    // before anything commits to a shape.
    let (value, migrated) = match migrate(value, version) {
        Ok(answer) => answer,
        Err(error) => {
            return ReadOutcome::Refused {
                error: format!("task file schema {version} could not be migrated: {error}"),
            };
        }
    };

    // From the migrated `Value` when there was a migration, from the raw text when there was not.
    // The text path is not an optimisation — it is what keeps an unmigrated read byte-exact about
    // field order and numeric spelling, which `serde_json::Value` does not promise.
    let parsed = if migrated.is_some() {
        serde_json::from_value::<TaskFile>(value).map_err(|e| e.to_string())
    } else {
        serde_json::from_str::<TaskFile>(&raw).map_err(|e| e.to_string())
    };
    match parsed {
        Ok(mut file) => {
            // In memory a tracker is always the current schema; the number on disk is decided at
            // write time by `TaskFile::schema_on_disk`. (M83)
            file.schema_version = TaskFile::CURRENT_SCHEMA;
            repair(&mut file);
            ReadOutcome::Ready {
                file,
                stamp,
                // A migrated file's content is in memory and **nowhere on disk yet**, so the store
                // must flush it rather than wait for a mutation. This flag is how `open` knows;
                // without it a converted tracker would sit with an index in memory and no content
                // files until somebody happened to edit something.
                migrated,
            }
        }
        Err(error) => ReadOutcome::Unparseable { error, conflicted },
    }
}

/// Bring a raw tracker document up to [`TaskFile::CURRENT_SCHEMA`]. (M68)
///
/// A ladder: one arm per supported older schema, each rewriting the document one step forward and
/// re-entering. Adding version 3 is an arm, not a restructuring — `cide_core::persist::migrate` is
/// the model and its doc carries the argument for taking JSON rather than a typed value.
///
/// Answers `(document, migrated)`, where `migrated` is false only for a document that was already
/// current. The caller needs to know, because a migrated tracker's content exists **in memory only**
/// and has to be flushed before anything else reads it.
type Migrated = Option<HashMap<TaskId, TaskContent>>;

fn migrate(value: Value, version: u64) -> std::result::Result<(Value, Migrated), String> {
    // 2 and 3 are one shape: 3 only adds a status value (M83), so a 2 needs no conversion and
    // — importantly — no flush. Counting it as migrated would rewrite every schema 2 tracker on
    // open, which is exactly what `TaskFile::schema_on_disk` exists to avoid.
    if version >= u64::from(TaskFile::SCHEMA_WITHOUT_INBOX) {
        return Ok((value, None));
    }
    let mut value = value;
    let mut version = version;
    let mut content: HashMap<TaskId, TaskContent> = HashMap::new();
    // The ladder ends at 2, not at `CURRENT_SCHEMA`: 2 → 3 is not a conversion (see above).
    while version < u64::from(TaskFile::SCHEMA_WITHOUT_INBOX) {
        let (next, lifted) = match version {
            1 => v1_to_v2(value)?,
            other => return Err(format!("no migration from schema {other}")),
        };
        value = next;
        // Later steps win, because a step that touched a task's content produced the newer answer for
        // it. There are no later steps yet; the `extend` is what makes adding one an arm rather than
        // a rethink.
        content.extend(lifted);
        version += 1;
    }
    Ok((value, Some(content)))
}

/// Schema 1 → 2: lift each task's body, log, history and files out of the index. (M68)
///
/// Schema 1 held every task whole inside `.cide/tasks.json`. Schema 2 keeps a [`TaskRow`] there and
/// puts the rest in `.cide/tasks/<id>/task.json`, so this rewrites each element of `tasks` into a row
/// — and hands the content back **inside the same document**, under a key the current `TaskFile` does
/// not have, so that `read` can hand it to the store in one piece.
///
/// # Why the content rides in the document rather than being written here
///
/// Because `read` is a *read*. It is called by `reload`, by `refresh_from_disk`, by
/// `cide-headless tasks` and by tests, and a function that wrote 135 files as a side effect of being
/// asked what a file says would be a surprise in every one of those places. The store's `open` is the
/// one caller that owns the conversion, and it is the one that flushes.
///
/// The two counts are computed here through [`TaskRow::of`] — the single producer, as everywhere else — by
/// parsing each element as a whole `Task` first. That is also the migration's validation: an element
/// that will not parse as a `Task` is a tracker this build cannot convert, and it says so rather than
/// silently dropping the task.
fn v1_to_v2(value: Value) -> std::result::Result<(Value, HashMap<TaskId, TaskContent>), String> {
    let mut root = match value {
        Value::Object(map) => map,
        _ => return Err("a task file must be a JSON object".to_string()),
    };
    let tasks = root
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut rows = Vec::with_capacity(tasks.len());
    let mut content = HashMap::with_capacity(tasks.len());
    for element in tasks {
        let mut task: Task = serde_json::from_value(element)
            .map_err(|error| format!("a schema 1 task will not parse: {error}"))?;
        /*
         * The creator recovery happens **here**, and this is the right moment for it. (M21, M68)
         *
         * A task written before `Task::created_by` existed records its creator by having a seeded
         * first comment, and `recovered_creator` reads it back. Before M68 that ran on every read
         * and was deliberately never persisted, because reading a teammate's tracker must not
         * rewrite it.
         *
         * A migration is the one read that *is* a rewrite, and it is the only read that has both
         * halves of a legacy task in hand at once — the row's `created_by` and the log that answers
         * it. So the answer is written down once, here, instead of being re-derived by every
         * content load for the rest of the file's life. After this, `t-1` simply says who asked.
         */
        if let Some(author) = recovered_creator(&TaskRow::of(&task), &content_of(&task)) {
            task.created_by = author;
        }
        let row = serde_json::to_value(TaskRow::of(&task))
            .map_err(|error| format!("could not encode a migrated row: {error}"))?;
        content.insert(task.id.clone(), content_of(&task));
        rows.push(row);
    }

    root.insert("schemaVersion".to_string(), Value::from(2u32));
    root.insert("tasks".to_string(), Value::Array(rows));
    Ok((Value::Object(root), content))
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
fn load(path: &Path) -> (TaskFile, DiskState, Option<FileStamp>, Migrated) {
    match read(path) {
        ReadOutcome::Absent => (TaskFile::default(), DiskState::Absent, None, None),
        ReadOutcome::Ready {
            file,
            stamp,
            migrated,
        } => (file, DiskState::Ready, stamp, migrated),
        ReadOutcome::Refused { error } => {
            tracing::warn!(path = %path.display(), %error, "task file left untouched");
            (
                TaskFile::default(),
                DiskState::Unreadable { error },
                // Deliberately no stamp. A `None` here means the next write preflight sees a
                // mismatch against anything and re-reads, which is what should happen: the file is
                // one this build refused, and the moment it changes is the moment to look again.
                None,
                // Nothing was migrated: the file was not read.
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
            (
                TaskFile::default(),
                DiskState::Quarantined { error },
                None,
                None,
            )
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
/// Write one task's content, creating its directory. Answers with the stamp of what landed. (M68)
///
/// `write_shared`'s mode, for its reason: the file is committed, a teammate reads it, and 0600 would
/// make a tracker one developer could read and another could not. The **directory** gets the same
/// treatment by being created with the process umask rather than a tightened mode — a 0700 directory
/// holding 0644 files is a file nobody but the owner can reach, which is the mode question asked one
/// level up and got wrong the first time somebody writes `create_dir` with explicit permissions.
fn write_content(root: &Path, id: &TaskId, content: &TaskContent) -> Result<Option<FileStamp>> {
    let dir = content_dir(root, id);
    fs::create_dir_all(&dir)
        .map_err(|error| CoreError::Io(format!("could not create {}: {error}", dir.display())))?;
    let path = content_path(root, id);
    let mut bytes = serde_json::to_vec_pretty(content)
        .map_err(|error| CoreError::Serde(format!("could not encode task content: {error}")))?;
    // `to_vec_pretty` stops at the closing brace. A committed text file without a trailing newline
    // makes `git diff` print "\ No newline at end of file" on every hunk that reaches the end, for
    // ever — and these files' diffs are read by people exactly as the index's are.
    bytes.push(b'\n');
    write_shared(&path, &bytes)?;
    Ok(document::stamp_at(&path))
}

/// Remove one task's whole directory: its content **and** its attachments. (M68)
///
/// Best effort and never an error, on `attachments::remove`'s terms and for its reason: this runs
/// after the tombstone has landed, so a failure here leaves bytes on disk that nothing points at —
/// untidy, recoverable with `ls`, and not worth failing a delete the user has already seen succeed.
///
/// `remove_dir_all`, which takes `attachments/` with it. That is the whole argument for putting a
/// task's files under the task rather than beside it: deleting a task used to leave
/// `.cide/attachments/<id>/` behind, because the two lived in different trees and only one of them
/// was being cleaned up.
///
/// The parent `.cide/tasks` is then pruned **non-recursively**, so it disappears only when it is
/// empty — `attachments::remove`'s own last line, for the same reason: a tracker whose last task was
/// deleted should not leave an empty directory in somebody's repository.
fn remove_content(root: &Path, id: &TaskId) {
    let dir = content_dir(root, id);
    if let Err(error) = fs::remove_dir_all(&dir)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(
            path = %dir.display(),
            %error,
            "could not remove a deleted task's directory; its files are left on disk"
        );
    }
    let _ = fs::remove_dir(root.join(TASKS_DIR));
}

pub fn write_shared(path: &Path, json: &[u8]) -> Result<()> {
    persist::write_atomic_with_mode(path, json, persist::SHARED_MODE)
}

/// Where one task's content file lives, relative to the project root: `.cide/tasks/<id>`. (M68)
///
/// The task's own directory, holding `task.json` **and** `attachments/`. One directory per task is
/// what makes `git rm -r .cide/tasks/t-17` take a task's description, its whole conversation and
/// every file anybody attached to it, in one gesture — which is the reason
/// [`cide_ipc::TaskAttachment::relative_path`] already gave for putting the task id in the
/// attachment path, now applied to the whole task.
///
/// `well_formed_id` is what makes this safe: ASCII alphanumerics plus `-` and `_`, so an id can be
/// neither a separator nor `..`. That function's doc records this as its second reason.
#[must_use]
pub fn content_dir(project_root: &Path, id: &TaskId) -> PathBuf {
    project_root.join(TASKS_DIR).join(id.as_str())
}

/// `<root>/.cide/tasks/<id>/task.json`.
#[must_use]
pub fn content_path(project_root: &Path, id: &TaskId) -> PathBuf {
    content_dir(project_root, id).join(CONTENT_FILE)
}

/// One task's content as this process holds it, and whether it owes the disk a write.
#[derive(Clone)]
struct Held {
    content: TaskContent,
    /// The stamp of the bytes this store last wrote or read **for this task**.
    ///
    /// Per task rather than one for the tracker, because that is the unit that is now written: a
    /// comment on `t-14` must not make the store think `t-15`'s file moved under it.
    stamp: Option<FileStamp>,
}

/// The content this process has loaded, and the subset of it that is dirty. (M68)
///
/// # The invariant that makes lazy loading safe
///
/// **A content file this process has not loaded is never written.** Nothing else in this module has
/// to reason about a partially-known board, because an absent entry here is not "empty" — it is "not
/// ours to touch". A `git pull` that brings new comments for a task nobody opened therefore needs no
/// merge at all: theirs is simply still on disk, and the next reader loads it.
///
/// It is also what bounds the merge. Before M68 every write re-read and merged the whole tracker;
/// now `reconcile` walks the index plus exactly the tasks in `dirty`, which on an ordinary afternoon
/// is one.
#[derive(Default)]
struct ContentCache {
    loaded: HashMap<TaskId, Held>,
    /// Ids whose content differs from what is on disk. A `BTreeSet` so a flush writes in a stable
    /// order — not for correctness, but so two runs of the same mutations touch files in the same
    /// sequence and a `strace` or a filesystem watcher reads the same either time.
    dirty: BTreeSet<TaskId>,
    /// Ids whose directory the next flush should remove: tasks deleted since the last one.
    ///
    /// Recorded rather than removed on the spot because [`TaskStore::update`]'s closure does no I/O —
    /// the rule `TaskEdit::DetachAttachment` already follows, and what keeps a rollback from having
    /// to put a directory back.
    removed: BTreeSet<TaskId>,
    /// The undo log for the mutation in flight, or `None` outside one.
    ///
    /// # Why an undo log and not a clone
    ///
    /// `TaskStore::update` restores the index by cloning it, which costs ~50 KiB and is the right
    /// trade for a structure that small. The same move here would clone every body and every comment
    /// this process has loaded — on a board whose content is warm, the entire tracker, per mutation.
    /// That is precisely the cost the split was done to remove, and it would have arrived back
    /// through the rollback path where nothing would have measured it.
    ///
    /// So only what a mutation actually touches is remembered. One entry per `put`, holding the
    /// previous [`Held`] (or `None` for a task whose content this process had not loaded) and whether
    /// it was already dirty — because a rollback must not leave a task dirty that only this failed
    /// mutation made so, and must not clear a dirt an *earlier* accepted mutation created.
    journal: Option<Vec<Undo>>,
}

/// One `put`, remembered so it can be taken back.
struct Undo {
    id: TaskId,
    /// What was held before, or `None` if nothing was.
    was: Option<Held>,
    /// Whether the id was already in `dirty`.
    was_dirty: bool,
    /// Whether the id was already marked for removal.
    ///
    /// Without this a `delete` whose `validate` then failed would leave its id in `removed`, and the
    /// next flush would delete the directory of a task that is still in the index — the content of a
    /// live task, gone, with the row still drawing it. The narrowest possible window and the widest
    /// possible consequence, which is exactly the shape of thing this journal exists for.
    was_removed: bool,
}

impl ContentCache {
    /// Start remembering. Called by [`TaskStore::update`] under the lock.
    fn begin(&mut self) {
        self.journal = Some(Vec::new());
    }

    /// Accept the mutation: forget the undo log.
    fn commit(&mut self) {
        self.journal = None;
    }

    /// Put every touched task back exactly as it was, newest first.
    fn rollback(&mut self) {
        let Some(journal) = self.journal.take() else {
            return;
        };
        for undo in journal.into_iter().rev() {
            match undo.was {
                Some(held) => {
                    self.loaded.insert(undo.id.clone(), held);
                }
                None => {
                    self.loaded.remove(&undo.id);
                }
            }
            if undo.was_dirty {
                self.dirty.insert(undo.id.clone());
            } else {
                self.dirty.remove(&undo.id);
            }
            // A `delete` that was rolled back must not leave its directory marked for removal: the
            // task is still in the index, and the next flush would take its content away underneath
            // it. `delete` journals through `note` for exactly this line.
            if undo.was_removed {
                self.removed.insert(undo.id);
            } else {
                self.removed.remove(&undo.id);
            }
        }
    }

    /// Record the state of one task before it is overwritten.
    fn note(&mut self, id: &TaskId) {
        if self.journal.is_some() {
            let was = self.loaded.get(id).cloned();
            let was_dirty = self.dirty.contains(id);
            let was_removed = self.removed.contains(id);
            if let Some(journal) = self.journal.as_mut() {
                journal.push(Undo {
                    id: id.clone(),
                    was,
                    was_dirty,
                    was_removed,
                });
            }
        }
    }
}

impl ContentCache {
    /// Read one task's content off disk, repairing it, and remember it.
    ///
    /// Total: a file that is absent, unreadable or unparseable yields [`TaskContent::default`] and a
    /// log line, never an error. That is `load`'s discipline for the index applied one level down,
    /// and here it matters more rather than less — a card that will not open because one comment
    /// file has a stray brace is a task the user cannot reach, and the honest failure is an empty
    /// log they can see is wrong.
    ///
    /// **A file that failed to read gets no stamp** (`None`), which compares unequal to everything,
    /// so the next write preflight re-reads rather than assuming its own empty copy is current. The
    /// alternative — stamping the failure — would let an empty in-memory content overwrite a file
    /// that was merely locked or mid-write when we looked.
    fn read(root: &Path, id: &TaskId) -> Held {
        let path = content_path(root, id);
        // Stamped **before** the bytes are read, on `read`'s own argument for the index: a stamp
        // taken afterwards can be newer than the bytes in hand, and the preflight would then see
        // agreement and quietly overwrite somebody's write. Stamping first costs at worst one
        // redundant merge.
        let stamp = document::stamp_at(&path);
        let mut content = match fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<TaskContent>(&raw) {
                Ok(content) => content,
                Err(error) => {
                    tracing::error!(
                        path = %path.display(),
                        %error,
                        "task content will not parse; treating it as empty (the file is left alone)"
                    );
                    return Held {
                        content: TaskContent::default(),
                        stamp: None,
                    };
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // The ordinary answer for a task whose row exists and whose content does not: a
                // task created and never given a body, or a row a merge adopted from a side whose
                // content file has not arrived. Not a warning — it is a legal state of the tracker.
                TaskContent::default()
            }
            Err(error) => {
                tracing::error!(
                    path = %path.display(),
                    %error,
                    "could not read task content; treating it as empty (the file is left alone)"
                );
                return Held {
                    content: TaskContent::default(),
                    stamp: None,
                };
            }
        };
        repair_content(id, &mut content);
        // The same posture `open` takes for the index: `repair_content` is total so this cannot
        // fire, and the line exists because if it ever does, every mutation of this task would roll
        // back and the card would refuse every edit with no other symptom.
        if let Err(error) = validate_content(id, &content) {
            tracing::error!(
                path = %path.display(),
                %error,
                "repaired task content still fails validation — this is a bug in repair_content"
            );
        }
        Held { content, stamp }
    }
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
/// Four since M68, and the order is always `inner` → `content` → `state` → `stamp`. Nothing here
/// calls back into the store, and [`Self::update`]'s closure must not either — it runs under
/// `inner`, so anything that re-enters deadlocks, which is the same contract
/// `WorkspaceState::with` states.
///
/// `state` is the field the four-field sketch of this struct did not have, and it is here because
/// [`TaskBoard`] has three variants. `Absent` and `Ready` can be derived from a `stat` and the
/// in-memory file, but `Unreadable` carries the parser's own message and there is nowhere else to
/// keep it — and a `TaskBoard` variant that nothing can ever produce is the dead-control failure
/// this repository has already deleted one instance of (`SplitIntent::Diff`).
pub struct TaskStore {
    path: PathBuf,
    inner: Mutex<TaskFile>,
    /// The content this process has loaded, and what of it is dirty. (M68)
    ///
    /// Fourth in the lock order, always taken after `inner` — see the struct's doc. A mutation holds
    /// both, because a content change has to move its row's `updated_unix_ms` in the same breath or
    /// the board silently stops reordering.
    content: Mutex<ContentCache>,
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
        let (file, state, stamp, migrated) = load(&path);

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

        let mut cache = ContentCache::default();
        let converting = migrated.is_some();
        if let Some(content) = migrated {
            /*
             * A schema 1 tracker, converted. (M68)
             *
             * The content is in memory and nowhere on disk, so every task is seeded **and marked
             * dirty**: the flush below is what writes `.cide/tasks/<id>/task.json` for each of them.
             * The stamp is `None` because there is no file behind any of it yet, which is also what
             * stops the preflight in `write_now` from deciding these are somebody else's writes.
             */
            for (id, body) in content {
                cache.dirty.insert(id.clone());
                cache.loaded.insert(
                    id,
                    Held {
                        content: body,
                        stamp: None,
                    },
                );
            }
        }

        let store = Self {
            path,
            inner: Mutex::new(file),
            content: Mutex::new(cache),
            debounce: Debouncer::new(persist::SAVE_DEBOUNCE),
            stamp: Mutex::new(stamp),
            state: Mutex::new(state),
            root: project_root.to_path_buf(),
        };

        if converting {
            /*
             * **Converted on start, outright, before anything else touches the tracker.**
             *
             * The alternative was to let the conversion ride the first mutation, which is where a
             * lazy design naturally puts it and which preserved cide's *opening a project rewrites
             * nothing* promise. It was rejected: it makes the moment of conversion unpredictable —
             * some later edit, possibly while several agents are writing — and leaves `git status`
             * clean until it suddenly is not. Here it is one observable event at a known time, and
             * `git checkout .cide` undoes all of it.
             *
             * What that costs, knowingly: merely *opening* a schema 1 project rewrites its committed
             * files, which is the surprise commit `TaskBoard::Absent`'s own hint text argues against.
             * For a project of the user's own that is what they want; for a repository they opened to
             * look at it is a diff they did not ask for. The alternatives were worse — asking first
             * leaves the tracker read-only, which refuses a dispatched agent's `cide_task_update`
             * until somebody notices a banner, and converting only for "projects of mine" invents a
             * distinction nothing else in cide draws.
             *
             * The attachment relocation rides along, and is the one part that touches bytes rather
             * than JSON.
             */
            let moved = attachments::relocate(project_root, &store.snapshot());
            let written = store.write_now();
            tracing::info!(
                path = %store.path.display(),
                tasks = store.snapshot().tasks.len(),
                attachments_moved = moved,
                converged = written.is_some(),
                "converted this project's task tracker from schema 1 to schema 2"
            );
        }

        store
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
        let guard = self.inner.lock();
        let mut cache = self.content.lock();
        self.compose(&guard, &mut cache, id).ok()
    }

    /// Every task's **row**, in file order — the order the panel renders and an agent reads. (M68)
    ///
    /// Rows and not whole tasks, which is what keeps this callable on a hot path. Its three callers
    /// want exactly what a row carries: `TaskSink::list` renders `cide_task_list`'s summaries,
    /// `task_triggers` reads blockers' statuses once per burst, and `cmd::agents` checks the
    /// dispatch gate. None of them reads a body or a log — `cide_task_get` is the road for that, and
    /// `render_summary`'s doc says why the split is what makes the list affordable.
    ///
    /// A version of this that composed every task would open one file per task on every trigger
    /// burst, which is the cost the whole split exists to avoid.
    pub fn list(&self) -> Vec<TaskRow> {
        self.inner.lock().tasks.clone()
    }

    /// One task's content, loaded if need be — for a caller that has the row already. (M68)
    ///
    /// The seam `search` reaches through, and the reason it takes a closure: this is where the policy
    /// about *how many* content files to open lives, and a search is the one operation that wants all
    /// of them.
    fn body_of(&self, cache: &mut ContentCache, id: &TaskId) -> String {
        cache
            .loaded
            .entry(id.clone())
            .or_insert_with(|| ContentCache::read(&self.root, id))
            .content
            .body
            .clone()
    }

    /// Which tasks match a query: `cide_tasks::search`, over this store's content. (M68)
    ///
    /// Loads and caches whatever bodies it has not got, which is the documented cost of searching:
    /// the first query in a session pays for the board, and every later one is served from memory.
    /// The rule itself is the free function's — this only supplies the text.
    pub fn search(&self, query: &str) -> Vec<TaskId> {
        let guard = self.inner.lock();
        let mut cache = self.content.lock();
        // Collected first, because `search`'s closure cannot borrow the cache mutably while the
        // index is being walked — and because a body read is a file read, which should not happen
        // inside a matcher.
        let ids: Vec<TaskId> = guard.tasks.iter().map(|task| task.id.clone()).collect();
        let bodies: HashMap<TaskId, String> = ids
            .iter()
            .map(|id| (id.clone(), self.body_of(&mut cache, id)))
            .collect();
        search(&guard, &|id| bodies.get(id).cloned(), query)
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
            // Rows, not whole tasks (M68) — `TaskRow::of`'s doc and [`TaskRow`]'s carry the reason. This
            // is a `map` and not a `clone` on purpose: the board is broadcast to every window on
            // every mutation, so what it does *not* carry is the whole point of it.
            // The index *is* the board since M68: `TaskFile::tasks` is already `Vec<TaskRow>`, so
            // this is one clone of ~50 KiB rather than the 2.25 MB it used to be. [`TaskRow`]'s doc
            // has the measurement and `emit::tasks_changed`'s has what it was costing.
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
            let mut cache = self.content.lock();
            let mut state = self.state.lock();
            let (disk, disk_state, stamp, migrated) = load(&self.path);
            /*
             * A *Retry* that finds a schema 1 file converts it, exactly as `open` does. (M68)
             *
             * The path is reachable: a `git checkout` of a branch that predates the conversion puts a
             * schema 1 tracker back under a running cide, the panel says the board changed, and Retry
             * is what the user presses. Seeding without marking dirty would leave the content in
             * memory and never on disk — and the index, which *is* written, would then name content
             * files that do not exist.
             */
            if let Some(content) = migrated {
                for (id, body) in content {
                    cache.dirty.insert(id.clone());
                    cache.loaded.insert(
                        id,
                        Held {
                            content: body,
                            stamp: None,
                        },
                    );
                }
                let moved = attachments::relocate(&self.root, &disk);
                tracing::info!(
                    path = %self.path.display(),
                    attachments_moved = moved,
                    "a reload found a schema 1 tracker; converted it"
                );
                self.debounce.note_change();
            }
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
    /// One task, composed out of its row and its content — loading the content if this process
    /// has not seen it. (M68)
    ///
    /// # Why every mutation still works on a whole `Task`
    ///
    /// Each arm of [`TaskEdit`] was written against a `&mut Task`, and every one of them carries a
    /// comment explaining a decision that was paid for. Rewriting twelve arms to reach into two
    /// structures would have moved all of that prose and risked all of those decisions, to save what
    /// turns out to be one small file read on the arms that do not touch content.
    ///
    /// So the split lives here and in [`Self::put`], and nothing above them changed. The cost is
    /// paid honestly: a `SetTitle` loads the task's content, `put` finds it unchanged, and only the
    /// index is written. The *benefit* invariant still holds either way — content this process has
    /// loaded is content it is allowed to write, and content it has not loaded it never touches.
    ///
    /// Must be called with `inner` and `content` already held, which is why it takes them rather
    /// than locking: re-entering the store under its own lock is the deadlock [`Self::update`]'s
    /// contract forbids.
    fn compose(&self, index: &TaskFile, cache: &mut ContentCache, id: &TaskId) -> Result<Task> {
        let row = find(index, id).ok_or_else(|| no_such_task(id))?.clone();
        let held = cache
            .loaded
            .entry(id.clone())
            .or_insert_with(|| ContentCache::read(&self.root, id));
        /*
         * The creator recovery, applied here rather than in `repair_content`. (M21, M68)
         *
         * It is the one repair that needs both halves, and this is the first moment both are in
         * hand. **In memory only**: nothing is marked dirty, so opening a project and reading a task
         * still rewrites nothing — `the_creator_of_a_task_from_an_older_build_is_recovered_from_its_seeded_comment`
         * asserts that byte for byte. The next real mutation persists it with everything else.
         */
        let mut task = task_of(&row, &held.content);
        if let Some(author) = recovered_creator(&row, &held.content) {
            task.created_by = author;
        }
        Ok(task)
    }

    /// Write a composed task back: its row into the index, its content into the cache. (M68)
    ///
    /// **Dirty only when the content actually changed.** That comparison is what keeps a title edit
    /// from rewriting a task's whole conversation — `TaskContent` derives `PartialEq`, the check is
    /// one memcmp-ish walk over data already in cache, and without it every status flip would touch
    /// two files and put a no-op diff in a committed file somebody reviews.
    ///
    /// The row is re-derived through [`TaskRow::of`], never assembled by hand, so the two counts are
    /// recomputed from the content that was just written and cannot drift from it — the single
    /// producer rule that [`cide_ipc::TaskDetail`]'s nesting exists to protect.
    fn put(&self, index: &mut TaskFile, cache: &mut ContentCache, task: &Task) {
        cache.note(&task.id);
        let row = TaskRow::of(task);
        if let Some(slot) = find_mut(index, &task.id) {
            *slot = row;
        } else {
            index.tasks.push(row);
        }
        let content = content_of(task);
        let held = cache
            .loaded
            .entry(task.id.clone())
            .or_insert_with(|| ContentCache::read(&self.root, &task.id));
        if held.content != content {
            held.content = content;
            cache.dirty.insert(task.id.clone());
        }
    }

    fn update<T>(
        &self,
        f: impl FnOnce(&mut TaskFile, &mut ContentCache) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self.inner.lock();
        let mut cache = self.content.lock();

        if let DiskState::Unreadable { error } = &*self.state.lock() {
            return Err(CoreError::Io(format!(
                "refusing to change {} while it is unreadable: {error}",
                self.path.display()
            )));
        }

        let before = guard.clone();
        // The content half of the rollback. (M68) The index is restored by clone — it is ~50 KiB —
        // while content is restored from an undo log, because cloning it would copy every body and
        // comment this process has loaded on every mutation, which is the cost the whole split
        // removed. `ContentCache::journal`'s doc has the argument.
        cache.begin();

        let outcome = f(&mut guard, &mut cache);
        if outcome.is_ok()
            && let Err(error) = validate(&guard)
        {
            *guard = before;
            cache.rollback();
            tracing::error!(%error, "rejected a mutation that broke a task file invariant");
            return Err(error);
        }
        if outcome.is_err() {
            // An operation that reports failure should not have changed anything, but restoring
            // costs one clone and removes the question.
            *guard = before;
            cache.rollback();
            return outcome;
        }

        cache.commit();
        guard.rev += 1;
        drop(cache);
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
        self.update(move |file, cache| {
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
            // Through `put` (M68): the row appends to the index — at the end, because the array
            // *is* the order and new work goes at the bottom of the list rather than jumping the
            // queue the user is reading top to bottom — and the content lands in the cache, dirty,
            // so the flush writes `.cide/tasks/<id>/task.json` beside the index.
            //
            // A fresh task's content is `TaskContent::default()` in all but the body, and `put`
            // marks it dirty only if it differs from what is held. For a brand-new id nothing is
            // held, so `ContentCache::read` is asked and answers the default — which is equal to a
            // new task's content whenever the body is empty. That is correct and deliberate: a task
            // created with no description has no content file until it gets one, and a row with no
            // content file is a state the reader already treats as ordinary.
            self.put(file, cache, &task);
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
        let task = self.update(move |file, cache| {
            // The two link arms route through free functions over the whole index before the task
            // is composed below: the rules they enforce — the target's existence, the cycle walk,
            // `related`'s inverse edge — live on *other* tasks, and all three read only the links a
            // row carries.
            // No author gate, unlike the comment arms: an edge is recorded intent, not a
            // rewrite of the record, and what keeps a run's `blockedBy` from dispatching
            // anything is `autodispatch`'s author gate downstream, not ownership here.
            let edit = match edit {
                TaskEdit::Link { link, target } => {
                    let row = apply_link(file, id, link, &target, now)?;
                    // Composed on the way out because the caller is promised a whole `Task` — and
                    // `apply_link` answers with a row, which is all it has and all it needs.
                    return self.compose(file, cache, &row.id);
                }
                TaskEdit::Unlink { link, target } => {
                    let row = apply_unlink(file, id, link, &target, now)?;
                    return self.compose(file, cache, &row.id);
                }
                other => other,
            };
            /*
             * Composed, so every arm below still operates on a whole `&mut Task`. (M68)
             *
             * Each of these twelve arms carries a comment recording a decision that was paid for —
             * the assignee/session exclusion, the no-op `SetStatus`, the author gates on the comment
             * arms. Rewriting them to reach into a row and a content struct separately would have
             * moved all of that prose and risked all of those decisions, to save one small file read
             * on the arms that touch no content. `compose`'s doc has the argument; `put` below is
             * what splits the result back, and it marks the content dirty only if it changed, so a
             * `SetTitle` still writes one file.
             */
            let task = &mut self.compose(file, cache, id)?;
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
            self.put(file, cache, task);
            validate_content(&task.id, &content_of(task))?;
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
        let mut cache = self.content.lock();
        // Composed rather than read off the row, because the comment this may be naming is content.
        // The load is the point of the check: it happens *before* `attachments::import` copies a
        // byte, which is the ordering `attach`'s doc states — a refusal here costs nothing on disk.
        let task = self.compose(&guard, &mut cache, id)?;
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
        let outcome = self.update(move |file, cache| {
            let task = &mut self.compose(file, cache, id)?;
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
            self.put(file, cache, task);
            validate_content(&task.id, &content_of(task))?;
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
        self.update(|file, cache| {
            let before = file.tasks.len();
            file.tasks.retain(|task| &task.id != id);
            if file.tasks.len() == before {
                return Err(no_such_task(id));
            }
            /*
             * The content goes with the row, and the directory with it. (M68)
             *
             * Forgotten from the cache *and* removed from disk — `write_now` does the removal, which
             * is where every other write happens; recording the intent here keeps the closure free of
             * I/O, which is `update`'s standing rule and what `TaskEdit::DetachAttachment` already
             * follows.
             *
             * The tempting symmetry with `merge` is a bug, and it is worth naming. A merge *adopts* a
             * row that exists only on disk, because a task nobody deleted must not vanish. A content
             * directory that no row names is the opposite: there is no task tombstone in this format
             * (see `merge`'s own doc on why a delete does not survive a concurrent write), so an
             * orphan directory is overwhelmingly a deleted task's leftovers — and adopting one would
             * resurrect every task anybody had ever deleted, at the next open. So orphans are ignored
             * by the reader and removed here, by the delete that made them.
             */
            cache.note(id);
            cache.loaded.remove(id);
            cache.dirty.remove(id);
            cache.removed.insert(id.clone());
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
        let mut cache = self.content.lock();
        let mut state = self.state.lock();

        // Layer 3, before every write and not only on the debounced path: a `git pull` between two
        // explicit saves is the same race as one between two ticks.
        let merged = self.reconcile(&mut guard, &mut cache, &mut state);

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

        // Written as the schema the content needs, not the one this build knows (M83): a tracker
        // with nothing in the inbox stays readable by a build that predates it. The field is put
        // back straight after, under the same lock, because `validate` and every reader in this
        // process hold the in-memory file to `CURRENT_SCHEMA`.
        guard.schema_version = guard.schema_on_disk();
        let encoded = serde_json::to_vec_pretty(&*guard);
        guard.schema_version = TaskFile::CURRENT_SCHEMA;
        let mut bytes = match encoded {
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

        /*
         * Content first, then the index. (M68)
         *
         * The order is the one that survives being interrupted. Two files is two publishes with no
         * transaction between them, so a crash, a SIGKILL or a `git add -A` can land between them —
         * and the pair has to be readable either way round.
         *
         * Content-then-index means the worst interrupted state is an index that does not yet mention
         * a task whose content file is already on disk: an **orphan directory**, which the reader
         * ignores and the next successful flush overwrites. Index-first would instead leave a row
         * pointing at content that was never written — a task whose body and whole conversation read
         * as empty, which is the one failure here that looks like data loss to the person reading it.
         *
         * A content write that fails does **not** abort the index write. The row is still the truth
         * about the task's existence and its status, and refusing to record that because one comment
         * file could not be written would turn a disk hiccup into a lost status change. The failure
         * is logged and the debounce re-armed, so the next tick tries the content again.
         */
        let mut content_failed = false;
        for id in std::mem::take(&mut cache.dirty) {
            let Some(held) = cache.loaded.get_mut(&id) else {
                // Dirty with nothing loaded cannot happen — `put` inserts before it marks — and if
                // it ever does, writing nothing is the safe half: invariant 1 says a file this
                // process has not loaded is a file it must not write.
                tracing::error!(id = %id, "a task was dirty with no content held; not writing it");
                continue;
            };
            match write_content(&self.root, &id, &held.content) {
                Ok(stamp) => held.stamp = stamp,
                Err(error) => {
                    tracing::error!(
                        id = %id,
                        %error,
                        "failed to save a task's content; the index is still written and the next tick retries"
                    );
                    // Re-dirtied so the retry has something to find. `std::mem::take` above cleared
                    // the set, which is what makes this the only way back in.
                    cache.dirty.insert(id);
                    content_failed = true;
                }
            }
        }
        for id in std::mem::take(&mut cache.removed) {
            remove_content(&self.root, &id);
        }

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
        if content_failed {
            self.debounce.note_change();
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
        let mut cache = self.content.lock();
        let mut state = self.state.lock();
        let merged = self.reconcile(&mut guard, &mut cache, &mut state);
        if merged.is_some() {
            self.debounce.note_change();
        }
        merged
    }

    /// Layer 3: re-`stat`, and reload-and-merge when the file has moved under us.
    ///
    /// Returns the merged file when the in-memory copy actually changed, and `None` when there was
    /// nothing to do — which is the overwhelmingly common case, one `stat` per write.
    /// Merge in whatever happened to a dirty task's content file behind our back. (M68)
    ///
    /// The per-task half of [`Self::reconcile`], and the shape is the index's rule applied one level
    /// down: compare the stamp we recorded when we last read or wrote the file, and if it moved,
    /// re-read and [`merge_content`] rather than overwrite.
    ///
    /// Three things here are load-bearing.
    ///
    /// **A file that will not read is not merged as empty.** `ContentCache::read` answers
    /// `TaskContent::default()` for an unreadable file, and unioning *that* as `theirs` would read as
    /// the other side having deleted every comment — and `union_comments` keeps whatever either side
    /// has, so the damage would not even be visible as a loss until the next write persisted our copy
    /// over theirs. The `stamp.is_none()` guard is what keeps such a read out of the merge: no stamp
    /// means the read failed, and a failed read has nothing to say about what is on disk.
    ///
    /// **The row's `updated_unix_ms` is raised** from the merged content, through
    /// [`newest_content_stamp`]. That is invariant 4: a comment adopted from the losing side is an
    /// update, and a row that predates its own newest comment loses the next merge it takes part in
    /// and sorts below tasks nothing has happened to.
    ///
    /// **The counts are recomputed** from the merged content, because the row is re-derived through
    /// `TaskRow::of`. This is where the one knowing inaccuracy `merge_row` leaves behind gets corrected.
    fn reconcile_content(&self, file: &mut TaskFile, cache: &mut ContentCache) {
        let dirty: Vec<TaskId> = cache.dirty.iter().cloned().collect();
        for id in dirty {
            let Some(held) = cache.loaded.get(&id) else {
                continue;
            };
            let path = content_path(&self.root, &id);
            let current = document::stamp_at(&path);
            if current == held.stamp {
                continue;
            }
            let disk = ContentCache::read(&self.root, &id);
            if disk.stamp.is_none() {
                // The read failed. Leaving ours dirty and untouched is the conservative answer: the
                // next tick tries again, and nothing was merged against a copy we could not trust.
                continue;
            }
            let merged = merge_content(&held.content, &disk.content);
            if let Some(held) = cache.loaded.get_mut(&id) {
                held.content = merged;
                // Deliberately **not** stamped with what we just read: our merged copy is not what is
                // on disk, and claiming otherwise would let the flush skip writing it. The stamp
                // moves when `write_content` succeeds.
                held.stamp = None;
            }
            let Some(content) = cache.loaded.get(&id).map(|h| h.content.clone()) else {
                continue;
            };
            if let Some(row) = find_mut(file, &id) {
                let stamp = newest_content_stamp(&content);
                let joined = task_of(row, &content);
                *row = TaskRow::of(&joined);
                if let Some(newest) = stamp {
                    row.updated_unix_ms = row.updated_unix_ms.max(newest);
                }
            }
            tracing::info!(
                id = %id,
                path = %path.display(),
                "a task's content changed outside this process; merged it"
            );
        }
    }

    fn reconcile(
        &self,
        file: &mut TaskFile,
        cache: &mut ContentCache,
        state: &mut DiskState,
    ) -> Option<TaskFile> {
        // The content half, first and unconditionally. (M68)
        //
        // Unconditionally because it is keyed on each dirty task's own stamp rather than on the
        // index's: a teammate can push a comment on `t-14` without the index moving at all — a
        // content file changed and the row did not — and a preflight that only watched
        // `.cide/tasks.json` would overwrite it without ever looking.
        //
        // **Dirty tasks only**, which is invariant 1 paying for itself: a task this process never
        // loaded is a task it will never write, so there is nothing to merge and theirs is simply
        // still on disk. Before the split every write re-read and merged the whole tracker; on an
        // ordinary afternoon this loop now runs once.
        self.reconcile_content(file, cache);

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
            ReadOutcome::Ready {
                file: disk,
                stamp,
                migrated,
            } => {
                /*
                 * A schema 1 file appeared under a running cide. (M68)
                 *
                 * Reachable, and a silent loss if ignored: `git checkout` of a branch that predates
                 * the conversion puts one back while the store is open, and the content the ladder
                 * lifted out of it would then exist only in a value this arm dropped on the floor.
                 * The index would merge fine and every one of those tasks would read as having no
                 * body and no conversation.
                 *
                 * So it is seeded and marked dirty, exactly as `open` and `reload` do — and
                 * **merged**, not overwritten, for anything this process already holds: our copy is
                 * the one with the edits that have not been flushed yet.
                 */
                if let Some(lifted) = migrated {
                    for (id, body) in lifted {
                        let merged = match cache.loaded.get(&id) {
                            Some(held) => merge_content(&held.content, &body),
                            None => body,
                        };
                        cache.dirty.insert(id.clone());
                        cache.loaded.insert(
                            id,
                            Held {
                                content: merged,
                                stamp: None,
                            },
                        );
                    }
                }
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

    /// A whole tracker as these tests think of one: the index, and the content beside it. (M68)
    ///
    /// # Why the tests keep a shape the store does not have
    ///
    /// Because every rule in the merge table is a rule about a **task** — "a tombstone wins
    /// whichever side it is on", "the stale side's comment is kept" — and none of them is a rule
    /// about which file the bytes happened to be in. Splitting the table along the storage seam
    /// would have rewritten thirteen cases and every `why` string in them to say something narrower
    /// than what they are actually asserting.
    ///
    /// So the tests hold both halves and [`Tracker::merge`] does exactly what `TaskStore` does:
    /// `merge_row` over the index, `merge_content` per task, and the row's stamp raised from the
    /// merged content. If those three ever stop composing the way the store composes them, every
    /// row of the table fails at once — which is the property that makes this shape a test helper
    /// rather than a second implementation.
    struct Tracker {
        index: TaskFile,
        content: HashMap<TaskId, TaskContent>,
    }

    impl Tracker {
        /// One task, composed — what an assertion reads.
        fn task(&self, id: &str) -> Task {
            let id = TaskId(id.to_string());
            let row = find(&self.index, &id).expect("no such task in this tracker");
            task_of(
                &row.clone(),
                self.content.get(&id).unwrap_or(&EMPTY_CONTENT),
            )
        }

        /// Every task, in the index's order.
        fn tasks(&self) -> Vec<Task> {
            self.index
                .tasks
                .iter()
                .map(|row| task_of(row, self.content.get(&row.id).unwrap_or(&EMPTY_CONTENT)))
                .collect()
        }

        /// The store's own merge, both halves, in the store's own order.
        fn merge(&self, theirs: &Tracker) -> Tracker {
            let mut index = merge(&self.index, &theirs.index);
            let mut content = HashMap::new();
            for row in &mut index.tasks {
                let mine = self.content.get(&row.id).unwrap_or(&EMPTY_CONTENT);
                let disk = theirs.content.get(&row.id).unwrap_or(&EMPTY_CONTENT);
                let merged = merge_content(mine, disk);
                // Invariant 4, applied exactly where `TaskStore::reconcile_content` applies it: a
                // comment adopted from the losing side is an update, and a row that predates its own
                // newest comment loses the next merge it takes part in.
                if let Some(newest) = newest_content_stamp(&merged) {
                    row.updated_unix_ms = row.updated_unix_ms.max(newest);
                }
                let joined = task_of(row, &merged);
                *row = TaskRow::of(&joined);
                content.insert(row.id.clone(), merged);
            }
            Tracker { index, content }
        }
    }

    /// A task with nothing in its content file, which is also a task with no content file at all.
    static EMPTY_CONTENT: TaskContent = TaskContent {
        body: String::new(),
        comments: Vec::new(),
        history: Vec::new(),
        attachments: Vec::new(),
    };

    /// Read a whole tracker off disk the way `TaskStore::open` does: the index, then each task's
    /// own file. (M68)
    ///
    /// The tests that use this are the layer-3 and layer-4 ones — the ones whose whole point is that
    /// what ended up *on disk* is right. Reading only the index would leave them asserting about the
    /// half of the tracker that was never in question.
    fn read_tracker(root: &Path) -> Tracker {
        let index = match read(&tasks_path(root)) {
            ReadOutcome::Ready { file, .. } => file,
            other => panic!("expected Ready, got {other:?}"),
        };
        let content = index
            .tasks
            .iter()
            .map(|row| (row.id.clone(), ContentCache::read(root, &row.id).content))
            .collect();
        Tracker { index, content }
    }

    fn a_file(rev: u64, tasks: Vec<Task>) -> Tracker {
        let mut index = TaskFile {
            schema_version: TaskFile::CURRENT_SCHEMA,
            rev,
            next_id: 0,
            tasks: tasks.iter().map(TaskRow::of).collect(),
        };
        settle_next_id(&mut index);
        let content = tasks
            .iter()
            .map(|task| (task.id.clone(), content_of(task)))
            .collect();
        Tracker { index, content }
    }

    fn titles(file: &Tracker) -> Vec<&str> {
        file.index.tasks.iter().map(|t| t.title.as_str()).collect()
    }

    fn ids(file: &Tracker) -> Vec<&str> {
        file.index.tasks.iter().map(|t| t.id.as_str()).collect()
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
        mine: Tracker,
        theirs: Tracker,
        check: fn(&Tracker),
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

        let merged = mine.merge(&stale);
        let comments = &merged.task("t-1").comments;
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
        let after = a_file(3, vec![with_comments("t-1", 1, vec![gone])]).merge(&stale);
        let comments = &after.task("t-1").comments;
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
            let merged = a_file(1, vec![with_comments("t-1", 1, vec![mine.clone()])]).merge(
                &a_file(1, vec![with_comments("t-1", 1, vec![theirs.clone()])]),
            );
            assert!(
                merged.task("t-1").comments[0].deleted,
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
        // `repair_content` rather than `repair` since M68: a comment's id is content, and the index
        // half of repair has no comment to look at. The claim is unchanged — two readers of one
        // legacy file derive the *same* id without either of them writing.
        let id_of = |raw: &TaskComment| {
            let mut content = content_of(&with_comments("t-1", 1, vec![raw.clone()]));
            repair_content(&TaskId("t-1".into()), &mut content);
            content.comments[0].id.clone()
        };
        let a_id = id_of(&raw);
        let b_id = id_of(&raw);

        let id = &a_id;
        assert!(!id.is_empty(), "repair fills in every empty id");
        assert_eq!(
            id, &b_id,
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
        let repaired = |raw: &TaskComment| {
            let mut task = with_comments("t-1", 1, vec![raw.clone()]);
            let mut content = content_of(&task);
            repair_content(&task.id, &mut content);
            task.comments = content.comments;
            a_file(1, vec![task])
        };
        let merged = repaired(&raw).merge(&repaired(&raw));
        assert_eq!(
            merged.task("t-1").comments.len(),
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
                check: |out| assert_eq!(out.task("t-1").comments.len(), 1),
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
                    let task = out.task("t-1");
                    let texts: Vec<&str> = task.comments.iter().map(|c| c.text.as_str()).collect();
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
                        out.task("t-1").status,
                        TaskStatus::Review,
                        "the newer side's status"
                    );
                    assert_eq!(out.task("t-1").comments.len(), 2, "both comments");
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
                    assert_eq!(out.task("t-1").created_by, TaskAuthor::Orchestrator);
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
                    assert_eq!(out.task("t-1").created_by, TaskAuthor::Orchestrator);
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
                    let task = &out.task("t-1");
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
                check: |out| assert!(!out.task("t-1").links[0].deleted),
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
                check: |out| assert_eq!(out.task("t-1").links.len(), 2),
            },
        ];

        for case in cases {
            let out = case.mine.merge(&case.theirs);
            (case.check)(&out);
            assert_eq!(
                out.index.rev,
                case.mine.index.rev.max(case.theirs.index.rev) + 1,
                "{}: rev must land past both sides, or a window holding either one keeps its \
                 stale snapshot ({})",
                case.name,
                case.why
            );
            assert_eq!(
                out.index.schema_version,
                TaskFile::CURRENT_SCHEMA,
                "{}: the merge writes this build's schema",
                case.name
            );
            assert!(
                validate(&out.index).is_ok(),
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

        let once = mine.merge(&theirs);
        let twice = once.merge(&theirs);
        assert_eq!(
            once.tasks(),
            twice.tasks(),
            "the merge is idempotent over content"
        );
        assert!(twice.index.rev > once.index.rev);
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
        assert_eq!(ids(&mine.merge(&theirs)), ["t-3", "t-1", "t-7"]);
    }

    // --- the board's row, and searching what it no longer carries (M68) -----------------------

    #[test]
    fn a_row_counts_what_is_live_and_nothing_else() {
        let mut task = a_task("t-1", "counted", 10);

        // Body attachments: one live, one tombstoned.
        task.attachments = vec![an_attachment("a-1", "keep.png", 10), {
            let mut gone = an_attachment("a-2", "gone.png", 11);
            gone.deleted = true;
            gone
        }];

        // Three comments: one plain, one tombstoned, one live and carrying two files of which one
        // is itself tombstoned.
        let mut tombstoned = a_comment("removed", 20);
        tombstoned.deleted = true;
        // A tombstoned comment's own attachments are tombstoned with it by `DeleteComment`, but the
        // count must not depend on that having happened: a hand-edited file can carry a live record
        // under a dead comment, and the comment is what decides.
        tombstoned.attachments = vec![an_attachment("a-3", "orphan.png", 21)];
        let mut carrying = a_comment("has files", 30);
        carrying.attachments = vec![an_attachment("a-4", "one.png", 31), {
            let mut gone = an_attachment("a-5", "two.png", 32);
            gone.deleted = true;
            gone
        }];
        task.comments = vec![a_comment("plain", 10), tombstoned, carrying];

        let row = TaskRow::of(&task);
        assert_eq!(
            row.comment_count, 2,
            "a tombstoned comment is one the user removed and `adapt.ts` drops it at the seam, so \
             counting it puts a `3 comment(s)` on a card showing two"
        );
        assert_eq!(
            row.attachment_count, 2,
            "one live on the body and one on a live comment — the dead comment's record does not \
             count however it is spelled, because the comment is what the panel drew"
        );
    }

    #[test]
    fn a_row_carries_every_field_a_list_reads_and_none_it_does_not() {
        let mut task = a_task("t-9", "titled", 77);
        task.body = "a whole statement of work".into();
        task.status = TaskStatus::Doing;
        task.agent = Some(AgentId("developer".into()));
        task.change = Some(ChangeName("add-dark-mode".into()));
        task.links = vec![TaskLink {
            link: LinkType::BlockedBy,
            target: TaskId("t-3".into()),
            deleted: false,
            at_unix_ms: 5,
        }];
        task.created_by = TaskAuthor::Orchestrator;

        let row = TaskRow::of(&task);
        assert_eq!(row.id, task.id);
        assert_eq!(row.title, task.title);
        assert_eq!(row.status, task.status);
        assert_eq!(row.agent, task.agent);
        assert_eq!(row.change, task.change);
        assert_eq!(row.created_by, task.created_by);
        assert_eq!(row.created_unix_ms, task.created_unix_ms);
        assert_eq!(
            row.updated_unix_ms, task.updated_unix_ms,
            "the merge tiebreak and the panel's in-group sort key — a row that dropped it would \
             quietly stop reordering"
        );
        assert_eq!(
            row.links, task.links,
            "links stay on the row because `blocks` is derived by scanning every other task's \
             list, and auto-dispatch reads a blocker's status: a reader that had to open a file \
             per task to answer `is this blocked` would load the whole board to draw a list"
        );
    }

    #[test]
    fn the_board_hands_out_rows_and_keeps_its_revision() {
        let dir = TempDir::new("board-rows");
        let store = TaskStore::open(&dir.0);
        let mut req = new_task("Add dark mode");
        req.body = Some("the long version".into());
        let task = store.create(&req, TaskAuthor::User).expect("created");
        store
            .edit(
                &task.id,
                TaskEdit::Comment {
                    text: "started".into(),
                },
                TaskAuthor::User,
            )
            .expect("commented");

        let TaskBoard::Ready { tasks, rev } = store.board() else {
            panic!("a tracker with a task in it is ready");
        };
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task.id);
        assert_eq!(tasks[0].comment_count, 1);
        assert!(
            rev > 0,
            "`rev` must stay on the ready arm: the receiving store's drop rule is \
             `Number.isFinite(next.rev) && next.rev > current.rev`, so a missing one judges every \
             snapshot not-newer and freezes the panel for the rest of the session"
        );
    }

    #[test]
    fn search_reads_the_id_the_title_and_the_body_but_never_the_comments() {
        let mut body = a_task("t-1", "nothing in the title", 10);
        body.body = "the retry ladder".into();
        let mut commented = a_task("t-2", "also nothing", 10);
        commented.comments = vec![a_comment("the retry ladder again", 20)];
        let tracker = a_file(1, vec![body, a_task("t-3", "the RETRY bar", 10), commented]);
        let file = &tracker.index;
        /*
         * The bodies arrive through a closure. (M68)
         *
         * That is the seam the store reaches through — it owns the policy about how many content
         * files to open, and a search is the one operation that wants all of them — and it is what
         * keeps this test able to state *id, title, body, never the comments* with no disk anywhere
         * near it. The closure below is the whole of what a content file contributes.
         */
        let bodies = &tracker.content;
        let body_of = |id: &TaskId| bodies.get(id).map(|c| c.body.clone());
        let search = |query: &str| search(file, &body_of, query);

        assert_eq!(
            search("retry"),
            [TaskId("t-1".into()), TaskId("t-3".into())],
            "the body and the title match, case-insensitively; a comment does not — a query would \
             otherwise match on a word an agent used in a progress note, which is not what the \
             person asking meant"
        );
        assert_eq!(
            search("t-3"),
            [TaskId("t-3".into())],
            "the id matches, because quoting one back is how a task is found after an agent named it"
        );
        assert_eq!(
            search("   "),
            [
                TaskId("t-1".into()),
                TaskId("t-3".into()),
                TaskId("t-2".into())
            ],
            "an empty or blank query matches everything, in the file's own order: the panel's `no \
             filter` state must never be spelled as a query that matches nothing, which would draw \
             `nothing matched` over a full board"
        );
        assert!(
            search("absent").is_empty(),
            "and a real miss is still a miss"
        );
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

    /// The schema number on disk follows the content, not the build. (M83)
    ///
    /// A build that predates `Inbox` reads `"status": "inbox"` as an unparseable file and sets it
    /// aside; a newer schema number is a clean refusal instead. So a tracker is written as 3 only
    /// while something is in the inbox — and a schema 2 file is opened *without* being rewritten,
    /// or the installed cide beside a development one would be locked out of every tracker the
    /// development one had merely looked at.
    #[test]
    fn the_schema_on_disk_is_three_only_while_something_is_in_the_inbox() {
        let dir = TempDir::new("inbox-schema");
        let schema = |dir: &TempDir| -> u64 {
            let raw = fs::read_to_string(dir.tasks()).expect("written");
            let value: Value = serde_json::from_str(&raw).expect("json");
            value["schemaVersion"].as_u64().expect("a number")
        };

        let store = TaskStore::open(dir.root());
        let plain = store
            .create(&new_task("plain"), TaskAuthor::User)
            .expect("created");
        store.write_now();
        assert_eq!(
            schema(&dir),
            2,
            "no inbox row: an older build can still read it"
        );

        let mut noticed = new_task("noticed in passing");
        noticed.status = Some(TaskStatus::Inbox);
        let noticed = store.create(&noticed, TaskAuthor::User).expect("created");
        store.write_now();
        assert_eq!(
            schema(&dir),
            3,
            "an inbox row: an older build must refuse, not quarantine"
        );

        store
            .edit(
                &noticed.id,
                TaskEdit::SetStatus {
                    status: TaskStatus::Todo,
                },
                TaskAuthor::User,
            )
            .expect("promoted");
        store.write_now();
        assert_eq!(
            schema(&dir),
            2,
            "the inbox emptied, so the file is readable again"
        );
        drop(store);

        let before = fs::read_to_string(dir.tasks()).expect("read");
        let reopened = TaskStore::open(dir.root());
        assert!(reopened.get(&plain.id).is_some());
        assert_eq!(
            fs::read_to_string(dir.tasks()).expect("read"),
            before,
            "opening a schema 2 tracker is not a conversion and writes nothing"
        );
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
        assert_eq!(
            store
                .snapshot()
                .tasks
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            ["first"]
        );
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
        //
        // Through the ladder since M68 rather than straight into `TaskFile`: a schema 1 document no
        // longer *has* the current shape, and the claim this test makes — that an older file still
        // opens, with the fields it never had at their defaults — is now a claim about the migration.
        let json = r#"{"schemaVersion":1,"rev":3,"tasks":[{"id":"t-1","title":"old",
            "body":"","status":"todo","agent":null,"comments":[],
            "createdUnixMs":1,"updatedUnixMs":1}]}"#;
        let value: Value = serde_json::from_str(json).expect("valid JSON");
        let (migrated, content) = migrate(value, 1).expect("an older file still migrates");
        let file: TaskFile = serde_json::from_value(migrated).expect("and parses");
        // The ladder stops at 2; `read` lifts the in-memory number to `CURRENT_SCHEMA` (M83).
        assert_eq!(file.schema_version, TaskFile::SCHEMA_WITHOUT_INBOX);
        assert_eq!(file.tasks[0].change, None);
        assert!(
            file.tasks[0].links.is_empty(),
            "and no links, same posture (M30)"
        );
        assert_eq!(
            file.tasks[0].comment_count, 0,
            "the counts are derived from the content the migration lifted, not invented"
        );
        let content = content.expect("a migrated file hands back its content");
        assert_eq!(
            content[&TaskId("t-1".into())],
            TaskContent::default(),
            "a task that had nothing but a title migrates to empty content"
        );
    }

    /// A schema 1 tracker becomes an index plus one file per task, and nothing is lost on the way.
    #[test]
    fn a_schema_1_tracker_migrates_to_an_index_and_a_file_per_task() {
        let json = r#"{"schemaVersion":1,"rev":9,"nextId":3,"tasks":[
            {"id":"t-1","title":"with a log","body":"the long version","status":"doing",
             "agent":"developer","createdUnixMs":1,"updatedUnixMs":2,
             "comments":[
               {"id":"c-1","author":{"kind":"user"},"text":"first","atUnixMs":10},
               {"id":"c-2","author":{"kind":"user"},"text":"gone","atUnixMs":11,"deleted":true}],
             "history":[{"from":"todo","to":"doing","by":{"kind":"user"},"atUnixMs":12}],
             "attachments":[{"id":"a-1","name":"shot.png","bytes":10,"kind":"image",
                             "addedBy":{"kind":"user"},"addedUnixMs":13}]},
            {"id":"t-2","title":"bare","body":"","status":"todo","agent":null,
             "comments":[],"createdUnixMs":1,"updatedUnixMs":1}]}"#;
        let value: Value = serde_json::from_str(json).expect("valid JSON");
        let (migrated, content) = migrate(value, 1).expect("migrates");
        let file: TaskFile = serde_json::from_value(migrated).expect("parses");
        let content = content.expect("content was lifted");

        assert_eq!(file.rev, 9, "the revision is carried, not reset");
        assert_eq!(file.next_id, 3, "and so is the id counter");
        assert_eq!(
            ids(&Tracker {
                index: file.clone(),
                content: content.clone(),
            }),
            ["t-1", "t-2"],
            "in the file's own order"
        );

        let row = &file.tasks[0];
        assert_eq!(
            row.comment_count, 1,
            "the tombstoned comment is not counted — `TaskRow::of`'s rule, applied by the migration              rather than restated by it"
        );
        assert_eq!(row.attachment_count, 1);
        assert_eq!(row.status, TaskStatus::Doing);
        assert_eq!(row.agent.as_ref().map(|a| a.as_str()), Some("developer"));

        let lifted = &content[&TaskId("t-1".into())];
        assert_eq!(lifted.body, "the long version");
        assert_eq!(
            lifted.comments.len(),
            2,
            "the tombstone is LIFTED even though it is not counted: it stays in the file so a merge              against a stale copy cannot resurrect the comment"
        );
        assert_eq!(lifted.history.len(), 1);
        assert_eq!(lifted.attachments.len(), 1);

        assert_eq!(
            content[&TaskId("t-2".into())],
            TaskContent::default(),
            "a task with nothing in it lifts to nothing, and writes no file"
        );
    }

    /// The conversion happens **on open**, writes both halves, and moves the attachment bytes.
    #[test]
    fn opening_a_schema_1_project_converts_it_and_moves_its_attachments() {
        let dir = TempDir::new("migrate-open");
        dir.plant(
            r#"{"schemaVersion":1,"rev":4,"nextId":2,"tasks":[
                {"id":"t-1","title":"has a file","body":"b","status":"todo","agent":null,
                 "comments":[{"id":"c-1","author":{"kind":"user"},"text":"hi","atUnixMs":10}],
                 "createdUnixMs":1,"updatedUnixMs":2,
                 "attachments":[{"id":"a-1","name":"shot.png","bytes":3,"kind":"image",
                                 "addedBy":{"kind":"user"},"addedUnixMs":13}]}]}"#,
        );
        // The bytes, where a schema 1 build would have put them.
        let old_dir = dir.root().join(".cide/attachments/t-1/a-1");
        fs::create_dir_all(&old_dir).expect("plant the old layout");
        fs::write(old_dir.join("shot.png"), b"png").expect("plant the bytes");

        let store = TaskStore::open(dir.root());

        let raw = fs::read_to_string(dir.tasks()).expect("the index was rewritten");
        assert!(
            raw.contains("\"schemaVersion\": 2"),
            "the index is converted on open, not on the first mutation: {raw}"
        );
        let on_disk = read_tracker(dir.root());
        let task = on_disk.task("t-1");
        assert_eq!(task.body, "b", "the body landed in the task's own file");
        assert_eq!(task.comments.len(), 1, "and so did the log");
        assert_eq!(store.snapshot().rev, 4, "conversion is not a mutation");

        let moved = dir.root().join(".cide/tasks/t-1/attachments/a-1/shot.png");
        assert!(moved.is_file(), "the bytes moved under the task");
        assert!(
            !dir.root().join(".cide/attachments").exists(),
            "and the old tree was pruned once it emptied"
        );
        assert_eq!(
            attachments::path_of(dir.root(), &task.id, &task.attachments[0]),
            moved,
            "and `path_of` finds them where they now are"
        );
    }

    /// An interrupted relocation is harmless: the reader tries the new path, then the old.
    #[test]
    fn an_attachment_left_in_the_old_layout_is_still_found() {
        let dir = TempDir::new("legacy-attachment");
        let task = TaskId("t-1".into());
        let record = an_attachment("a-1", "shot.png", 10);
        let old = dir.root().join(".cide/attachments/t-1/a-1");
        fs::create_dir_all(&old).expect("plant");
        fs::write(old.join("shot.png"), b"png").expect("plant");

        assert_eq!(
            attachments::path_of(dir.root(), &task, &record),
            old.join("shot.png"),
            "a file the conversion did not reach is still read where it is — a move interrupted by \
             a crash or a full disk must not read as a missing attachment"
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

        let merged = a_file(1, vec![mine.clone()]).merge(&a_file(1, vec![theirs.clone()]));
        let task = &merged.task("t-1");
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
        assert!(validate(&file.index).is_ok());
        file.index.next_id = 4;
        assert!(
            validate(&file.index).is_err(),
            "a counter that is not past every id will mint one of them again"
        );
    }

    #[test]
    fn the_merge_takes_the_higher_counter_from_either_side() {
        let mut mine = a_file(5, vec![a_task("t-1", "one", 1)]);
        let mut theirs = a_file(5, vec![a_task("t-1", "one", 1)]);
        mine.index.next_id = 7;
        theirs.index.next_id = 40;
        assert_eq!(mine.merge(&theirs).index.next_id, 40);
        assert_eq!(theirs.merge(&mine).index.next_id, 40);
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

        /*
         * Opening a schema 1 project **does** rewrite it, and that is the M68 change. (M68)
         *
         * This assertion used to be the reverse — that reading derived the creator in memory and
         * left the file byte-identical, so a teammate who merely opened the project produced no
         * diff. That promise cannot survive a format conversion, and the conversion was made
         * deliberate rather than lazy: one observable event at a known time, before anything else
         * touches the tracker, undone by one `git checkout .cide`. `TaskStore::open` carries the
         * argument and what it costs.
         *
         * The half of the promise that *does* survive is pinned by
         * `opening_a_schema_2_project_rewrites_nothing` below: once converted, opening a project
         * writes nothing at all — which is the property teammates actually feel, since after the
         * first conversion every subsequent open is one.
         */
        let now = fs::read_to_string(dir.tasks()).expect("read");
        assert_ne!(now, planted, "a schema 1 tracker is converted on open");
        assert!(
            now.contains("\"schemaVersion\": 2"),
            "and the conversion is what rewrote it: {now}"
        );
        assert!(
            now.contains("\"agent\": \"qa\""),
            "with the recovered creator written down once, rather than re-derived for ever: {now}"
        );
    }

    /// A **real** schema 1 tracker converts, against whatever is at `CIDE_TRACKER`.
    ///
    /// # Why this exists when the synthetic migration tests pass
    ///
    /// Because every fixture in this file was written by somebody who already knew the format. A
    /// four-month-old board has tasks from four different builds of cide in it — comments with no
    /// ids, tasks with no `createdBy`, a `nextId` that predates the field, attachment records whose
    /// bytes were moved by hand — and the conversion has to survive all of it at once. `cide-core`'s
    /// `a_real_package` takes the same shape for the same reason, and names its input the same way.
    ///
    /// Ignored, because it needs a tracker nobody's CI has:
    ///
    /// ```sh
    /// CIDE_TRACKER=/path/to/project cargo test -p cide-tasks a_real_tracker -- --ignored --nocapture
    /// ```
    ///
    /// It **copies** the project's `.cide/` into a scratch directory and converts the copy. Pointing
    /// a migration at somebody's live tracker to find out whether it works is the one way this test
    /// could cost more than it proves.
    #[test]
    #[ignore = "needs CIDE_TRACKER pointing at a real project"]
    fn a_real_tracker_converts() {
        let Ok(source) = std::env::var("CIDE_TRACKER") else {
            eprintln!("set CIDE_TRACKER to a project directory");
            return;
        };
        let source = PathBuf::from(source);
        let dir = TempDir::new("real-tracker");
        copy_tree(&source.join(".cide"), &dir.root().join(".cide"));

        let before = read(&tasks_path(dir.root()));
        let ReadOutcome::Ready { file: planned, .. } = before else {
            panic!("the copied tracker does not read: {before:?}");
        };
        let expected: Vec<TaskId> = planned.tasks.iter().map(|t| t.id.clone()).collect();
        eprintln!("{} task(s) to convert", expected.len());

        let store = TaskStore::open(dir.root());

        let after = read_tracker(dir.root());
        assert_eq!(
            after.index.schema_version,
            TaskFile::CURRENT_SCHEMA,
            "the index on disk is converted"
        );
        assert_eq!(
            after
                .index
                .tasks
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>(),
            expected,
            "every task survived, in the file's own order"
        );
        assert_eq!(
            after.index.rev, planned.rev,
            "a conversion is not a mutation and does not move the revision"
        );

        // Every count on disk agrees with the content file beside it. This is invariant 2 asked of
        // real data: the numbers were derived by the migration and the files were written by the
        // flush, and nothing after this point re-derives them.
        for row in &after.index.tasks {
            let content = &after.content[&row.id];
            let recomputed = TaskRow::of(&task_of(row, content));
            assert_eq!(
                (row.comment_count, row.attachment_count),
                (recomputed.comment_count, recomputed.attachment_count),
                "{}: the index's counts disagree with its content file",
                row.id
            );
        }

        // No attachment record points at bytes that are not there — through `path_of`, so a record
        // whose file the relocation did not reach still resolves to where it actually is.
        let mut files = 0;
        for row in &after.index.tasks {
            let task = after.task(row.id.as_str());
            for record in task
                .attachments
                .iter()
                .chain(task.comments.iter().flat_map(|c| c.attachments.iter()))
                .filter(|a| !a.deleted)
            {
                let path = attachments::path_of(dir.root(), &row.id, record);
                assert!(
                    path.is_file(),
                    "{}: {} is recorded but not on disk at {}",
                    row.id,
                    record.name,
                    path.display()
                );
                files += 1;
            }
        }
        eprintln!("{files} attachment(s) resolve");

        // And opening the converted copy again writes nothing at all.
        let index = fs::read_to_string(dir.tasks()).expect("index");
        drop(store);
        let _again = TaskStore::open(dir.root());
        assert_eq!(
            fs::read_to_string(dir.tasks()).expect("index"),
            index,
            "a second open rewrote the converted index"
        );
    }

    /// `cp -r`, for [`a_real_tracker_converts`]. Recursive, symlink-free, and good enough for a
    /// directory this test made a copy of on purpose.
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).expect("make the copy's directory");
        for entry in fs::read_dir(from).expect("read the source") {
            let entry = entry.expect("an entry");
            let kind = entry.file_type().expect("a file type");
            let target = to.join(entry.file_name());
            if kind.is_dir() {
                // `worktrees/` is one whole checkout per agent and has nothing to do with the
                // tracker; copying it would turn a 2 MB test into a 6 GB one.
                if entry.file_name() == "worktrees" {
                    continue;
                }
                copy_tree(&entry.path(), &target);
            } else if kind.is_file() {
                fs::copy(entry.path(), &target).expect("copy a file");
            }
        }
    }

    /// The surviving half of *reading does not rewrite*: an already-converted project is untouched.
    #[test]
    fn opening_a_schema_2_project_rewrites_nothing() {
        let dir = TempDir::new("open-idempotent");
        {
            let store = TaskStore::open(dir.root());
            let mut req = new_task("has a body");
            req.body = Some("and a conversation".into());
            let task = store.create(&req, TaskAuthor::User).expect("create");
            store
                .edit(
                    &task.id,
                    TaskEdit::Comment {
                        text: "a line".into(),
                    },
                    TaskAuthor::User,
                )
                .expect("comment");
            store.write_now();
        }

        let index = fs::read_to_string(dir.tasks()).expect("index");
        let content =
            fs::read_to_string(content_path(dir.root(), &TaskId("t-1".into()))).expect("content");

        // Opened again, and again — the repeat matters, because a conversion that ran every time
        // would still produce identical bytes on the second open and differ only on the first.
        for _ in 0..2 {
            let _store = TaskStore::open(dir.root());
        }

        assert_eq!(
            fs::read_to_string(dir.tasks()).expect("index"),
            index,
            "opening a converted project rewrote its index"
        );
        assert_eq!(
            fs::read_to_string(content_path(dir.root(), &TaskId("t-1".into()))).expect("content"),
            content,
            "opening a converted project rewrote a task's content"
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

        let on_disk = read_tracker(dir.root());
        assert_eq!(
            titles(&on_disk),
            ["ours", "theirs"],
            "the pulled task was adopted"
        );
        let task = on_disk.task("t-1");
        let texts: Vec<&str> = task.comments.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            ["from the teammate", "from this process"],
            "neither side's comment was lost"
        );
        assert!(
            on_disk.index.rev > 40,
            "rev landed past both sides: {}",
            on_disk.index.rev
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
        assert_eq!(
            merged
                .tasks
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            ["ours", "theirs"]
        );
        assert!(
            store.refresh_from_disk().is_none(),
            "asked twice, the second look found something new in an unchanged file"
        );

        // The merge left the in-memory board ahead of the file (`merge` lands `rev` past both
        // sides), and `refresh_from_disk` re-armed the debounce so the ordinary flusher tick —
        // not the watcher thread — writes the convergence.
        std::thread::sleep(persist::SAVE_DEBOUNCE);
        store.flush_if_due();
        let on_disk = read_tracker(dir.root());
        assert_eq!(
            on_disk.index,
            store.snapshot(),
            "the flusher converged the index to the merged board"
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
        assert_eq!(
            merged
                .tasks
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            ["ours", "theirs"]
        );
        // Asked of the **store**, not of the disk: `refresh_from_disk` merges in memory and re-arms
        // the debounce, deliberately leaving the write to the ordinary flusher tick rather than
        // doing it on a watcher thread. The comments it merged are in the store's cache; the task's
        // file on disk is still the one `write_now` left, which is exactly the state this test is
        // about. (M68)
        let task = store.get(&TaskId("t-1".into())).expect("still there");
        let texts: Vec<&str> = task.comments.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            ["from the teammate", "pending"],
            "one of the two comments was lost to the refresh"
        );

        // And the convergence reaches disk on the next tick, which is the other half of the
        // arrangement: a merge that only ever lived in memory would be lost to a crash.
        std::thread::sleep(persist::SAVE_DEBOUNCE);
        store.flush_if_due();
        let on_disk = read_tracker(dir.root());
        let written: Vec<String> = on_disk
            .task("t-1")
            .comments
            .iter()
            .map(|c| c.text.clone())
            .collect();
        assert_eq!(
            written,
            ["from the teammate", "pending"],
            "the flusher converged the task's own file too"
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

        // A task with nothing in it has **no content file**, which is deliberate and is what
        // `TaskStore::create` documents: content that equals the default is not dirty, so nothing is
        // written, and a row with no content file is a state the reader already treats as ordinary.
        // The alternative — a `task.json` of four empty fields per task — would put a file in
        // somebody's repository for every task they ever created without describing.
        assert!(
            !content_path(dir.root(), &TaskId("t-1".into())).exists(),
            "an empty task wrote a content file"
        );

        let mut described = new_task("described");
        described.body = Some("the long version".into());
        store.create(&described, TaskAuthor::User).expect("create");
        store.write_now();

        let raw = fs::read_to_string(dir.tasks()).expect("read");
        assert!(raw.ends_with("}\n"), "no trailing newline: {raw:?}");
        assert!(
            raw.lines().count() > 5,
            "one-line JSON makes every change a whole-file diff"
        );
        assert!(raw.contains("\"schemaVersion\": 2"));

        // The same two promises for a task's own file, which is committed and diffed exactly as the
        // index is. (M68) Asserted here rather than in a test of its own because it is the same
        // claim about the same reader — a person looking at a pull request.
        let content = fs::read_to_string(content_path(dir.root(), &TaskId("t-2".into())))
            .expect("a task with a body has a content file");
        assert!(
            content.ends_with("}\n"),
            "no trailing newline on a task's file: {content:?}"
        );
        assert!(
            content.lines().count() > 3,
            "one-line JSON makes every comment a whole-file diff"
        );
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
            on_disk.starts_with(attachments::dir(dir.root(), &id)),
            "since M68 a task's files live under the task: {}",
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
            !attachments::dir(dir.root(), &id).exists(),
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
        assert!(!attachments::dir(dir.root(), &id).exists());
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
        // `validate_content`/`repair_content` since M68: an attachment record is content, and the
        // index half of either has no record to look at. Every rule asserted below is unchanged.
        let id = TaskId("t-1".into());
        let content = content_of(&task);
        let text = validate_content(&id, &content)
            .expect_err("duplicate across body and comment")
            .to_string();
        assert!(text.contains("two attachments"), "{text}");

        let mut repaired = content.clone();
        repair_content(&id, &mut repaired);
        assert!(validate_content(&id, &repaired).is_ok());
        assert_eq!(repaired.attachments.len(), 1, "the first copy is kept");
        assert_eq!(
            repaired.attachments[0].id.as_str(),
            "a-1",
            "and keeps its id"
        );
        assert!(repaired.comments[0].attachments.is_empty());

        // A malformed id or name is dropped, and the surviving record keeps the id it had.
        let mut task = a_task("t-2", "files", 100);
        task.attachments.push(an_attachment("", "no-id.txt", 10));
        task.attachments.push(an_attachment("a-2", "../escape", 20));
        task.attachments.push(an_attachment("a-3", "fine.txt", 30));
        let id = TaskId("t-2".into());
        let mut content = content_of(&task);
        assert!(validate_content(&id, &content).is_err());
        repair_content(&id, &mut content);
        assert!(validate_content(&id, &content).is_ok());
        let kept: Vec<&str> = content.attachments.iter().map(|a| a.id.as_str()).collect();
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
                    assert_eq!(out.task("t-1").attachments.len(), 1);
                    assert!(
                        out.task("t-1").updated_unix_ms >= 150,
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
                    assert!(out.task("t-1").attachments[0].deleted, "deleted wins");
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
                    let comment = &out.task("t-1").comments[0];
                    assert_eq!(comment.text, "report, corrected", "the edit wins the text");
                    assert_eq!(comment.attachments.len(), 1, "and the attachment survives");
                },
            },
        ];
        for case in cases {
            let out = case.mine.merge(&case.theirs);
            (case.check)(&out);
            assert!(
                validate(&out.index).is_ok(),
                "{}: the merge must produce a file this build can write ({})",
                case.name,
                case.why
            );
        }
    }
}
