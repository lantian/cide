//! File tree and file operation commands.
//!
//! Thin, like every other handler here: unwrap arguments, check the path belongs to the
//! project, call `cide-fs`, wrap the result. The one rule enforced at this layer rather than
//! below it is containment — every path argument is checked against the project's roots
//! before anything touches the disk, because this is where a path stops being a value the
//! backend produced and becomes one the webview sent back.
//!
//! # Why every handler here is `async`
//!
//! Nothing in this file is asynchronous work; all of it is *blocking* work — a walk, a lock,
//! a `read(2)`. A synchronous `#[tauri::command]` runs on the main thread, so a walk of a
//! large repository froze the window it was opened from, and even the cheap handlers queued
//! behind it. Declaring them `async` moves the body to Tauri's async runtime, which is not
//! enough on its own: that runtime is a small pool shared with the IDE server, and a walk
//! parked on one of its workers starves everything else the pool owes an answer to. So each
//! handler resolves its `State` arguments — map lookups, microseconds — and hands the rest to
//! [`blocking`], which is the pool that exists for exactly this.
//!
//! The bodies of the three handlers the picker's streaming property runs through are named
//! functions ([`index_project`], [`status_of`], and `picker::query_project`) rather than
//! bodies inline in the `#[tauri::command]` items. Tauri's argument extraction is the only
//! thing that separates the two, and it is what makes the test at the foot of this file able
//! to drive the real command layer.

use std::path::PathBuf;
use std::sync::Arc;

use cide_fs::{FsError, ops};
use cide_ipc::{FsStatus, ProjectId, TreeRow};
use tauri::{Manager, State};

use crate::cmd::search::SearchRegistry;
use crate::files::{FsEvents, FsRegistry};
use crate::workspace_state::WorkspaceState;

/// The most rows one request will return.
///
/// The tree draws about thirty. A frontend bug asking for a million must cost a clamped
/// answer, not a serialised megabyte.
const MAX_ROWS: usize = 2048;

fn project_fs(
    registry: &FsRegistry,
    project: ProjectId,
) -> Result<std::sync::Arc<crate::files::ProjectFs>, FsError> {
    registry.get(project).ok_or(FsError::NoIndex)
}

/// Run a handler's blocking half on the blocking pool.
///
/// The join can only fail if the job panicked or the runtime is shutting down. Reported as
/// an `Io` error naming the command rather than re-panicked: re-panicking would take out a
/// runtime worker over one bad path, and the frontend would see a command that never
/// resolves — which is indistinguishable from a hung app. Same shape as `cmd::file::blocking`
/// and for the same reasons.
async fn blocking<T: Send + 'static>(
    what: &'static str,
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, FsError> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| FsError::Io {
            path: what.to_string(),
            message: format!("the worker running this command failed: {error}"),
        })
}

/// Walk a project's roots and start watching them.
///
/// Called by the frontend after `project.open` rather than from `project_open` itself: the
/// walk is seconds of work on a large repository, and a project that opens instantly with an
/// empty tree that fills in is better than one that hangs before it appears. The picker is
/// answerable from the moment this starts.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_index(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
) -> Result<FsStatus, FsError> {
    let roots: Vec<PathBuf> = state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .map(|p| p.roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    index_project(Arc::new(app), &registry, project, roots).await
}

/// [`fs_index`] with its two Tauri-injected arguments already resolved to values.
///
/// Returns the status a *second* caller would get — `indexing: true` and a live picker —
/// when a walk for this project is already running, rather than starting a second walk of
/// the same tree. Two windows both reacting to `project.open` is the ordinary case, not an
/// error one.
pub(crate) async fn index_project(
    events: Arc<dyn FsEvents>,
    registry: &FsRegistry,
    project: ProjectId,
    roots: Vec<PathBuf>,
) -> Result<FsStatus, FsError> {
    if roots.is_empty() {
        return Err(FsError::NoIndex);
    }
    match registry.claim(project, roots) {
        Ok(walk) => blocking("fs_index", move || walk.run(events, project)).await,
        Err(already_running) => Ok(already_running),
    }
}

/// Drop a project's index, stop watching it, and stop any search over it. Idempotent.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_close(
    registry: State<'_, FsRegistry>,
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
) -> Result<bool, FsError> {
    close_project(&registry, &searches, project).await
}

/// [`fs_close`] with its Tauri-injected arguments already resolved. See the module note.
pub(crate) async fn close_project(
    registry: &FsRegistry,
    searches: &SearchRegistry,
    project: ProjectId,
) -> Result<bool, FsError> {
    // Taken out of the registry here — a map operation — and *dropped* on a worker. The drop
    // is what stops the watcher thread and frees a matcher that may hold 100 000 candidates,
    // and doing that on the async runtime is the same mistake as walking there.
    let taken = registry.remove(project);
    let existed = taken.is_some();
    // A content search is a walker thread per core reading a tree nobody has open any more,
    // holding its hits until the flag is set. It used to be cleaned up by the *next* query
    // against any project, which is a moment that may never come. Cancelling costs one store
    // and it happens before the drop, so the walkers see the flag while the index is still
    // being torn down rather than after.
    searches.cancel(project);
    blocking("fs_close", move || drop(taken)).await?;
    Ok(existed)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn fs_status(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
) -> Result<FsStatus, FsError> {
    status_of(&registry, project).await
}

/// [`fs_status`] without Tauri's argument extraction. See the module note.
pub(crate) async fn status_of(
    registry: &FsRegistry,
    project: ProjectId,
) -> Result<FsStatus, FsError> {
    let fs = project_fs(registry, project)?;
    blocking("fs_status", move || fs.status()).await
}

/// How many rows the tree currently has. The virtual scroller's range.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_tree_count(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
) -> Result<u32, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_tree_count", move || {
        fs.with_index(|index| index.count() as u32)
    })
    .await
}

/// The rows in `[offset, offset + len)`.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_tree_rows(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    offset: u32,
    len: u32,
) -> Result<Vec<TreeRow>, FsError> {
    let fs = project_fs(&registry, project)?;
    let len = (len as usize).min(MAX_ROWS);
    blocking("fs_tree_rows", move || {
        fs.with_index(|index| index.rows(offset as usize, len))
    })
    .await
}

/// Expand a directory. Returns the new row count.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_expand(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<u32, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_expand", move || {
        fs.with_index_mut(|index| index.expand(&path))
            .map(|n| n as u32)
            .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))
    })
    .await?
}

/// Collapse a directory. Returns the new row count.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_collapse(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<u32, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_collapse", move || {
        fs.with_index_mut(|index| index.collapse(&path))
            .map(|n| n as u32)
            .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))
    })
    .await?
}

/// Expand everything above a path and return the row it sits on.
///
/// `None` rather than an error when the path is not in the tree: revealing a file that is
/// gitignored, or that has just been deleted, is an ordinary thing for the editor to ask and
/// the honest answer is "there is no row for that".
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_reveal(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<Option<u32>, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_reveal", move || {
        fs.with_index_mut(|index| index.reveal(&path).map(|n| n as u32))
    })
    .await
}

/// The most paths one existence probe will answer.
///
/// A hovered line in a terminal yields a handful of candidates and each candidate a handful of
/// bases, so a real request is single digits. This is the clamp for a frontend bug, and it
/// answers `None` past the cap rather than erroring: the caller is a mouse hover, and the
/// honest degradation there is "no link", not a toast.
const MAX_PROBE: usize = 128;

/// What the index holds at each of these paths — the oracle behind terminal file links.
///
/// # Why this is a batch, and why it touches no disk
///
/// It is asked from `provideLinks` while the pointer crosses a line of output, so the two costs
/// that matter are round trips and syscalls. One call per hovered *line* answers every
/// candidate on it, and every answer comes from [`cide_fs::Index::kind_of`] — a hash lookup
/// against the tree that has already been walked. A `stat` per candidate would be correct and
/// would put filesystem latency on a mouse move.
///
/// `with_index` and not `with_index_mut`, which rules out [`fs_reveal`] as the probe it
/// otherwise resembles: that one *expands directories* as a side effect, so using it to ask
/// whether a path exists would silently unfold the user's file tree under their pointer.
///
/// # What it deliberately cannot see
///
/// The index is gitignore-filtered and does not descend through symlinked directories, so
/// `target/debug/…` and `node_modules/…` answer `None` and never light up. That is the policy
/// and not a gap: index membership is what makes an *offered* link contained by construction.
/// The click is re-checked in Rust regardless — see `cmd::file::terminal_open_path` — because
/// this answer travelled through the webview and comes back as a claim, not as evidence.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_paths_exist(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    paths: Vec<PathBuf>,
) -> Result<Vec<Option<cide_ipc::TreeRowKind>>, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_paths_exist", move || {
        fs.with_index(|index| {
            paths
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    if i >= MAX_PROBE {
                        return None;
                    }
                    index.kind_of(path)
                })
                .collect()
        })
    })
    .await
}

/// Show a path in the desktop file manager. Returns the directory that was opened.
///
/// The file tree's context menu offers *Reveal*, and without this that item could only ever
/// have been a disabled line with an explanation attached to it. Rust does the opening,
/// exactly as `app_open_log_dir` does and for the same reason: `tauri-plugin-opener`'s JS
/// command is capability-gated per window and a detached-pane window deliberately has none —
/// going through it would make the item work in the shell window and silently do nothing in a
/// torn-out one.
///
/// The **containing directory** is what gets opened, not the file. There is no portable way to
/// ask a Linux file manager to select one entry — `nautilus --select` and `dolphin --select`
/// are different flags on different binaries, and neither is necessarily the file manager the
/// user's desktop is running — so this does the thing that behaves the same everywhere rather
/// than the nicer thing that works on one developer's machine. A directory row opens itself.
///
/// Containment is checked first, like every other handler here: the path came back from the
/// webview, and "open this in the user's file manager" is not something to do to `/etc` on a
/// frontend bug.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_show_in_manager(
    app: tauri::AppHandle,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<PathBuf, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_show_in_manager", move || {
        use tauri_plugin_opener::OpenerExt;

        ops::check_within(&fs.root_paths(), &path)?;
        // `is_dir` follows symlinks, which is what the user means here: revealing a symlinked
        // directory should open what it points at, not the directory the link sits in.
        let target = if path.is_dir() {
            path.clone()
        } else {
            path.parent()
                .map(PathBuf::from)
                .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))?
        };
        app.opener()
            .open_path(target.to_string_lossy(), None::<&str>)
            .map_err(|e| FsError::Io {
                path: target.display().to_string(),
                message: e.to_string(),
            })?;
        Ok(target)
    })
    .await?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn fs_read_file(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<String, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_read_file", move || {
        ops::check_within(&fs.root_paths(), &path)?;
        ops::read_to_string(&path)
    })
    .await?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn fs_write_file(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
    contents: String,
) -> Result<(), FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_write_file", move || {
        ops::check_within(&fs.root_paths(), &path)?;
        ops::write(&path, &contents)
    })
    .await?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn fs_create(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
    directory: bool,
) -> Result<(), FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_create", move || {
        ops::check_within(&fs.root_paths(), &path)?;
        ops::create(&path, directory)
    })
    .await?
}

/// Create a file or a folder *inside a named directory*, and put it in the tree immediately.
///
/// Not `fs_create` with a joined path, for three reasons that are all about the gesture this
/// serves — the file tree's *New File…* / *New Folder…*:
///
/// * **The parent has to already exist.** `fs_create` reaches `ops::create`, which calls
///   `create_dir_all`; a *New File* in a folder that was deleted since the tree drew it would
///   put the folder back. See [`ops::create_in`].
/// * **The name is checked as a name**, not as a path. `src/main.rs` typed into the box is a
///   thing the user plainly meant and would not get, so it is refused with a reason rather
///   than obeyed as two path components or mangled into `src_main.rs`.
/// * **The tree shows the row now.** The watcher will report this create in a few hundred
///   milliseconds, and "a few hundred milliseconds" is exactly long enough for the user to
///   decide the menu item did nothing. Folding the path into the index here is the same
///   `Index::apply` the watcher would run, so the watcher's own event a moment later
///   reconciles to no change — `rescan_dir` matches by name and reports nothing new, which is
///   also why the picker cannot end up with the file twice.
///
/// Answers with the path it created, so the caller can select that row without re-deriving
/// the same string a second time in a second language.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_create_in(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    parent: PathBuf,
    name: String,
    directory: bool,
) -> Result<PathBuf, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_create_in", move || {
        create_entry(&fs, &parent, &name, directory)
    })
    .await?
}

/// [`fs_create_in`] with its Tauri-injected argument already resolved to a value.
///
/// A named function for the same reason [`index_project`] is one: the property worth testing
/// — that the row is in the tree the instant the command returns, with no watcher involved —
/// is a property of *this* wiring, and a body inline in the `#[tauri::command]` item cannot be
/// called from a test.
pub(crate) fn create_entry(
    fs: &crate::files::ProjectFs,
    parent: &std::path::Path,
    name: &str,
    directory: bool,
) -> Result<PathBuf, FsError> {
    let created = ops::create_in(&fs.root_paths(), parent, name, directory)?;

    // The same fold the watcher does, run here so the tree does not have to wait for it.
    // `admits` is consulted by `Index::apply` itself, so a name the project's ignore rules
    // hide simply adds no row — the file is still created, and the frontend notices the
    // missing row and says so rather than pretending the tree is showing it.
    let filter = fs.filter();
    let change = cide_ipc::FsChange {
        paths: vec![created.clone()],
        truncated: false,
        git: false,
    };
    let added = fs.with_index_mut(|index| index.apply(&change, &filter));
    for item in added.iter().filter(|i| !i.is_dir) {
        fs.matcher().push(cide_search::Candidate::new(
            item.rel.clone(),
            item.path.to_string_lossy().into_owned(),
        ));
    }
    Ok(created)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn fs_rename(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    from: PathBuf,
    to: PathBuf,
) -> Result<(), FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_rename", move || {
        let roots = fs.root_paths();
        ops::check_within(&roots, &from)?;
        ops::check_within(&roots, &to)?;
        ops::check_not_root(&roots, &from)?;
        ops::rename(&from, &to)
    })
    .await?
}

/// Move paths to the desktop trash. Returns where each one landed.
///
/// Trash rather than `unlink`, and the reason is worth stating where it can be read: a file
/// tree's delete is one keystroke away from a misclick, and the freedesktop trash is the only
/// undo a file manager offers. See `cide_fs::trash`.
///
/// Every path is checked before any path is moved, so a selection with one bad entry in it
/// trashes nothing rather than half of itself. Once the moves start, a failure part way
/// through cannot be undone — the paths already trashed are reported in the error so the
/// caller knows what actually moved rather than assuming nothing did.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_delete(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    paths: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_delete", move || {
        let roots = fs.root_paths();
        for path in &paths {
            ops::check_within(&roots, path)?;
            ops::check_not_root(&roots, path)?;
        }
        let mut trashed = Vec::with_capacity(paths.len());
        for path in &paths {
            match ops::delete(path) {
                Ok(dest) => trashed.push(dest),
                Err(err) => {
                    tracing::warn!(
                        failed = %path.display(),
                        already_trashed = trashed.len(),
                        "a multi-path delete stopped part way through"
                    );
                    return Err(FsError::PartialDelete {
                        trashed: trashed.iter().map(|p| p.display().to_string()).collect(),
                        error: err.to_string(),
                    });
                }
            }
        }
        Ok(trashed)
    })
    .await?
}

/// Which names a paste would land on that are already taken — and nothing is written.
///
/// The read half of Ctrl+V, asked first so the confirmation can be a decision rather than an
/// apology. An empty answer is the common case and means the paste can go straight through;
/// anything in it is a question for the user, and [`fs_paste`] is called afterwards carrying
/// the answers. Cancelling costs nothing because nothing has happened yet — which is the
/// whole reason this is a separate command instead of a flag on the paste.
///
/// I/O (it stats the destination and walks a folder pair to count what a merge would cost), so
/// `spawn_blocking` like every other handler here. The refusals arrive from this call as well
/// as from the paste: a folder pasted into itself is refused *instead of* being asked about.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_paste_plan(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    sources: Vec<PathBuf>,
    dest_dir: PathBuf,
    mode: cide_ipc::PasteMode,
) -> Result<Vec<cide_ipc::PasteCollision>, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_paste_plan", move || {
        let roots = fs.root_paths();
        cide_fs::copy::plan(&roots, &sources, &dest_dir, mode)
    })
    .await?
}

/// Copy or move paths into a folder — the file tree's Ctrl+C / Ctrl+X / Ctrl+V.
///
/// The rules are all in [`cide_fs::copy`], because they are decisions about the disk rather
/// than about Tauri: a collision is renamed unless a `decisions` entry says otherwise, a cut
/// moves nothing until it is pasted, a directory is copied recursively and refused into itself,
/// and a symlink is copied as a link. Read that module before changing anything here.
///
/// `decisions` is the dialog's answers, and it is the *only* way anything here can overwrite a
/// file. A source that is not named in it is renamed on collision, exactly as before — so this
/// command is safe to call with an empty list, and a frontend that loses its answers renames
/// rather than destroys. The confirmation is a courtesy; this refusal is the guarantee.
///
/// What is this layer's own is the same two things every handler in this file owns: the paths
/// are checked against the project's roots before anything touches the disk (twice over —
/// `copy::paste` checks them again, because it is also called from tests with roots of their
/// own), and the result is folded into the index so the tree shows the new rows **now**
/// instead of a few hundred milliseconds later when the watcher's debounce expires. A paste
/// that appears to do nothing for half a second is a paste the user presses again.
///
/// The fold names the *sources* as well as the destinations. `Index::apply` rescans the parent
/// directory of every path it is given, so naming a cut's source is what makes the row it left
/// behind disappear in the same repaint that draws the row it arrived at — otherwise a move
/// shows the file in two places until the watcher catches up.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_paste(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    sources: Vec<PathBuf>,
    dest_dir: PathBuf,
    mode: cide_ipc::PasteMode,
    decisions: Vec<cide_ipc::PasteDecision>,
) -> Result<Vec<cide_ipc::PastedEntry>, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_paste", move || {
        paste_into(&fs, &sources, &dest_dir, mode, &decisions)
    })
    .await?
}

/// [`fs_paste`] with its Tauri-injected argument already resolved to a value.
///
/// A named function for the same reason [`create_entry`] is one: the property worth testing is
/// that the pasted rows are in the tree the instant the command returns, and a body inline in
/// the `#[tauri::command]` item cannot be called from a test.
pub(crate) fn paste_into(
    fs: &crate::files::ProjectFs,
    sources: &[PathBuf],
    dest_dir: &std::path::Path,
    mode: cide_ipc::PasteMode,
    decisions: &[cide_ipc::PasteDecision],
) -> Result<Vec<cide_ipc::PastedEntry>, FsError> {
    let roots = fs.root_paths();
    let pasted = cide_fs::copy::paste_with(&roots, sources, dest_dir, mode, decisions)?;

    // Sources first so a rename that lands on the *same* directory reads as one rescan, and
    // because a cut's old row has to go in the same write that adds the new one.
    let mut touched: Vec<PathBuf> = sources.to_vec();
    touched.extend(pasted.iter().map(|entry| entry.dest.clone()));
    let change = cide_ipc::FsChange {
        paths: touched,
        truncated: false,
        git: false,
    };
    let filter = fs.filter();
    let added = fs.with_index_mut(|index| index.apply(&change, &filter));
    for item in added.iter().filter(|i| !i.is_dir) {
        fs.matcher().push(cide_search::Candidate::new(
            item.rel.clone(),
            item.path.to_string_lossy().into_owned(),
        ));
    }
    Ok(pasted)
}

/// Stop watching everything, and stop every search.
///
/// **No caller today.** It reads like the quit path and is not: `lifecycle::shutdown` is,
/// and it cancels the searches itself rather than coming through here, because dropping every
/// project's index on the main thread is the teardown `fs_close` hands to a blocking worker
/// for being too slow. Kept for the whole-registry teardown a second window mode would want;
/// if that never arrives, this should go rather than keep implying something calls it.
pub fn close_all(app: &tauri::AppHandle) {
    // Searches first: a walker thread that outlives the registry entry it is searching is what
    // makes a quit hang, and the flag is what stops it. Same order as `close_project`.
    if let Some(searches) = app.try_state::<SearchRegistry>() {
        searches.cancel_all();
    }
    if let Some(registry) = app.try_state::<FsRegistry>() {
        registry.close_all();
    }
}

#[cfg(test)]
mod tests {
    //! The picker answers *through the command layer* while the walk is still running.
    //!
    //! `cide-fs/tests/large_repo.rs` already proves the walker streams into the matcher, and
    //! `cide-search` proves the matcher answers while it is being filled. Neither says
    //! anything about this file: the seam they exercise is two crates below the handlers, and
    //! every wiring mistake that could break the property for a user lives here — claiming the
    //! project after the walk instead of before it, running the walk inside the lock the
    //! status handler takes, or (the version this replaced) simply awaiting the walk before
    //! answering anything.
    //!
    //! What is asserted is deliberately not "the picker eventually returns matches". It is
    //! that a frame with matches was taken *between two `fs_status` readings that both said
    //! `indexing: true`*, **early**. Two claims, and the second one was not obvious:
    //!
    //! * The brackets. Reading the status only before the query leaves a hole big enough to
    //!   drive the bug through: the walk can finish in between, and the frame then says
    //!   nothing. Two readings, one on each side, put the frame provably inside the walk.
    //! * The clock. The brackets alone are not enough, and this was measured rather than
    //!   reasoned about. A build whose sink collects every entry into a `Vec` and pushes the
    //!   lot into the matcher after `Index::build` returns *still* holds `indexing: true`
    //!   through that final injection, and a poll landing in that window sees matches, a
    //!   small `total`, and `indexing: true` on both sides. Against that mutant the bracket
    //!   assertion alone passed one run in three. What it cannot fake is *when*: its first
    //!   match arrives at 96-100% of the walk, a real one at under 15%.
    //!
    //! Nothing here touches a path outside `cide_fs::testing::scratch`, and the tree goes when
    //! the `Scratch` drops — including while unwinding.

    use super::*;
    use crate::cmd::picker::query_project;
    use cide_fs::testing::{Scratch, scratch};
    use cide_ipc::FsChange;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    /// The fuzzy needle. `q` and `z` appear nowhere else in the corpus, so a query for it
    /// cannot be satisfied by filler — fuzzy matching is subsequence matching, and a needle
    /// the filler can spell out makes `matched > 0` mean nothing.
    const NEEDLE: &str = "zqneedle";

    /// 30 000 files.
    ///
    /// The size is set by the clock assertion rather than by taste. The picker's first answer
    /// costs a roughly fixed few milliseconds — one walk batch plus `nucleo`'s first tick —
    /// while the walk itself grows with the corpus, so the ratio between them is only
    /// meaningful once the walk is comfortably longer than that constant. Measured here:
    /// 12 000 files walk in ~30 ms and the first match lands at ~8 ms (27%), which is too
    /// close to [`EARLY`] to bet a suite on; 30 000 walk in 66-89 ms with the first match
    /// still at ~8 ms (~10%). Writing the corpus is the cost, and at this size it is under
    /// 200 ms for the whole test. The 100 000-file case belongs to `cide-fs` and is
    /// `#[ignore]`d there for the inodes it writes.
    const PACKAGES: usize = 24;
    const PER_PACKAGE: usize = 1250;
    const NEEDLE_EVERY: usize = 25;

    /// A first match is "streamed" if it arrived in the first third of the walk.
    ///
    /// Three rather than ten — `large_repo.rs`'s factor at 100 000 files — because this walk
    /// is a tenth of that one's and the picker's fixed startup cost is a bigger share of it.
    /// The measured separation is wide either way: ~10% for a streaming build, 96-100% for one
    /// that injects after the walk.
    const EARLY: u32 = 3;

    /// Five walks before giving up on *observing* anything.
    ///
    /// The retry is for the machine, not for the code, and the reasoning is `large_repo.rs`'s:
    /// `nucleo` scores on its own thread pool, and on a box whose cores are all busy that pool
    /// can go unscheduled for a whole walk, so every frame reads `(0, 0)`. Five independent
    /// attempts put that at roughly one run in a thousand. A build that does not stream fails
    /// all five every time, because every one of its attempts is late by the same amount.
    const ATTEMPTS: usize = 5;

    /// Events go nowhere. Counted anyway: `fs.index` emitting `cide://fs-status` at both ends
    /// is what tells the explorer to re-read a tree that was empty when it attached, and a
    /// walk that emitted nothing would leave the sidebar blank until the first file changed.
    #[derive(Default)]
    struct Counting {
        statuses: AtomicU32,
        /// Statuses that said `indexing: true` — **exactly one per walk**, which makes this
        /// an exact count of walks started rather than an approximate one.
        ///
        /// `Indexing::run` emits its opening status while the claim's flag is still set, and
        /// every other emission in the file happens after `drop(guard)` has cleared it: the
        /// closing status, and the watcher thread's own first status. Counting *all* statuses
        /// instead would make any assertion about "how many walks ran" race that watcher
        /// thread, which emits whenever it is scheduled.
        walks_started: AtomicU32,
    }

    impl FsEvents for Counting {
        fn status(&self, _project: ProjectId, status: &FsStatus) {
            self.statuses.fetch_add(1, Ordering::Relaxed);
            if status.indexing {
                self.walks_started.fetch_add(1, Ordering::Relaxed);
            }
        }

        fn changed(&self, _project: ProjectId, _change: &FsChange) {}
    }

    /// Ask the picker until it has scored `expected` matches, or give up.
    ///
    /// `nucleo` scores on its own thread pool and `frame` only spends a bounded slice waiting
    /// for it, so the first frame after an injection legitimately reports fewer matches than
    /// are in the matcher. Polling for the settled answer keeps the assertions below about
    /// *what the matcher holds* rather than about how promptly a thread pool was scheduled.
    async fn settled_matches(registry: &FsRegistry, project: ProjectId, expected: u32) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let frame = query_project(registry, project, NEEDLE.to_string(), Some(50))
                .await
                .expect("the picker answers for an indexed project");
            if frame.matched >= expected || Instant::now() >= deadline {
                return frame.matched;
            }
            tokio::task::yield_now().await;
        }
    }

    /// Write `PACKAGES` × `PER_PACKAGE` empty files under a fresh scratch directory.
    ///
    /// Threaded because at this size the `create` syscalls cost several times the walk they
    /// exist for, and a test whose setup dominates its subject is one people delete.
    fn corpus(tag: &str) -> Scratch {
        let dir = scratch(tag);
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(PACKAGES);
        let per_thread = PACKAGES.div_ceil(threads);
        std::thread::scope(|scope| {
            for chunk in 0..threads {
                let base: &Path = dir.path();
                scope.spawn(move || {
                    for p in (chunk * per_thread)..((chunk + 1) * per_thread).min(PACKAGES) {
                        let sub = base.join(format!("pkg{p:05}/src"));
                        std::fs::create_dir_all(&sub).expect("create a package directory");
                        for f in 0..PER_PACKAGE {
                            let n = p * PER_PACKAGE + f;
                            let name = if n.is_multiple_of(NEEDLE_EVERY) {
                                format!("{NEEDLE}{n}.rs")
                            } else {
                                format!("filler{n}.rs")
                            };
                            std::fs::write(sub.join(name), []).expect("write a corpus file");
                        }
                    }
                });
            }
        });
        dir
    }

    /// One frame taken while `fs_status` said `indexing: true` on both sides of it.
    #[derive(Debug, Clone, Copy)]
    struct MidWalk {
        matched: u32,
        /// `item_count()` — what `nucleo::tick` had ingested when the frame was taken. It
        /// lags the injector, so it is context rather than evidence.
        total: u32,
        /// How long after the walk was started this frame came back.
        at: Duration,
    }

    /// Poll `picker_query` and `fs_status` the way the overlay does, until either a frame
    /// with matches comes back mid-walk or the walk finishes.
    ///
    /// Returns how many frames were taken mid-walk alongside the first that had matches, so a
    /// run that observed *nothing* can be reported as the inconclusive thing it is rather
    /// than as a pass.
    async fn watch_one_walk(
        registry: &FsRegistry,
        project: ProjectId,
        started: Instant,
    ) -> (usize, Option<MidWalk>) {
        let mut mid_walk_frames = 0usize;
        loop {
            // `NoIndex` here means the spawned task has not claimed the project yet — a race
            // with a task started microseconds ago, not a failure.
            let Ok(before) = status_of(registry, project).await else {
                tokio::task::yield_now().await;
                continue;
            };
            if !before.indexing {
                return (mid_walk_frames, None);
            }

            let frame = query_project(registry, project, NEEDLE.to_string(), Some(50))
                .await
                .expect("picker_query is answerable the whole time the walk runs");
            let at = started.elapsed();

            // The closing bracket. Without it the frame could have been taken after the last
            // inode was read, which is exactly the case this test exists to rule out.
            let after = status_of(registry, project)
                .await
                .expect("the project is still indexed");
            if !after.indexing {
                return (mid_walk_frames, None);
            }

            mid_walk_frames += 1;
            if frame.matched > 0 {
                return (
                    mid_walk_frames,
                    Some(MidWalk {
                        matched: frame.matched,
                        total: frame.total,
                        at,
                    }),
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_picker_answers_early_in_the_walk_the_command_layer_started() {
        let corpus = corpus("cmd-stream");
        let counter = Arc::new(Counting::default());
        let mut mid_walk_frames = 0usize;
        let mut streamed: Option<MidWalk> = None;
        // Frames that had matches but arrived too late to be evidence of streaming, kept for
        // the failure message: "nothing matched" and "everything matched at the end" are
        // different bugs and send a reader to different code.
        let mut late: Vec<(MidWalk, Duration)> = Vec::new();

        for _ in 0..ATTEMPTS {
            // A registry per attempt, not one reused across all five. `Indexing::run` clears
            // the matcher *after* the claim, so a retry against a registry that already holds
            // the corpus has a window where the picker answers from the previous walk with
            // `indexing: true` — a frame that would pass this test while proving nothing.
            let registry = Arc::new(FsRegistry::default());
            let project = ProjectId::new();
            let started = Instant::now();

            let walk = {
                let registry = Arc::clone(&registry);
                let events: Arc<dyn FsEvents> = counter.clone();
                let roots = vec![corpus.path().to_path_buf()];
                tokio::spawn(async move { index_project(events, &registry, project, roots).await })
            };

            let (frames, found) = watch_one_walk(&registry, project, started).await;
            mid_walk_frames += frames;

            let status = walk
                .await
                .expect("the indexing task")
                .expect("fs_index over a directory that exists");
            // Everything after the last inode — installing the index, starting the watcher —
            // is counted as part of the walk, which can only make the ratio below *kinder*
            // to a build that does not stream.
            let walked = started.elapsed();
            assert!(
                !status.indexing,
                "fs_index answered with a walk it had not finished"
            );
            assert_eq!(
                status.files,
                (PACKAGES * PER_PACKAGE) as u32,
                "the walk did not report the corpus it was pointed at"
            );

            // Stops the watcher this walk started before the next attempt plants another.
            drop(registry.remove(project));

            match found {
                Some(seen) if seen.at * EARLY < walked => {
                    streamed = Some(seen);
                    break;
                }
                Some(seen) => late.push((seen, walked)),
                None => {}
            }
        }

        assert!(
            mid_walk_frames > 0,
            "every poll landed after the walk had finished, so nothing was observed either \
             way — {} files is no longer enough of a corpus to see the property on this \
             machine. This is an inconclusive run, not a pass.",
            PACKAGES * PER_PACKAGE
        );
        let seen = streamed.unwrap_or_else(|| {
            panic!(
                "in {ATTEMPTS} walks, picker_query never returned a match in the first \
                 1/{EARLY} of one. {mid_walk_frames} frames were taken while fs_status \
                 reported indexing: true on both sides of them; the ones that matched at all \
                 arrived at {late:?} (frame, walk). The picker is not being filled as the \
                 walk runs — it is being filled after it."
            )
        });
        assert!(
            seen.total < (PACKAGES * PER_PACKAGE) as u32,
            "the matcher already held the whole corpus when it answered ({} matched of {} \
             injected, corpus {}), so the frame says nothing about streaming",
            seen.matched,
            seen.total,
            PACKAGES * PER_PACKAGE
        );
        assert!(
            counter.statuses.load(Ordering::Relaxed) >= 2,
            "fs.index emitted no cide://fs-status frames; the explorer would never re-read \
             the tree it attached to while it was empty"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_index_while_the_first_is_walking_does_not_start_a_second_walk() {
        let corpus = corpus("cmd-reindex");
        let registry = Arc::new(FsRegistry::default());
        let counter = Arc::new(Counting::default());
        let project = ProjectId::new();
        let roots = vec![corpus.path().to_path_buf()];

        let first = {
            let registry = Arc::clone(&registry);
            let events: Arc<dyn FsEvents> = counter.clone();
            let roots = roots.clone();
            tokio::spawn(async move { index_project(events, &registry, project, roots).await })
        };

        // Wait for the walk to be genuinely in flight before asking again. Issuing the second
        // call blind would usually land after the first had finished, and then this test would
        // be about re-indexing rather than about the concurrent case.
        let mut walking = false;
        while !first.is_finished() {
            if matches!(status_of(&registry, project).await, Ok(s) if s.indexing) {
                walking = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            walking,
            "the walk finished before it could be caught in flight; nothing was observed \
             about the concurrent case"
        );

        // Both windows react to `project.open`, so a second `fs.index` mid-walk is the
        // ordinary case rather than an error one. `indexing: true` coming straight back is
        // the signature of the already-running branch: had it claimed a second walk, it
        // would have awaited that walk and answered `indexing: false` with the corpus
        // injected twice.
        let events: Arc<dyn FsEvents> = counter.clone();
        let second = index_project(events, &registry, project, roots.clone())
            .await
            .expect("a second fs_index is not an error");
        assert!(
            second.indexing,
            "a second fs.index during a walk answered as if it had done a walk of its own"
        );

        let first = first.await.expect("the first walk").expect("fs_index");
        assert!(
            !first.indexing,
            "the flag outlived the walk that set it, so every later fs.index would return \
             early for ever"
        );
        assert_eq!(
            first.files,
            (PACKAGES * PER_PACKAGE) as u32,
            "the walk did not report the corpus it was pointed at"
        );
        // The count, not the file total. `files` is the size of whatever index ended up
        // installed, so it reads the same whether the corpus was walked once or twice — the
        // message this assertion replaced claimed to check "more or less than once" and
        // checked no such thing.
        assert_eq!(
            counter.walks_started.load(Ordering::Relaxed),
            1,
            "the corpus was walked more than once"
        );

        drop(registry.remove(project));
    }

    /// A second `fs.index` for a project that has *already been walked* must do nothing.
    ///
    /// This is the ordinary case rather than an exotic one, and it is not the one the
    /// concurrent test above covers. `store/workspace.ts` remembers what *this window* has
    /// indexed, but that memory is module state in one webview: a second window — and a
    /// detached pane is a window — starts with an empty map and asks for every project in the
    /// workspace on its first snapshot, long after the first window's walk finished. So does
    /// a reloaded webview. `ui/src/ipc/client.ts` promises the command behaves ("Safe to call
    /// twice; the second is a no-op") and the frontend relies on that promise instead of
    /// deciding which window owns a project.
    ///
    /// The cost of getting it wrong is not just a wasted walk. `Indexing::run` opens by
    /// dropping the watcher and clearing the matcher, so a re-walk blanks the *first*
    /// window's Ctrl+P and stops its file events until the new walk catches up — seconds, on
    /// the large repository this milestone exists for.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_index_of_a_walked_project_does_not_walk_it_again() {
        const FILES: u32 = 8;
        let dir = scratch("cmd-reindex-settled");
        for n in 0..FILES {
            std::fs::write(dir.path().join(format!("{NEEDLE}{n}.rs")), []).expect("a corpus file");
        }
        let registry = FsRegistry::default();
        let counter = Arc::new(Counting::default());
        let project = ProjectId::new();
        let roots = vec![dir.path().to_path_buf()];

        let events: Arc<dyn FsEvents> = counter.clone();
        let first = index_project(events, &registry, project, roots.clone())
            .await
            .expect("the first index");
        assert_eq!(first.files, FILES, "the walk did not see the corpus");
        assert_eq!(
            counter.walks_started.load(Ordering::Relaxed),
            1,
            "the first fs.index did not walk"
        );
        let before = settled_matches(&registry, project, FILES).await;
        assert_eq!(
            before, FILES,
            "the picker did not end up holding the corpus"
        );

        let events: Arc<dyn FsEvents> = counter.clone();
        let second = index_project(events, &registry, project, roots)
            .await
            .expect("a second fs.index is not an error");

        assert_eq!(
            counter.walks_started.load(Ordering::Relaxed),
            1,
            "a second fs.index over the same roots started a second walk. A second window \
             opening on this project would re-walk the whole repository and, worse, blank the \
             matcher and drop the watcher the first window is using."
        );
        assert_eq!(
            second.files, first.files,
            "the no-op answered with a different tree than the walk built"
        );
        assert!(
            !second.indexing,
            "the no-op answered as if a walk were running, so the picker would poll for a \
             walk that is never going to end"
        );
        // The observable the user would have lost. `matched`, not a status field, because the
        // matcher being cleared is what empties Ctrl+P.
        let after = query_project(&registry, project, NEEDLE.to_string(), Some(50))
            .await
            .expect("the picker still answers");
        assert_eq!(
            after.matched, before,
            "the second fs.index cleared the matcher, so opening a second window empties the \
             first window's file picker"
        );

        drop(registry.remove(project));
    }

    /// A project whose roots changed is still re-walked — the no-op above must not swallow it.
    ///
    /// The pair matters: making a repeat `fs.index` cheap is only correct if "repeat" means
    /// *the same roots*. A project that gains a submodule root and is then never re-walked is
    /// the silent half of this bug — a tree permanently missing a directory, with no error
    /// anywhere to say why.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_project_whose_roots_changed_is_walked_again() {
        let first_root = scratch("cmd-roots-a");
        let second_root = scratch("cmd-roots-b");
        std::fs::write(first_root.path().join(format!("{NEEDLE}0.rs")), []).expect("a file");
        std::fs::write(second_root.path().join(format!("{NEEDLE}1.rs")), []).expect("a file");
        std::fs::write(second_root.path().join(format!("{NEEDLE}2.rs")), []).expect("a file");

        let registry = FsRegistry::default();
        let counter = Arc::new(Counting::default());
        let project = ProjectId::new();

        let events: Arc<dyn FsEvents> = counter.clone();
        let one = index_project(
            events,
            &registry,
            project,
            vec![first_root.path().to_path_buf()],
        )
        .await
        .expect("the first index");
        assert_eq!(one.files, 1);

        let events: Arc<dyn FsEvents> = counter.clone();
        let two = index_project(
            events,
            &registry,
            project,
            vec![
                first_root.path().to_path_buf(),
                second_root.path().to_path_buf(),
            ],
        )
        .await
        .expect("the re-index");

        assert_eq!(
            counter.walks_started.load(Ordering::Relaxed),
            2,
            "a project that gained a root was not walked again, so the new root's files exist \
             in neither the tree nor the picker"
        );
        assert_eq!(
            two.files, 3,
            "the re-index reported the old root set rather than the new one"
        );

        drop(registry.remove(project));
    }

    /// A frame taken before the walk has injected anything still says "ask me again".
    ///
    /// `PickerFrame::running` is the overlay's entire stopping condition — `FilePicker.tsx`
    /// re-polls if and only if it is set — but `Matcher::frame` fills it from `nucleo::tick`,
    /// which reports whether the *scorer* has queued work, not whether the walk is done. The
    /// two disagree in exactly one place, and `claim` putting the project in the registry
    /// before the walk starts is what makes that place reachable: between the claim and the
    /// first batch the matcher is empty and idle, so `nucleo` reports `running: false` over a
    /// repository that has not been read yet. The overlay stops polling and shows an empty
    /// list until the user types another character — and Ctrl+P straight after opening a
    /// project lands there.
    ///
    /// Held rather than raced: the claim is kept un-run, which *is* that window, so this
    /// tests the property instead of trying to hit a few microseconds of a real walk.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_frame_taken_before_the_walk_injects_anything_still_asks_to_be_polled() {
        let dir = scratch("cmd-picker-early");
        std::fs::write(dir.path().join(format!("{NEEDLE}0.rs")), []).expect("a corpus file");
        let registry = FsRegistry::default();
        let project = ProjectId::new();

        let claimed = registry
            .claim(project, vec![dir.path().to_path_buf()])
            .expect("a project nobody has indexed claims");

        let frame = query_project(&registry, project, NEEDLE.to_string(), Some(50))
            .await
            .expect("the picker is answerable from the instant the project is claimed");
        assert_eq!(frame.matched, 0, "nothing has been walked yet");
        assert!(
            frame.running,
            "the frame said the picker had settled while the walk had not read an inode. The \
             overlay stops polling on exactly this and would show an empty Ctrl+P over a \
             repository it is in the middle of indexing."
        );

        // Dropping the claim without running it clears the flag, which is the same guard that
        // covers a walk that panicked — and the picker must then stop asking.
        drop(claimed);
        let settled = query_project(&registry, project, NEEDLE.to_string(), Some(50))
            .await
            .expect("still answerable");
        assert!(
            !settled.running,
            "`running` outlived the walk, so the overlay would poll for ever"
        );

        drop(registry.remove(project));
    }

    /// The ignore rules handed out by `ProjectFs::filter` are the ones the walk installed.
    ///
    /// The accessor exists so `cmd::search` stops rebuilding an identical `Filter` from
    /// `dir_paths()` at the start of every query — a `stat` per directory in the project. That
    /// is only a saving if the two really are identical, and the thing that could make them
    /// differ is *which* filter comes back: the one built in `ProjectFs::new` has been given
    /// no directories, so it has never read a nested `.gitignore`, and a search running under
    /// it would report hits the file tree refuses to show.
    ///
    /// Asserted on both sides of the walk, because "the nested rule is enforced" would also be
    /// true of a filter that got it from somewhere else — the pre-walk reading is what shows
    /// the accessor is following the walk.
    #[test]
    fn a_projects_filter_is_the_one_its_walk_built() {
        let dir = scratch("cmd-filter");
        std::fs::create_dir_all(dir.path().join("src")).expect("a source directory");
        // Nested, so only a `Filter` that was told about `src/` can know the rule exists.
        std::fs::write(dir.path().join("src/.gitignore"), "generated.rs\n").expect("a .gitignore");
        std::fs::write(dir.path().join("src/generated.rs"), []).expect("a corpus file");
        std::fs::write(dir.path().join(format!("src/{NEEDLE}.rs")), []).expect("a corpus file");

        let registry = FsRegistry::default();
        let project = ProjectId::new();
        let generated = dir.path().join("src/generated.rs");

        let claimed = registry
            .claim(project, vec![dir.path().to_path_buf()])
            .expect("a project nobody has indexed claims");
        let fs = registry
            .get(project)
            .expect("the claim registered the project");
        assert!(
            fs.filter().admits(&generated, false),
            "the pre-walk filter has read no directory, so it cannot know this rule — if it \
             did, this test could not tell the two filters apart"
        );

        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());
        let status = claimed.run(events, project);
        assert_eq!(status.files, 1, "the walk itself honours the nested rule");
        assert!(
            !fs.filter().admits(&generated, false),
            "the accessor still returns the filter from before the walk, so a content search \
             using it would report hits inside directories the tree does not show"
        );

        drop(registry.remove(project));
    }

    /// The row is in the tree the instant `fs_create_in` returns — no watcher involved.
    ///
    /// This is the claim the file tree's *New File…* rests on, and it is a claim about *this*
    /// wiring rather than about `cide-fs`: `ops::create_in` writes an inode and knows nothing
    /// about an index, and `Index::apply` folds a path in and knows nothing about a command.
    /// Joining them here is what makes the row appear now instead of whenever the watcher's
    /// debounce elapses — a few hundred milliseconds, which is exactly long enough for the
    /// user to decide the menu item did nothing and click it again.
    ///
    /// The watcher is deliberately not waited for. Its event arrives afterwards and rescans
    /// the same directory; the second assertion below is that doing so changes nothing, which
    /// is also why the picker cannot end up holding the file twice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_created_file_is_a_row_before_the_watcher_has_said_anything() {
        let dir = scratch("cmd-create-now");
        std::fs::create_dir(dir.path().join("src")).expect("a source directory");
        let registry = FsRegistry::default();
        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());
        let project = ProjectId::new();

        index_project(events, &registry, project, vec![dir.path().to_path_buf()])
            .await
            .expect("the walk");
        let fs = registry.get(project).expect("an indexed project");
        // Expanded first, exactly as the panel does before it shows its draft row: a new file
        // inside a collapsed folder is in the index and contributes no *visible* row, and
        // asserting on `count()` without this would be asserting about the twisty.
        fs.with_index_mut(|index| index.expand(&dir.path().join("src")));
        let before = fs.with_index(|index| index.count());

        let created = create_entry(&fs, &dir.path().join("src"), "main.rs", false)
            .expect("creating a file in a directory that exists");
        assert_eq!(created, dir.path().join("src/main.rs"));

        let rows = fs.with_index(|index| index.rows(0, 64));
        assert_eq!(
            fs.with_index(|index| index.count()),
            before + 1,
            "the tree did not grow, so the user's new file has no row until the watcher fires"
        );
        assert!(
            rows.iter()
                .any(|r| r.path == created && r.name == "main.rs"),
            "the new path is not among the rows the panel would draw: {rows:?}"
        );

        // The picker too. A file created here and then absent from Ctrl+P until the next full
        // walk is the quieter half of the same omission. Polled rather than read once:
        // `nucleo` scores on its own pool, so the first frame after an injection legitimately
        // reports fewer matches than the matcher holds.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut matched = 0;
        while matched == 0 && Instant::now() < deadline {
            matched = query_project(&registry, project, "mainrs".to_string(), Some(50))
                .await
                .expect("the picker answers for an indexed project")
                .matched;
            tokio::task::yield_now().await;
        }
        assert!(
            matched > 0,
            "the created file never reached the picker, so Ctrl+P cannot open the file the \
             user just made until the project is walked again"
        );

        // The watcher's own event for this create, replayed. It must reconcile to nothing —
        // if `apply` added the node a second time the count would climb again.
        let filter = fs.filter();
        let change = FsChange {
            paths: vec![created.clone()],
            truncated: false,
            git: false,
        };
        let added = fs.with_index_mut(|index| index.apply(&change, &filter));
        assert!(
            added.is_empty(),
            "the watcher's event added the path a second time, so the picker holds it twice"
        );
        assert_eq!(fs.with_index(|index| index.count()), before + 1);

        drop(registry.remove(project));
    }

    /// A paste moves the rows in the same repaint, in **both** directions.
    ///
    /// The claim `cide-fs` cannot make: `copy::paste` writes bytes and knows nothing about an
    /// index, and this wiring is what puts the result on screen without waiting out the
    /// watcher's debounce. A cut is the case that needs both halves — the row it *left* has to
    /// go at the same moment the row it *arrived at* appears, or the file is visibly in two
    /// places for a few hundred milliseconds and the tree looks like it duplicated it.
    ///
    /// That is why `paste_into` names the sources as well as the destinations in the change it
    /// folds: `Index::apply` rescans the parent of every path it is given, and naming only the
    /// destinations leaves the source's directory unread.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pasted_path_is_a_row_and_a_cut_source_stops_being_one_immediately() {
        let dir = scratch("cmd-paste-now");
        std::fs::create_dir(dir.path().join("src")).expect("a source directory");
        std::fs::create_dir(dir.path().join("dest")).expect("a destination directory");
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").expect("a file to move");
        let registry = FsRegistry::default();
        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());
        let project = ProjectId::new();

        index_project(events, &registry, project, vec![dir.path().to_path_buf()])
            .await
            .expect("the walk");
        let fs = registry.get(project).expect("an indexed project");
        // Both folders expanded, exactly as a user pasting between two visible directories
        // would have them: a row inside a collapsed folder is in the index and contributes no
        // visible row, so the counts below would be about twisties rather than about rows.
        fs.with_index_mut(|index| index.expand(&dir.path().join("src")));
        fs.with_index_mut(|index| index.expand(&dir.path().join("dest")));
        let before = fs.with_index(|index| index.count());

        let pasted = paste_into(
            &fs,
            &[dir.path().join("src/main.rs")],
            &dir.path().join("dest"),
            cide_ipc::PasteMode::Cut,
            &[],
        )
        .expect("moving a file between two folders of the same project");
        assert_eq!(pasted[0].dest, dir.path().join("dest/main.rs"));

        let rows = fs.with_index(|index| index.rows(0, 64));
        assert!(
            rows.iter().any(|r| r.path == pasted[0].dest),
            "the moved file has no row until the watcher fires: {rows:?}"
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.path == dir.path().join("src/main.rs")),
            "the row it moved OUT of is still drawn, so the tree shows the file twice: {rows:?}"
        );
        assert_eq!(
            fs.with_index(|index| index.count()),
            before,
            "a move added a row without removing one"
        );

        // The watcher's own event for the same move, replayed. It must reconcile to nothing.
        let filter = fs.filter();
        let change = FsChange {
            paths: vec![dir.path().join("src/main.rs"), pasted[0].dest.clone()],
            truncated: false,
            git: false,
        };
        let added = fs.with_index_mut(|index| index.apply(&change, &filter));
        assert!(added.is_empty(), "the watcher's event added the path again");
        assert_eq!(fs.with_index(|index| index.count()), before);

        drop(registry.remove(project));
    }

    /// A paste that *replaced* a file adds no row, because the row was already there.
    ///
    /// The fold names every destination unconditionally, and an overwrite's destination is a
    /// path the index already holds. `Index::apply` rescanning its parent has to reconcile that
    /// to nothing — the version that appends what it is given would leave the tree drawing the
    /// same file twice, which looks exactly like the duplicate the user chose Replace to avoid.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replacing_a_file_leaves_the_row_it_already_had() {
        let dir = scratch("cmd-paste-replace");
        std::fs::create_dir(dir.path().join("src")).expect("a source directory");
        std::fs::create_dir(dir.path().join("dest")).expect("a destination directory");
        std::fs::write(dir.path().join("src/main.rs"), "new").expect("a file to paste");
        std::fs::write(dir.path().join("dest/main.rs"), "old").expect("a file to replace");
        let registry = FsRegistry::default();
        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());
        let project = ProjectId::new();

        index_project(events, &registry, project, vec![dir.path().to_path_buf()])
            .await
            .expect("the walk");
        let fs = registry.get(project).expect("an indexed project");
        fs.with_index_mut(|index| index.expand(&dir.path().join("src")));
        fs.with_index_mut(|index| index.expand(&dir.path().join("dest")));
        let before = fs.with_index(|index| index.count());

        let source = dir.path().join("src/main.rs");
        let pasted = paste_into(
            &fs,
            std::slice::from_ref(&source),
            &dir.path().join("dest"),
            cide_ipc::PasteMode::Copy,
            &[cide_ipc::PasteDecision {
                source: source.clone(),
                choice: cide_ipc::PasteChoice::Replace,
            }],
        )
        .expect("replacing a file the user was asked about");

        assert_eq!(pasted[0].dest, dir.path().join("dest/main.rs"));
        assert_eq!(pasted[0].replaced, 1);
        assert_eq!(
            std::fs::read_to_string(&pasted[0].dest).expect("the replaced file"),
            "new"
        );
        assert_eq!(
            fs.with_index(|index| index.count()),
            before,
            "an overwrite added a row for a path that already had one"
        );

        drop(registry.remove(project));
    }

    /// The containment check, driven through the command layer rather than through `copy`.
    ///
    /// `cide-fs` has its own tests for every rule; what is pinned here is that this handler
    /// resolves the *project's* roots and hands them over — the version that forgets to is the
    /// one that copies `/etc/shadow` into the user's repository because a webview asked it to.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pasting_from_outside_the_project_is_refused_by_the_handler() {
        let dir = scratch("cmd-paste-outside");
        let outside = scratch("cmd-paste-elsewhere");
        std::fs::write(outside.path().join("secret"), "s").expect("a file outside the project");
        let registry = FsRegistry::default();
        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());
        let project = ProjectId::new();

        index_project(events, &registry, project, vec![dir.path().to_path_buf()])
            .await
            .expect("the walk");
        let fs = registry.get(project).expect("an indexed project");

        assert!(matches!(
            paste_into(
                &fs,
                &[outside.path().join("secret")],
                dir.path(),
                cide_ipc::PasteMode::Copy,
                &[]
            ),
            Err(FsError::OutsideProject(_))
        ));
        assert!(!dir.path().join("secret").exists());

        drop(registry.remove(project));
    }

    /// The three refusals, driven through the command layer rather than through `ops`.
    ///
    /// `cide-fs` has its own tests for each rule; what is pinned here is that the handler
    /// resolves the project's roots and hands them over, because the version of this command
    /// that forgets to is the one that writes wherever the webview asked.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn creating_is_refused_for_a_vanished_parent_a_taken_name_and_a_path_outside_the_project()
    {
        let dir = scratch("cmd-create-refuse");
        let outside = scratch("cmd-create-outside");
        std::fs::write(dir.path().join("taken.rs"), []).expect("an existing file");
        let registry = FsRegistry::default();
        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());
        let project = ProjectId::new();

        index_project(events, &registry, project, vec![dir.path().to_path_buf()])
            .await
            .expect("the walk");
        let fs = registry.get(project).expect("an indexed project");
        let rows = fs.with_index(|index| index.count());

        // A directory the tree still draws a row for, deleted since it was drawn. `ops::create`
        // would have called `create_dir_all` and put it back.
        let gone = dir.path().join("gone");
        std::fs::create_dir(&gone).expect("a directory");
        std::fs::remove_dir(&gone).expect("removed under the tree");
        assert!(matches!(
            create_entry(&fs, &gone, "a.rs", false),
            Err(FsError::Io { .. })
        ));
        assert!(!gone.exists(), "a refused create resurrected the folder");

        assert!(matches!(
            create_entry(&fs, dir.path(), "taken.rs", false),
            Err(FsError::Exists(_))
        ));

        assert!(matches!(
            create_entry(&fs, outside.path(), "a.rs", false),
            Err(FsError::OutsideProject(_))
        ));
        assert!(
            !outside.path().join("a.rs").exists(),
            "a path outside every project root was obeyed rather than refused"
        );

        assert!(matches!(
            create_entry(&fs, dir.path(), "sub/a.rs", false),
            Err(FsError::InvalidPath(_))
        ));

        assert_eq!(
            fs.with_index(|index| index.count()),
            rows,
            "a refused create still moved the tree"
        );

        drop(registry.remove(project));
    }

    /// `FsError::NoIndex` reaches the webview as `{"kind":"noIndex"}`.
    ///
    /// Pinned here because two frontend behaviours are matched on that exact tag and nothing
    /// else in the workspace checks it: `ui/src/store/fileIndex.ts`'s `isNoIndex` is what
    /// stops a project that is merely mid-walk from being reported as *"fs_tree_count is not
    /// registered in this build"*, and it is what makes the picker say `Indexing…` and poll.
    /// Both fail *open* — a predicate that stops matching does not throw, it silently restores
    /// the exact bug it was written for — and `check-picker.mjs` asserts against a hand-written
    /// `{ kind: 'noIndex' }` fixture that cannot notice the Rust moving out from under it.
    #[test]
    fn no_index_serialises_as_the_tag_the_frontend_matches_on() {
        assert_eq!(
            serde_json::to_value(FsError::NoIndex).expect("FsError is Serialize"),
            serde_json::json!({ "kind": "noIndex" }),
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_project_with_no_roots_has_no_index_rather_than_an_empty_one() {
        let registry = FsRegistry::default();
        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());
        let project = ProjectId::new();

        // `fs_index` resolves the roots from the workspace, and a project id the workspace
        // does not know answers with an empty list. Indexing that would register an entry
        // whose picker is permanently empty and whose tree is permanently zero rows, which
        // reads to every caller as "this repository has no files".
        let error = index_project(events, &registry, project, Vec::new())
            .await
            .expect_err("a rootless project cannot be indexed");
        assert_eq!(error, FsError::NoIndex);
        assert!(status_of(&registry, project).await.is_err());
    }
}
