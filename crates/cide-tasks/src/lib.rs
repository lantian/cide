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
    CommentId, FileStamp, Task, TaskAuthor, TaskBoard, TaskComment, TaskEdit, TaskFile, TaskId,
    TaskNew,
};
use parking_lot::Mutex;
use serde_json::Value;

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
    }
    Ok(())
}

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
/// * **A malformed id is re-minted.** Rewriting an id normally breaks every reference to it — but
///   "malformed" here means precisely "cannot be quoted and matched back", so there were no usable
///   references to break. Dropping the task instead would lose work over a typo.
/// * **An empty title becomes `(untitled)`.** The option that lost was dropping the task, which
///   loses work; the other was leaving it, which freezes the tracker as described above. The
///   placeholder is visible in the panel and in the next diff, which is the point.
fn repair(file: &mut TaskFile) {
    // The high-water mark is read once, up front, and advanced by hand as ids are minted: a task
    // re-minted mid-pass must not be able to collide with one further down the list that has not
    // been visited yet.
    let mut high_water = high_water_mark(file);

    let mut kept: Vec<Task> = Vec::with_capacity(file.tasks.len());
    for mut task in std::mem::take(&mut file.tasks) {
        if !well_formed_id(&task.id) {
            high_water += 1;
            let minted = TaskId(format!("t-{high_water}"));
            tracing::warn!(
                was = %task.id,
                now = %minted,
                "task id was not usable as an identifier; re-minted it"
            );
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
        kept.push(task);
    }
    file.tasks = kept;
}

/// The largest `n` across every `t-<n>` in the file, and `rev`.
///
/// **`rev` is in here, and it is the half that survives a delete.** Taking the mark from the task
/// list alone is what [`TaskId`]'s doc warns about one step removed: delete the newest task and
/// the maximum drops, so the next mint reuses an id that comments, prompts and commit messages
/// still name — a second `t-17` silently re-points every one of them. `rev` only ever increases
/// (every accepted mutation bumps it, and [`merge`] sets it past both sides), and every create
/// bumps it at least once, so `max(largest id, rev)` is a mark that a delete cannot lower.
///
/// The cost is that ids are not contiguous after a delete or an out-of-process merge — `t-1`,
/// `t-2`, `t-4`. That is the correct direction to be wrong in: a gap is a curiosity, a reused id
/// is a reference that points at the wrong work with nothing anywhere to detect it.
///
/// Non-numeric ids (a hand-written `spike`) contribute nothing to the mark, which is why
/// [`next_id`] still has to check the candidate is actually free.
fn high_water_mark(file: &TaskFile) -> u64 {
    let largest = file
        .tasks
        .iter()
        .filter_map(|task| task.id.as_str().strip_prefix("t-"))
        .filter_map(|n| n.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    largest.max(file.rev)
}

/// The next free `t-<n>` for this file.
///
/// The loop is not paranoia: `high_water_mark` ignores ids that are not `t-<digits>`, and a
/// hand-edited file can perfectly legally contain `t-0007` — which parses as 7 for the mark but
/// is a different string, so a mint of `t-7` would be a genuine duplicate rather than a shadow.
pub fn next_id(file: &TaskFile) -> TaskId {
    let mut n = high_water_mark(file);
    loop {
        n += 1;
        let candidate = TaskId(format!("t-{n}"));
        if !file.tasks.iter().any(|task| task.id == candidate) {
            return candidate;
        }
    }
}

/// Look one task up by id.
pub fn find<'a>(file: &'a TaskFile, id: &TaskId) -> Option<&'a Task> {
    file.tasks.iter().find(|task| &task.id == id)
}

fn find_mut<'a>(file: &'a mut TaskFile, id: &TaskId) -> Option<&'a mut Task> {
    file.tasks.iter_mut().find(|task| &task.id == id)
}

/// `CoreError` has no `NoSuchTask` variant and this crate does not own `cide-core/src/error.rs`.
///
/// So the refusal travels as [`CoreError::Io`], which is the wrong tag: the frontend cannot branch
/// on it, and every other "no such X" in the domain (`NoSuchTab`, `NoSuchPane`, `NoSuchSplit`) is a
/// tagged variant precisely so that it can. Adding `NoSuchTask(TaskId)` beside them is the right
/// end state and is a one-line change in a file this crate is not allowed to edit; it is flagged
/// rather than worked around, because a bespoke error type here would be a *second* error vocabulary
/// crossing the same IPC boundary.
fn no_such_task(id: &TaskId) -> CoreError {
    CoreError::Io(format!("no such task: {id}"))
}

/// A comment that is not there, or is already tombstoned.
///
/// One message for both, because they are one answer to the caller: the comment you named is not
/// something you can act on. Telling them apart would let a caller probe which ids used to exist.
fn no_such_comment(id: &CommentId) -> CoreError {
    CoreError::Io(format!("no such comment: {id}"))
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
/// | comments | **unioned**, by `(author, at_unix_ms, text)` |
/// | one side deleted what the other edited | the task **survives**, with the edit |
///
/// ## Why comments union on `(author, at_unix_ms, text)`
///
/// [`TaskComment`] has no id — deliberately, because it is append-only and nothing ever addresses
/// one — so identity has to come from the value, and those three fields *are* the value; there is
/// no fourth. Two comments equal in all three are indistinguishable to every reader there is (the
/// panel, the next agent, a reviewer reading the diff), so collapsing them cannot lose information
/// that anything could have used. The false-collapse case — one author posting byte-identical text
/// inside the same millisecond — would be invisible in the panel anyway.
///
/// The alternative, appending both sides unconditionally, is not merely untidy: this merge runs
/// again on the next out-of-process write, so every surviving duplicate is re-duplicated, and a
/// noisy afternoon of `git pull`s turns a five-line log into fifty.
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
    // The creation stamp is the earlier of the two by definition: a task cannot have been created
    // twice, and if the two disagree one of them was hand-edited. The earlier is the safer read.
    winner.created_unix_ms = mine.created_unix_ms.min(theirs.created_unix_ms);
    // `updated_unix_ms` follows the winner already, but a comment adopted from the loser is itself
    // an update, and a merged task whose stamp predates its own newest comment would lose the next
    // merge it takes part in.
    if let Some(newest) = winner.comments.iter().map(|c| c.at_unix_ms).max() {
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
fn reconcile_comment(mine: &TaskComment, theirs: &TaskComment) -> TaskComment {
    if mine.deleted || theirs.deleted {
        let mut out = if mine.deleted {
            mine.clone()
        } else {
            theirs.clone()
        };
        out.deleted = true;
        out.text = String::new();
        return out;
    }
    let stamp = |c: &TaskComment| c.edited_at_unix_ms.unwrap_or(c.at_unix_ms);
    if stamp(theirs) > stamp(mine) {
        theirs.clone()
    } else {
        mine.clone()
    }
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
        }
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
    /// # Why `author` seeds a comment, and only for an agent
    ///
    /// [`Task`] has no `creator` field, deliberately — it is one more field an agent must be taught
    /// to fill and one more column the panel must draw. But *who asked for this task* is a real
    /// question when six subagents are writing, and the append-only log is exactly where "who did
    /// what, when" already lives.
    ///
    /// So a task created by an agent or the orchestrator opens its log with one line naming them,
    /// and a task the user created by hand opens with nothing. The condition is not arbitrary: the
    /// user was looking at the panel when they made it and already knows: telling them would be one
    /// line of noise on every hand-made task, in a log that is read.
    pub fn create(&self, req: &TaskNew, author: TaskAuthor) -> Result<Task> {
        let now = persist::now_ms();
        self.update(move |file| {
            let mut task = Task {
                id: next_id(file),
                // Trimmed, because a title is one line drawn in a 320px panel and trailing space is
                // invisible there but not in the file's diff.
                title: req.title.trim().to_string(),
                body: req.body.clone().unwrap_or_default(),
                // `Todo` unless the caller named one, which only the user's compose dialog does —
                // see `TaskNew::status` for why that is the user's field and structurally not an
                // agent's.
                status: req.status.unwrap_or(cide_ipc::TaskStatus::Todo),
                agent: req.agent.clone(),
                comments: Vec::new(),
                created_unix_ms: now,
                updated_unix_ms: now,
            };
            if !matches!(author, TaskAuthor::User) {
                task.comments.push(TaskComment {
                    id: CommentId::new(),
                    author,
                    text: "created this task".to_string(),
                    at_unix_ms: now,
                    edited_at_unix_ms: None,
                    deleted: false,
                });
            }
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
        self.update(move |file| {
            let task = find_mut(file, id).ok_or_else(|| no_such_task(id))?;
            match edit {
                TaskEdit::SetTitle { title } => task.title = title.trim().to_string(),
                TaskEdit::SetBody { body } => task.body = body,
                TaskEdit::SetStatus { status } => task.status = status,
                TaskEdit::Assign { agent } => task.agent = agent,
                // **Appends.** There is no edit and no delete, here or on the wire, and that single
                // restriction is what makes the log a channel between agents rather than a
                // scratchpad — see `TaskComment`.
                TaskEdit::Comment { text } => task.comments.push(TaskComment {
                    id: CommentId::new(),
                    author,
                    text,
                    // Stamped here rather than taken from the caller: a timestamp an agent supplies
                    // is one it can get wrong, and this one orders the log and decides the merge.
                    at_unix_ms: now,
                    edited_at_unix_ms: None,
                    deleted: false,
                }),
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
                }
            }
            task.updated_unix_ms = now;
            Ok(task.clone())
        })
    }

    /// Remove a task.
    ///
    /// The id is **not** returned to the pool — see `high_water_mark`. Deliberately not reachable
    /// from an MCP tool either: the file is the shared record of what happened, and deletion belongs
    /// to the user.
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
            created_unix_ms: 1_000,
            updated_unix_ms: updated,
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
        }
    }

    fn a_file(rev: u64, tasks: Vec<Task>) -> TaskFile {
        TaskFile {
            schema_version: TaskFile::CURRENT_SCHEMA,
            rev,
            tasks,
        }
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
        assert_eq!(file.tasks[1].title, "(untitled)");
        // The whole point of repairing: `update` validates and rolls back, so a file that stayed
        // invalid would refuse every later edit with nothing on screen saying why.
        assert!(validate(&file).is_ok());
    }

    // --- layer 1: the operations ---------------------------------------------------------------

    fn new_task(title: &str) -> TaskNew {
        TaskNew {
            project: cide_ipc::ProjectId::new(),
            title: title.to_string(),
            body: None,
            agent: None,
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
        assert_ne!(third.id, second.id, "the id of a deleted task came back");

        // And it survives a restart, which is what taking `rev` into the high-water mark buys: a
        // mark read from the task list alone would drop back to 1 the moment `t-2` was removed.
        store.write_now();
        let reopened = TaskStore::open(dir.root());
        let fourth = reopened
            .create(&new_task("four"), TaskAuthor::User)
            .expect("four");
        assert!(
            ![first.id.as_str(), second.id.as_str(), third.id.as_str()]
                .contains(&fourth.id.as_str()),
            "{} collided after a reopen",
            fourth.id
        );
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

    #[test]
    fn a_task_an_agent_created_says_who_asked_for_it() {
        let dir = TempDir::new("author");
        let store = TaskStore::open(dir.root());
        let task = store
            .create(
                &new_task("write the tests"),
                TaskAuthor::Agent {
                    agent: AgentId("qa".into()),
                    label: "QA".into(),
                },
            )
            .expect("create");
        // `Task` has no `creator` field on purpose, so the append-only log is the only place this
        // fact can live — and it is the place "who did what, when" already lives.
        assert_eq!(task.comments.len(), 1);
        assert!(matches!(task.comments[0].author, TaskAuthor::Agent { .. }));
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
}
