//! The task tracker's commands. (M18)
//!
//! Thin by policy like every other module here: resolve the project's first root, reach the one
//! [`TaskStore`](cide_tasks::TaskStore) that owns its `.cide/tasks.json`, call one method on it,
//! hand back the board. Nothing in this file knows what a merge is, what a `rev` is for, or
//! which file mode a committed tracker wants.
//!
//! Three shapes are worth naming, because each one is a decision rather than a habit.
//!
//! **Every mutation answers with the whole board.** `git`'s stated argument, one panel over:
//! "a call that returned `{ok: true}` would be followed at once by a second asking what
//! happened, and the frame in between shows a tree that is visibly wrong." It is if anything
//! stronger here — a status change moves a row between groups, so the row the user clicked is
//! not where it was, and a panel that re-asked would draw it in the old group for one frame.
//!
//! **Every mutation also emits `cide://tasks-changed`.** That is for the *other* windows, and
//! for nothing else: the caller already has its answer in the return value. A window that both
//! called and listened would apply the same board twice, which is harmless only because the
//! receiver drops anything not newer than what it holds.
//!
//! **The author is always [`TaskAuthor::User`].** These four commands are reachable from exactly
//! one place — the Tasks panel, in a window the user is looking at — so the identity is not in
//! question. [`TaskAuthor::Agent`] arrives on a different road entirely, over the agent-RPC
//! socket, where the run id comes out of the child's own environment rather than out of a
//! payload: identity must never be something the caller composes, or an agent can sign a comment
//! as the user. Nothing here takes an author argument, and that is what enforces it.
//!
//! **Nothing here runs on the main thread.** Tauri 2 polls a synchronous command on the GTK
//! loop, and the first ask for a project opens and parses a file. Every handler is `async` and
//! hands its store work to [`blocking`], for `cmd::git`'s reasons — see there.

use std::path::PathBuf;
use std::sync::Arc;

use cide_core::CoreError;
use cide_ipc::{ProjectId, TaskAuthor, TaskBoard, TaskEdit, TaskId, TaskNew};
use cide_tasks::TaskStore;
use tauri::State;

use crate::task_triggers::{self, TaskMutation};
use crate::tasks_state::{self, TasksStores};
use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, CoreError>;

/// Run store work on the blocking pool.
///
/// `#[tauri::command(async)]` is not the fix and neither is an `async fn` whose body never
/// awaits: both only move the call onto the async runtime, where a blocking file read still
/// occupies a runtime worker for its whole duration. `spawn_blocking` is the pool built for it,
/// and it is the same helper `cmd::git` carries for the same reason.
///
/// The workspace lock is *not* taken in here. Callers resolve their root first, on the caller's
/// thread, and move a plain `PathBuf` in; a `State` guard held across the await would be both
/// un-`Send` and a lock held for the length of a file write.
async fn blocking<T>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        // A panic in the worker is a bug, not a tracker condition — but it has to reach the
        // panel as *something*, or the command hangs on a channel that never answers.
        .map_err(|error| CoreError::Io(format!("the task worker did not finish: {error}")))?
}

/// Everything the four handlers do before they differ: resolve the root, then the store.
///
/// `ensure` rather than `get`, deliberately. The restore loop in `lib.rs` warms a store for every
/// project it brings back, but that list is capped at 32 (see `ide::servable_projects`) and a
/// project past the cap would otherwise have a Tasks panel that answered "no project" for ever,
/// with nothing anywhere saying why. `ensure` is idempotent and costs a `DashMap` hit for every
/// call but the first, so making the commands independent of whether the warm-up reached this
/// project removes the entire ordering question rather than documenting it.
fn tracker(stores: &TasksStores, project: ProjectId, root: PathBuf) -> Arc<TaskStore> {
    stores.ensure(project, &root)
}

/// What a project's tracker says right now.
///
/// Three shapes, not an array: `TaskBoard` distinguishes "there is no tracker in this project"
/// from "there is one and it is empty" from "it will not parse", because those are three
/// different sentences on screen with three different sets of buttons under them.
#[tauri::command(rename_all = "camelCase")]
pub async fn tasks_board(
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    project: ProjectId,
) -> Result<TaskBoard> {
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    blocking(move || Ok(tracker(&stores, project, root).board())).await
}

/// Create a task, and answer with the board it landed in.
///
/// `req.project` is what names the store; [`TaskStore::create`] ignores the field for exactly
/// that reason — the store already knows which project it is, from the path it was opened with,
/// and carrying the id further would be a second source of truth for one fact.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_new(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    req: TaskNew,
) -> Result<TaskBoard> {
    let project = req.project;
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    let (store, mutation) = blocking(move || {
        let store = tracker(&stores, project, root);
        // Captured before `req` is consumed: the creation body is the fresh text a mention
        // trigger may scan — see `TaskMutation::fresh_text`.
        let fresh_text: Vec<String> = req.body.clone().into_iter().collect();
        // A creation that names an assignee is an assignment gesture — the compose form's
        // dropdown was just used — so the policy dispatches it as one.
        let assign_gesture = req.agent.is_some();
        // `TaskAuthor::User`: see the module header for why no caller may name an author.
        let task = store.create(&req, TaskAuthor::User)?;
        let mutation = TaskMutation {
            before: None,
            after: task,
            author: TaskAuthor::User,
            assign_gesture,
            fresh_text,
        };
        Ok((store, mutation))
    })
    .await?;
    let board = answer(&app, project, &store);
    // After `answer`: the caller's board and the other windows' broadcast never wait on the
    // trigger, which does its own disk read on the blocking pool.
    task_triggers::consider(&app, project, vec![mutation]);
    Ok(board)
}

/// Apply one change to one task.
///
/// A task id naming nothing is [`CoreError::NoSuchTask`] from the store, which is an ordinary
/// race rather than a failure: the user deleted the row in the other window a moment ago, and
/// the panel answers by taking the board it is handed with the error.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_edit(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    project: ProjectId,
    task: TaskId,
    edit: TaskEdit,
) -> Result<TaskBoard> {
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    let (store, mutation) = blocking(move || {
        let store = tracker(&stores, project, root);
        // The prose this mutation introduces, read before `edit` is consumed. Everything else —
        // a status flip, a title, a comment edit — carries none, so an old mention in the stored
        // body cannot re-fire.
        let fresh_text: Vec<String> = match &edit {
            TaskEdit::SetBody { body } => vec![body.clone()],
            TaskEdit::Comment { text } => vec![text.clone()],
            _ => Vec::new(),
        };
        // The dropdown's own edit shape, and only it: `Assign { Some }` is the user picking a
        // role — the revive gesture the policy dispatches even when the role is unchanged.
        // `Assign { None }` is an unassign and stays inert.
        let assign_gesture = matches!(&edit, TaskEdit::Assign { agent: Some(_) });
        // `before` under a second lock take, immediately ahead of the edit. A writer landing in
        // the gap costs at worst one spurious or missed trigger (deduped against live runs
        // anyway) and corrupts nothing; holding one lock across both would mean a closure API
        // the store deliberately does not offer for edits.
        let before = store.get(&task);
        let after = store.edit(&task, edit, TaskAuthor::User)?;
        let mutation = TaskMutation {
            before,
            after,
            author: TaskAuthor::User,
            assign_gesture,
            fresh_text,
        };
        Ok((store, mutation))
    })
    .await?;
    let board = answer(&app, project, &store);
    task_triggers::consider(&app, project, vec![mutation]);
    Ok(board)
}

/// Remove a task.
///
/// Reachable from here and from nowhere else — no MCP tool deletes: the file is the shared
/// record of what happened, and an agent that could quietly drop a row could quietly drop the
/// evidence of its own turn.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_delete(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    project: ProjectId,
    task: TaskId,
) -> Result<TaskBoard> {
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    let store = blocking(move || {
        let store = tracker(&stores, project, root);
        store.delete(&task)?;
        Ok(store)
    })
    .await?;
    Ok(answer(&app, project, &store))
}

/// Broadcast the new board and return it — the two halves of every mutation's answer.
///
/// One board is computed and used for both, rather than reading the store twice: a mutation
/// landing between the two reads would otherwise let the caller and the other windows disagree
/// about what this call did, which is the one thing carrying the whole board is meant to prevent.
fn answer(app: &tauri::AppHandle, project: ProjectId, store: &TaskStore) -> TaskBoard {
    let (rev, board) = tasks_state::versioned_board(store);
    crate::emit::tasks_changed(app, project, rev, &board);
    board
}
