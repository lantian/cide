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
use cide_ipc::{
    AttachTarget, ImageDoc, ProjectId, StagedFile, TaskAttachmentId, TaskAuthor, TaskBoard,
    TaskEdit, TaskId, TaskNew,
};
use cide_tasks::{TaskStore, attachments};
use tauri::{Manager, State};

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
        // Files the New task dialog staged ride on the same request, and land before the one
        // broadcast below — see `TaskNew::attachments`. A refusal here (a file that went away
        // between the pick and the click) takes the task with it: the dialog stays open with
        // the sentence, and a retry must not mint a second `t-<n>` beside a first that has no
        // files. The id is spent either way, which `TaskFile::next_id` says is by design. (M39)
        let task = match req.attachments.as_deref() {
            Some(sources) if !sources.is_empty() => {
                match store.attach(&task.id, AttachTarget::Task, sources, TaskAuthor::User) {
                    Ok(with_files) => with_files,
                    Err(error) => {
                        let _ = store.delete(&task.id);
                        return Err(error);
                    }
                }
            }
            _ => task,
        };
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
    // From the panel, so no session to name: the primary pane hears about any run this starts.
    task_triggers::consider(&app, project, vec![mutation], cide_ipc::RunNotify::Primary);
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
    // From the panel, so no session to name: the primary pane hears about any run this starts.
    task_triggers::consider(&app, project, vec![mutation], cide_ipc::RunNotify::Primary);
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

/*
 * Attachments. (M39)
 *
 * Seven commands for one feature, and the count is the shape of the feature rather than a
 * failure to consolidate. Three of them *add* — from paths, from the clipboard, and the
 * clipboard staged for a comment or task that does not exist yet — and they are three because
 * the frontend cannot hold bytes: `crates/cide-ipc/src/image.rs`'s rule, applied in the direction
 * `fs_paste_image` first needed it. One *picks*, through the parented GTK dialog and never the
 * plugin's JS `open()`, for `project_pick`'s reason. One *grants* — `image_read`'s scope
 * widening, jailed to the record — and two *open*, from Rust, because a detached window's
 * capability set has no `opener:*` and the card is drawn in every window. Detaching is not here:
 * it is a `TaskEdit` and goes through `task_edit`, so there is one road for it.
 *
 * Every mutation among them answers with the whole board, per the module header, and every
 * one is `TaskAuthor::User` for the same reason the four above are.
 */

/// Attach files by path — picked, dropped, or staged — to the body, a comment, or a new comment.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_attach(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    project: ProjectId,
    task: TaskId,
    target: AttachTarget,
    sources: Vec<PathBuf>,
) -> Result<TaskBoard> {
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    let (store, mutation) = blocking(move || {
        let store = tracker(&stores, project, root);
        // A comment born with its files is prose a mention trigger may scan, exactly as a
        // plain `TaskEdit::Comment` is in `task_edit`.
        let fresh_text: Vec<String> = match &target {
            AttachTarget::NewComment { text } => vec![text.clone()],
            _ => Vec::new(),
        };
        let before = store.get(&task);
        let after = store.attach(&task, target, &sources, TaskAuthor::User)?;
        let mutation = TaskMutation {
            before,
            after,
            author: TaskAuthor::User,
            assign_gesture: false,
            fresh_text,
        };
        Ok((store, mutation))
    })
    .await?;
    let board = answer(&app, project, &store);
    // From the panel, so no session to name: the primary pane hears about any run this starts.
    task_triggers::consider(&app, project, vec![mutation], cide_ipc::RunNotify::Primary);
    Ok(board)
}

/// What the clipboard holds as a PNG, or `None` when it holds no image.
///
/// `fs_paste_image`'s round trip, and its reasoning: the clipboard is a bitmap, the read and the
/// encode belong on the blocking pool, and every failure of the read is one answer — an empty
/// clipboard, a text-only one, an owner that went away — because the caller's question is the
/// same in all three. `None` rather than an error variant, because this is reached by Ctrl+V
/// whenever the card has focus, and an ordinary paste of text must cost the user nothing to see.
fn clipboard_png(app: &tauri::AppHandle) -> Result<Option<(String, Vec<u8>)>> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let Ok(image) = app.clipboard().read_image() else {
        return Ok(None);
    };
    let bytes = cide_core::image::encode_png(image.rgba(), image.width(), image.height())?;
    Ok(Some((cide_core::image::pasted_image_name_now(), bytes)))
}

/// Attach the image on the clipboard, or answer `None` when there is none.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_attach_clipboard(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    project: ProjectId,
    task: TaskId,
    target: AttachTarget,
) -> Result<Option<TaskBoard>> {
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    let handle = app.clone();
    let Some((store, mutation)) = blocking(move || {
        let Some((name, bytes)) = clipboard_png(&handle)? else {
            return Ok(None);
        };
        let store = tracker(&stores, project, root);
        let fresh_text: Vec<String> = match &target {
            AttachTarget::NewComment { text } => vec![text.clone()],
            _ => Vec::new(),
        };
        let before = store.get(&task);
        let after = store.attach_bytes(&task, target, &name, &bytes, TaskAuthor::User)?;
        Ok(Some((
            store,
            TaskMutation {
                before,
                after,
                author: TaskAuthor::User,
                assign_gesture: false,
                fresh_text,
            },
        )))
    })
    .await?
    else {
        return Ok(None);
    };
    let board = answer(&app, project, &store);
    // From the panel, so no session to name: the primary pane hears about any run this starts.
    task_triggers::consider(&app, project, vec![mutation], cide_ipc::RunNotify::Primary);
    Ok(Some(board))
}

/// Write the clipboard's image somewhere it can wait for the comment or task it will belong to.
///
/// The composer and the New task dialog paste before there is anything to attach to, so the
/// bytes go under `cide_tasks::attachments::staging_dir()` and the path comes back as a
/// [`StagedFile`], to ride on the eventual `task_attach`/`task_new` like a picked path would.
/// `import` consumes it from there. `None` when the clipboard holds no image, as above.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_attachment_stage_clipboard(app: tauri::AppHandle) -> Result<Option<StagedFile>> {
    blocking(move || {
        let Some((name, bytes)) = clipboard_png(&app)? else {
            return Ok(None);
        };
        // A directory per staged file, named by a uuid, so two pastes of identically-named
        // screenshots in one second cannot collide, and so `import` can remove the whole slot.
        let slot = attachments::staging_dir().join(uuid::Uuid::new_v4().to_string());
        let path = slot.join(&name);
        // The user's own unfinished gesture: private mode, unlike the committed copy.
        cide_core::persist::write_atomic_with_mode(
            &path,
            &bytes,
            cide_core::persist::PRIVATE_MODE,
        )?;
        Ok(Some(StagedFile {
            bytes: bytes.len() as u64,
            path,
            name,
        }))
    })
    .await
}

/// Let the user pick files to attach. Empty means cancelled, which is not an error.
///
/// `show_picker` and never the dialog plugin's JS `open()`: `cmd::project` records why — the
/// plugin's dialog is not parented on Linux and opens *behind* the window on KDE Wayland.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_pick_attachments(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Vec<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    crate::cmd::project::show_picker(
        &app,
        window,
        crate::cmd::project::PickerSpec {
            title: "Attach files",
            accept: "Attach",
            folders: false,
            multiple: true,
            // Any file: the feature's whole premise. A filter here would be a list of what
            // cide thinks an attachment is, and the user knows better.
            filter: None,
        },
        tx,
    )?;
    // Unbounded — the user is browsing — so off the runtime's worker pool: `project_pick`'s
    // reasoning.
    tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or_default())
        .await
        .map_err(|e| CoreError::Io(format!("attachment picker: {e}")))
}

/// Where one attachment's bytes are, from the record — never from anything the caller typed.
///
/// The jail is the construction: the path is `root/.cide/attachments/<task>/<id>/<name>` from a
/// record the store holds, so there is nothing for `openable`'s containment ladder to check.
/// A tombstoned record answers "no such attachment", like a deleted comment.
fn attachment_path(
    store: &TaskStore,
    task: &TaskId,
    attachment: &TaskAttachmentId,
) -> Result<PathBuf> {
    let held = store
        .get(task)
        .ok_or_else(|| CoreError::NoSuchTask(task.clone()))?;
    let record = cide_tasks::attachment_record(&held, attachment)
        .filter(|record| !record.deleted)
        .ok_or_else(|| CoreError::Io(format!("no such attachment: {attachment}")))?;
    Ok(attachments::path_of(store.root(), task, record))
}

/// Vouch for an attachment as an image and let this webview load it.
///
/// `image_read`, jailed: the path comes from the record, the bytes are re-sniffed by
/// `cide_core::image::read` every time — the stored `AttachmentKind` is a hint in a committed,
/// hand-editable file, and the grant is a capability — and the scope is widened *after* every
/// refusal, one file at a time, for the reason `image_read` gives.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_attachment_image(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    project: ProjectId,
    task: TaskId,
    attachment: TaskAttachmentId,
) -> Result<ImageDoc> {
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    let doc = blocking(move || {
        let store = tracker(&stores, project, root);
        let path = attachment_path(&store, &task, &attachment)?;
        cide_core::image::read(&path)
    })
    .await?;
    app.asset_protocol_scope()
        .allow_file(&doc.path)
        .map_err(|e| {
            CoreError::Io(format!(
                "{} could not be served to the viewer: {e}",
                doc.path.display()
            ))
        })?;
    Ok(doc)
}

/// Open an attachment with whatever the desktop associates with its name.
///
/// From Rust, like every opener call in this crate: the JS command is capability-gated per
/// window and the detached-window set grants no `opener:*`, while the card is drawn in every
/// window. The first caller in the workspace to open a *file* rather than a directory — the
/// real name on disk, with its real extension, is what makes that meaningful.
#[tauri::command(rename_all = "camelCase")]
pub async fn task_attachment_open(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    project: ProjectId,
    task: TaskId,
    attachment: TaskAttachmentId,
) -> Result<()> {
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    blocking(move || {
        use tauri_plugin_opener::OpenerExt;
        let store = tracker(&stores, project, root);
        let path = attachment_path(&store, &task, &attachment)?;
        app.opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|e| CoreError::Io(format!("{}: {e}", path.display())))
    })
    .await
}

/// Reveal an attachment's directory in the file manager. The containing directory, for
/// `fs_show_in_manager`'s reason: there is no portable "select this file".
#[tauri::command(rename_all = "camelCase")]
pub async fn task_attachment_reveal(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<TasksStores>>,
    project: ProjectId,
    task: TaskId,
    attachment: TaskAttachmentId,
) -> Result<()> {
    let root = tasks_state::project_root(&state, project)?;
    let stores = Arc::clone(&stores);
    blocking(move || {
        use tauri_plugin_opener::OpenerExt;
        let store = tracker(&stores, project, root);
        let path = attachment_path(&store, &task, &attachment)?;
        let dir = path
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| CoreError::Io(format!("{} has no directory", path.display())))?;
        app.opener()
            .open_path(dir.to_string_lossy(), None::<&str>)
            .map_err(|e| CoreError::Io(format!("{}: {e}", dir.display())))
    })
    .await
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
