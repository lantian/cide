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

// --- one tree out of two ------------------------------------------------------------------
//
// The file tree the user sees is the concatenation of two independent row sources: the walked,
// watched, gitignore-filtered `cide_fs::Index`, and the lazy, unwatched, unindexed
// `cide_fs::groups::Groups` that draws *External Libraries* and *Scratches*. The composition
// lives here, at the command layer, and not inside either of them — that is the whole point of
// the split, and `cide_fs::groups`'s header says what grafting one into the other would have
// cost.
//
// The arithmetic is four lines and every one of them is an off-by-one waiting to happen, so it
// is written once, in named functions, with a test at the foot of this file that sweeps every
// window of every size across the seam.

/// Everything a tree read needs to do before it answers, on every one of them.
///
/// [`crate::groups::ProjectGroups::prepare`] decides which group rows exist (one atomic after
/// the first call), and the stamp check notices a `Cargo.lock` that moved. Both are here rather
/// than in `fs_index` because a second window attaching to an already-indexed project never
/// calls `fs_index` at all — its first contact with this project is `fs_tree_count`.
///
/// `events` is `None` for the reads that cannot start work — the two that only look. A stale
/// group noticed by a read that cannot resolve is left marked stale; the next `fs_tree_count`
/// picks it up, and that is a fraction of a second later.
fn prepare(fs: &Arc<crate::files::ProjectFs>, resolve: Option<(&Arc<dyn FsEvents>, ProjectId)>) {
    // Guarded, so the ordinary call — every tree read after the first for this project — costs
    // one atomic load and does not even build the root list.
    if fs.groups().needs_prepare() {
        fs.groups().prepare(&fs.root_paths());
    }
    let Some((events, project)) = resolve else {
        return;
    };
    if fs.groups().stale() == Some(cide_fs::groups::Expanded::Resolve) {
        crate::libraries::spawn_resolve(Arc::clone(fs), Arc::clone(events), project);
    }
}

/// The composed row count: the index's rows, then the groups'.
fn tree_count(fs: &crate::files::ProjectFs) -> usize {
    fs.with_index(|index| index.count()) + fs.groups().count()
}

/// Serve one row window from two sources laid end to end.
///
/// A named function with `compose_serves_every_window_across_the_seam` behind it, rather than
/// four lines inline in the handler, because the failure it prevents is invisible: a window
/// straddling the seam that asks the second source for the wrong offset draws a *correct-looking*
/// list of rows with a few missing in the middle, and no test that only checks the two ends can
/// see it. The tree is virtualized, so the straddling window is not an edge case — it is what
/// every scroll past the last project file produces.
///
/// Both sources clamp out-of-range requests to `[]` (`Index::rows` and `Groups::rows` both say so
/// in their own docs), so this never has to bounds-check either of them.
fn compose(
    first_len: usize,
    offset: usize,
    len: usize,
    first: impl FnOnce(usize, usize) -> Vec<TreeRow>,
    second: impl FnOnce(usize, usize) -> Vec<TreeRow>,
) -> Vec<TreeRow> {
    let mut rows = if offset < first_len {
        first(offset, len)
    } else {
        Vec::new()
    };
    if rows.len() < len {
        // Where the second source starts: `0` for a window that straddles the seam — the first
        // source has already served everything up to it — and `offset - first_len` for one that
        // begins past the seam entirely. `saturating_sub` is both cases in one expression.
        rows.extend(second(offset.saturating_sub(first_len), len - rows.len()));
    }
    rows
}

/// How many rows the tree currently has. The virtual scroller's range.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_tree_count(
    app: tauri::AppHandle,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
) -> Result<u32, FsError> {
    let fs = project_fs(&registry, project)?;
    let events: Arc<dyn FsEvents> = Arc::new(app);
    blocking("fs_tree_count", move || {
        prepare(&fs, Some((&events, project)));
        tree_count(&fs) as u32
    })
    .await
}

/// The rows in `[offset, offset + len)`.
///
/// The window is served from the index while it lasts and from the groups afterwards, which is
/// what makes a window *straddling* the seam the interesting case: it asks the index for its
/// tail and the groups for their head, and returns the two concatenated. Both sources clamp an
/// out-of-range request to `[]` rather than erroring, so the arithmetic never has to.
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
        prepare(&fs, None);
        let walked = fs.with_index(|index| index.count());
        compose(
            walked,
            offset as usize,
            len,
            |offset, len| fs.with_index(|index| index.rows(offset, len)),
            |offset, len| fs.groups().rows(offset, len),
        )
    })
    .await
}

/// Expand a directory, or a synthetic group. Returns the new row count.
///
/// The index is asked first and the groups only if it declines, which is the right order for
/// two reasons: every expand in an ordinary project is an index row, and the index physically
/// cannot hold a path outside its roots, so "the index said no" is a complete answer.
///
/// Expanding an unresolved group **starts a resolution and returns immediately**, with the
/// *Resolving…* row already in the count it answers. A handler that waited for `cargo metadata`
/// would be a click that blocks a blocking-pool worker for a quarter of a second at best.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_expand(
    app: tauri::AppHandle,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<u32, FsError> {
    let fs = project_fs(&registry, project)?;
    let events: Arc<dyn FsEvents> = Arc::new(app);
    blocking("fs_expand", move || {
        prepare(&fs, Some((&events, project)));
        if fs.with_index_mut(|index| index.expand(&path)).is_some() {
            return Ok(tree_count(&fs) as u32);
        }
        match fs.groups().expand(&path) {
            Some(cide_fs::groups::Expanded::Resolve) => {
                crate::libraries::spawn_resolve(Arc::clone(&fs), events, project);
                Ok(tree_count(&fs) as u32)
            }
            Some(cide_fs::groups::Expanded::Ready) => Ok(tree_count(&fs) as u32),
            None => Err(FsError::InvalidPath(path.display().to_string())),
        }
    })
    .await?
}

/// Collapse a directory, or a synthetic group. Returns the new row count.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_collapse(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<u32, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_collapse", move || {
        prepare(&fs, None);
        if fs.with_index_mut(|index| index.collapse(&path)).is_some() {
            return Ok(tree_count(&fs) as u32);
        }
        fs.groups()
            .collapse(&path)
            .map(|()| tree_count(&fs) as u32)
            .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))
    })
    .await?
}

/// Expand everything above a path and return the row it sits on.
///
/// `None` rather than an error when the path is not in the tree: revealing a file that is
/// gitignored, or that has just been deleted, is an ordinary thing for the editor to ask and
/// the honest answer is "there is no row for that".
///
/// A path the index does not hold is offered to the groups, which is what makes *Select Opened
/// File* work for a tab opened by Go to definition into `~/.cargo/registry`, and for a scratch.
/// That walk materialises a chain that has never been expanded — one `read_dir` per level.
///
/// # The one place a reveal is allowed to block
///
/// A group that has never been expanded holds no rows, so `Groups::reveal` cannot find a file
/// inside it and this would answer `None` — and the caller would tell the user that a file they
/// are looking at *is not in this project's file tree*. That is not a degradation, it is a
/// false statement, and it is the exact class of answer this milestone exists to stop giving.
///
/// So when both sources decline **and** the path is one an unresolved *External Libraries*
/// would hold, the resolution is run here, on this blocking worker, before the second attempt.
/// [`crate::groups::ProjectGroups::resolve_now`] states the trade in full; the short version is
/// that the gate is I/O-free, the ordinary reveal never reaches it, and it can happen at most
/// once per project per process. *Scratches* needs none of this — its listing is eager — which
/// is why the retry is gated on the library predicate rather than run unconditionally.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_reveal(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
) -> Result<Option<u32>, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_reveal", move || reveal_path(&fs, &path)).await
}

/// [`fs_reveal`]'s body, over values.
///
/// A named function for the same reason [`index_project`] and [`create_entry`] are: the
/// property worth testing — that a tab opened by Go to definition into `~/.cargo/registry` can
/// be found in the tree even though nobody has ever opened the group — is a property of *this*
/// sequence, and a body inline in the `#[tauri::command]` item cannot be called from a test.
pub(crate) fn reveal_path(
    fs: &Arc<crate::files::ProjectFs>,
    path: &std::path::Path,
) -> Option<u32> {
    prepare(fs, None);
    if let Some(row) = fs.with_index_mut(|index| index.reveal(path)) {
        return Some(row as u32);
    }
    if let Some(row) = reveal_in_groups(fs, path) {
        return Some(row);
    }
    if !fs.groups().is_unlisted_library_path(path, &fs.root_paths()) {
        return None;
    }
    // `resolve_now` answers false when another thread already holds the resolution — a click on
    // the header a moment earlier. Nothing is retried in that case and the caller reports:
    // waiting on somebody else's `cargo` inside a keystroke would turn a gesture that usually
    // costs nothing into one that occasionally costs a cold build.
    if !fs.groups().resolve_now(&fs.root_paths()) {
        return None;
    }
    reveal_in_groups(fs, path)
}

/// A group row's index in the **composed** tree: the index's rows come first.
fn reveal_in_groups(fs: &crate::files::ProjectFs, path: &std::path::Path) -> Option<u32> {
    let walked = fs.with_index(|index| index.count());
    fs.groups().reveal(path).map(|row| (walked + row) as u32)
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

/// What is on **disk** at each of these paths — the oracle for terminal links that leave the
/// project.
///
/// [`fs_paths_exist`]'s disk-touching sibling, and a separate command rather than a flag on it
/// because the two have different costs and the difference has to stay visible at the call site.
/// That one is a hash lookup against a tree that has already been walked and answers `None` for
/// everything the index cannot see, which since M13 includes the case this command exists for:
/// a path outside every root, which the index will never hold however long it walks.
///
/// # Why the project-scoped one could not simply be relaxed
///
/// Two properties would have gone with it. `fs_paths_exist` is asked from `provideLinks` while
/// the pointer crosses a line of output, so *in-project hovering costs zero syscalls* — that is
/// a design property, written down where it is relied on, and putting a `stat` behind the same
/// name would have made every hover in every pane pay filesystem latency to buy a case that
/// only arises for absolute paths naming somewhere else. Splitting them also puts the new cost
/// in one greppable place: if terminal hovering ever gets slow on a stalled mount, this is the
/// command to look at, and it has exactly one caller.
///
/// # What it does and does not reveal
///
/// It answers *file / directory / nothing* for absolute paths anywhere on the machine, so it
/// tells the webview whether an arbitrary path exists. That is not a new capability — `file_read`
/// in `cmd::file` has no containment at all, so a *compromised webview* already reads any file —
/// and it is not what this feature's threat model is about, which is attacker-chosen bytes on
/// screen plus one unsuspecting ctrl+click. Against that, the defence is the confirmation
/// `cmd::file::terminal_open_path` refuses into, not the answer to a `stat`.
///
/// A relative path answers `None` rather than being resolved against cide's own cwd, which is
/// not the project and is not anything the user could have meant. Clamped to [`MAX_PROBE`] like
/// its sibling, and on the blocking pool because `stat` on a stalled network mount blocks.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_stat_paths(
    paths: Vec<PathBuf>,
) -> Result<Vec<Option<cide_ipc::TreeRowKind>>, FsError> {
    blocking("fs_stat_paths", move || {
        paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                if i >= MAX_PROBE || !path.is_absolute() {
                    return None;
                }
                // `metadata`, not `symlink_metadata`: a symlink to a real file is a file as far
                // as opening it goes, and `openable` canonicalises on the click anyway. A
                // dangling symlink therefore answers `None`, which is the honest answer — the
                // click would refuse it as `Missing`.
                let meta = std::fs::metadata(path).ok()?;
                if meta.is_dir() {
                    Some(cide_ipc::TreeRowKind::Dir)
                } else if meta.is_file() {
                    Some(cide_ipc::TreeRowKind::File)
                } else {
                    // A FIFO, a socket or a device node. `None` rather than `File`, so the link
                    // is never offered for the one shape that would park a blocking-pool worker
                    // in `read_to_end` for ever if the click's own guard ever slipped.
                    None
                }
            })
            .collect()
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

        // `writable_paths`, so a scratch can be shown in a file manager. That is a *widening*
        // of a containment check and it is argued rather than assumed: the drawer is a
        // directory cide itself writes into and draws rows for, so opening it is exactly as
        // legitimate as renaming a file in it. A dependency source is still refused — that
        // group contributes no writable directory — which is the case the note in the tree's
        // context menu is about.
        ops::check_within(&fs.writable_paths(), &path)?;
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
        ops::check_within(&fs.writable_paths(), &path)?;
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
        ops::check_within(&fs.writable_paths(), &path)?;
        ops::write(&path, &contents)?;
        // Nothing watches the drawer, so a write that created a file there has to say so or
        // the row appears the next time somebody folds the group. Guarded, so the ordinary
        // write — a project file — costs one lock and a prefix test.
        let _ = relist_if_scratch(&fs, std::slice::from_ref(&path));
        Ok(())
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
        ops::check_within(&fs.writable_paths(), &path)?;
        ops::create(&path, directory)?;
        let _ = relist_if_scratch(&fs, std::slice::from_ref(&path));
        Ok(())
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
    let created = ops::create_in(&fs.writable_paths(), parent, name, directory)?;

    // A scratch is not in the index and never will be, so the fold below cannot show it: the
    // drawer is re-listed instead, which is the same "the row exists before this returns"
    // promise by the other mechanism. Returned early because the two are exclusive — a path
    // cannot be both inside a project root and inside the drawer.
    if fs.groups().is_scratch_path(&created) {
        fs.groups().relist_scratches();
        return Ok(created);
    }

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

/// Create a scratch file of `ext` in this project's drawer, and answer where it landed.
///
/// # Why the extension and not a language name
///
/// The list of *offered* types lives in `ui/src/editor/languages.ts`, beside the table that
/// decides which grammar a path loads and what the status bar calls it. Shipping that list to
/// Rust would make it a DTO that has to stay in step with a TypeScript record, and the failure
/// when it drifts is silent and specific: a scratch offered as *YAML* whose extension the
/// editor does not recognise opens with no highlighting and a status bar reading `Plain Text`.
/// So the frontend names an extension, one assertion in `check:editor` pins that every offered
/// extension resolves through that same table, and this side validates the *shape* — see
/// `cide_core::scratch::check_ext`.
///
/// The file is created **empty and immediately** rather than opened as an unsaved buffer; the
/// three reasons are in `cide_core::scratch::create`. The drawer is re-listed before this
/// returns, so the row is in the tree the instant the command answers — the same promise
/// [`create_entry`] makes for a project file, by the other mechanism.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_scratch_new(
    app: tauri::AppHandle,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    ext: String,
) -> Result<PathBuf, FsError> {
    let fs = project_fs(&registry, project)?;
    let events: Arc<dyn FsEvents> = Arc::new(app);
    blocking("fs_scratch_new", move || {
        // `roots[0]`, which is the project's identity everywhere else in this codebase —
        // `RecentProject` is keyed by it and so is the scratch drawer. A multi-root project
        // gets one drawer, which is what "per project" means to the person using it.
        let root = fs
            .roots
            .first()
            .map(|root| root.path.clone())
            .ok_or(FsError::NoIndex)?;
        let created = fs
            .groups()
            .new_scratch(&root, &ext)
            .map_err(|error| FsError::Io {
                path: root.display().to_string(),
                message: error.to_string(),
            })?;
        // So the *other* window's Explorer redraws. The drawer is unwatched by design, so this
        // is the only thing that would ever tell it — and `FsStatus::rows` carries the composed
        // count, so its scroller resizes in the same frame the row appears.
        events.status(project, &fs.status());
        Ok(created)
    })
    .await?
}

/// Every directory the file tree's disk-changing verbs may act inside.
///
/// The project's roots, plus the scratch drawer. The frontend needs the same list Rust checks
/// against, because `ui/src/sidebar/rowPaths.ts::mutationRefusal` is what greys *Rename…*,
/// *Cut* and *Move to Trash* on a row before the click — and a menu that offers a verb the
/// handler then refuses is the dead control this panel has already shipped twice.
///
/// A command rather than a field on `FsStatus`, which is emitted on every watcher burst: this
/// answer changes when a project's roots change and at no other time, so it is asked once per
/// attach. It is deliberately **not** `Project::roots` widened — see
/// [`crate::files::ProjectFs::writable_paths`] for what that would break.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_writable_roots(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
) -> Result<Vec<PathBuf>, FsError> {
    let fs = project_fs(&registry, project)?;
    blocking("fs_writable_roots", move || {
        // Through `prepare`, because the drawer is only known once the groups have been shown
        // and this is routinely the *first* thing a freshly attached tree asks for. Without it
        // the answer would be the roots alone until some later read, and every scratch row
        // would be greyed for that window.
        if fs.groups().needs_prepare() {
            fs.groups().prepare(&fs.root_paths());
        }
        fs.writable_paths()
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn fs_rename(
    app: tauri::AppHandle,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    from: PathBuf,
    to: PathBuf,
) -> Result<(), FsError> {
    let fs = project_fs(&registry, project)?;
    let events: Arc<dyn FsEvents> = Arc::new(app);
    blocking("fs_rename", move || {
        if rename_entry(&fs, &from, &to)? {
            events.status(project, &fs.status());
        }
        Ok(())
    })
    .await?
}

/// [`fs_rename`] with its Tauri-injected arguments already resolved. Answers whether the scratch
/// drawer changed, which is what the caller turns into a `cide://fs-status`.
///
/// A named function for the same reason [`create_entry`] and [`reveal_path`] are: the property
/// worth pinning is **which list the containment check is given**, and that is a property of
/// this wiring rather than of `ops::rename`. A test that called `ops::check_within` itself would
/// agree with whatever list it chose, which is exactly the shape that let this handler keep
/// using the project roots while a scratch was supposed to be renamable.
pub(crate) fn rename_entry(
    fs: &crate::files::ProjectFs,
    from: &std::path::Path,
    to: &std::path::Path,
) -> Result<bool, FsError> {
    // `writable_paths`, so a scratch can be renamed — which also changes its *language*, because
    // `ui/src/editor/languages.ts` resolves by extension and `EditorSurface` reloads the grammar
    // when the path changes. That is the right behaviour and it is the reason a scratch is a
    // real file with a real name rather than a titled buffer.
    //
    // `check_not_root` gets the same list, so the drawer itself cannot be renamed away from
    // underneath the group — exactly as a project root cannot.
    let writable = fs.writable_paths();
    ops::check_within(&writable, from)?;
    ops::check_within(&writable, to)?;
    ops::check_not_root(&writable, from)?;
    ops::rename(from, to)?;
    Ok(relist_if_scratch(
        fs,
        &[from.to_path_buf(), to.to_path_buf()],
    ))
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
    app: tauri::AppHandle,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    paths: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, FsError> {
    let fs = project_fs(&registry, project)?;
    let events: Arc<dyn FsEvents> = Arc::new(app);
    blocking("fs_delete", move || {
        let (trashed, moved) = delete_entries(&fs, &paths)?;
        if moved {
            events.status(project, &fs.status());
        }
        Ok(trashed)
    })
    .await?
}

/// [`fs_delete`] with its Tauri-injected arguments already resolved. See [`rename_entry`] for
/// why this is a named function; the second half of the answer is whether the drawer changed.
pub(crate) fn delete_entries(
    fs: &crate::files::ProjectFs,
    paths: &[PathBuf],
) -> Result<(Vec<PathBuf>, bool), FsError> {
    let writable = fs.writable_paths();
    for path in paths {
        ops::check_within(&writable, path)?;
        ops::check_not_root(&writable, path)?;
    }
    let mut trashed = Vec::with_capacity(paths.len());
    for path in paths {
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
    // After the moves, not before: a delete that failed part way through has still removed the
    // rows it managed, and re-listing here is what stops the group showing files that are
    // already in the trash.
    let moved = relist_if_scratch(fs, paths);
    Ok((trashed, moved))
}

/// Re-list the scratch drawer if any of these paths was in it, and say whether it happened.
///
/// Every mutating handler above calls this rather than reasoning about which of them could
/// have changed the drawer, because the reasoning is what rots: a `read_dir` of a flat
/// directory is microseconds, and a group showing a file the user deleted a moment ago is the
/// bug this saves. There is no watcher to fall back on — see `crate::scratches`.
///
/// The answer is what the two handlers with an `AppHandle` turn into a `cide://fs-status`, so
/// a **second window** hears about it. That event is the one the Explorer already refreshes on,
/// and it is needed here for the same reason the drawer is re-listed at all: with no watcher on
/// it, nothing else would ever tell the other window.
fn relist_if_scratch(fs: &crate::files::ProjectFs, paths: &[PathBuf]) -> bool {
    if !paths.iter().any(|path| fs.groups().is_scratch_path(path)) {
        return false;
    }
    fs.groups().relist_scratches();
    true
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
        cide_fs::copy::plan(&fs.writable_paths(), &sources, &dest_dir, mode)
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
    // `writable_paths`, so the drawer is a place things can be pasted into and out of. The
    // out-of direction is the one that matters: a scratch that turned out to be worth keeping
    // is copied into the project with the gesture the user already knows.
    let writable = fs.writable_paths();
    let pasted = cide_fs::copy::paste_with(&writable, sources, dest_dir, mode, decisions)?;

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
    // The fold above cannot reach the drawer, so anything that landed there — or left it —
    // needs the other mechanism. `change.paths` is sources *and* destinations, which is
    // exactly the set that matters for a cut out of the drawer as well as a paste into it.
    // No `fs-status` here, and that is a gap rather than a decision: `fs_paste` has no
    // `AppHandle` and `paste_into` is also called from tests with none, so a second window sees
    // a scratch cut into the project on its next refresh rather than at once. The window that
    // made the gesture is correct immediately, which is the case that matters.
    let _ = relist_if_scratch(fs, &change.paths);
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

/// The seam between the two row sources, and the group whose rows sit past it.
///
/// A module of its own rather than more tests in the one above, which is about the picker
/// answering during a walk and drags a 30 000-file corpus in with it. Nothing here writes a
/// corpus; the point is arithmetic.
#[cfg(test)]
mod compose_tests {
    use super::*;
    use cide_fs::groups::{Entry, Groups, group_path};
    use cide_ipc::{NO_ROOT, TreeRowKind};

    /// A row source that answers with names, so a window can be compared to a list of strings.
    fn source(names: &'static [&'static str]) -> impl Fn(usize, usize) -> Vec<TreeRow> {
        move |offset: usize, len: usize| {
            names
                .iter()
                .skip(offset)
                .take(len)
                .map(|name| TreeRow {
                    path: PathBuf::from(*name),
                    name: (*name).to_string(),
                    depth: 0,
                    kind: TreeRowKind::File,
                    expanded: false,
                    has_children: false,
                    symlink: false,
                    root: NO_ROOT,
                    detail: None,
                })
                .collect()
        }
    }

    /// Every window of every size across the join, against the flat list.
    ///
    /// The straddling window is the one that matters and the one an end-to-end test would miss:
    /// it draws a plausible list with rows missing from the middle, which no assertion about the
    /// first or last row can see. A virtualized tree produces one on every scroll past the last
    /// project file.
    #[test]
    fn compose_serves_every_window_across_the_seam() {
        const FIRST: &[&str] = &["a", "b", "c"];
        const SECOND: &[&str] = &["External Libraries", "serde", "anyhow"];
        let all: Vec<&str> = FIRST.iter().chain(SECOND).copied().collect();

        for offset in 0..=all.len() + 2 {
            for len in 0..=all.len() + 2 {
                let window: Vec<String> =
                    compose(FIRST.len(), offset, len, source(FIRST), source(SECOND))
                        .into_iter()
                        .map(|row| row.name)
                        .collect();
                let start = offset.min(all.len());
                let end = (offset + len).min(all.len());
                assert_eq!(window, all[start..end], "offset {offset} len {len}");
            }
        }
    }

    /// With nothing past the seam — every project that has no Cargo or Go manifest — the
    /// composed answer has to be byte-identical to the index's own.
    ///
    /// The group source *is* still asked, and it has to be: `compose` cannot know it is empty
    /// without asking, and short-circuiting on a count read a moment earlier would be a second
    /// opinion about the same lock. The cost is one read-lock over an empty `Vec` per window
    /// request, and what the assertion pins is the offset it is asked with — `0`, because the
    /// window never reached the seam.
    #[test]
    fn a_project_with_no_groups_is_served_entirely_by_the_index() {
        const FIRST: &[&str] = &["a", "b", "c"];
        let asked = std::cell::Cell::new(None);
        let rows = compose(FIRST.len(), 0, 10, source(FIRST), |offset, len| {
            asked.set(Some((offset, len)));
            Vec::new()
        });
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            FIRST
        );
        assert_eq!(asked.get(), Some((0, 7)));
    }

    /// The composition is over `Groups` in the app, so the same sweep is run against the real
    /// one — the fake above proves the arithmetic, this proves the two agree about `count`.
    #[test]
    fn the_real_group_tree_agrees_with_its_own_count() {
        let mut groups = Groups::new();
        groups.show("externalLibraries", "External Libraries");
        groups.expand(&group_path("externalLibraries"));
        groups.fulfil(
            "externalLibraries",
            vec![
                Entry::note("Resolving dependencies…"),
                Entry::note("and another"),
            ],
            Some("2".into()),
        );

        let walked = 4usize;
        let total = walked + groups.count();
        for offset in 0..=total + 1 {
            for len in 0..=total + 1 {
                let rows = compose(
                    walked,
                    offset,
                    len,
                    source(&["a", "b", "c", "d"]),
                    |o, l| groups.rows(o, l),
                );
                let expected = len.min(total.saturating_sub(offset));
                assert_eq!(rows.len(), expected, "offset {offset} len {len}");
            }
        }
    }
}

/// The *Scratches* group, through the real command layer.
///
/// Not `#[ignore]`d, unlike its neighbour below: a scratch drawer needs no toolchain, no
/// network and no registry — it is a `read_dir` of a directory this test makes itself. So the
/// half of M13 that a user touches most often is covered by `cargo test` rather than by a
/// deliberate run, which matters because every mutation path here (create, rename, delete,
/// paste) has to re-list a group **nothing watches**, and a path that forgets shows the user a
/// file that is no longer there.
#[cfg(test)]
mod scratch_tests {
    use super::*;
    use crate::scratches::GROUP_ID;
    use cide_fs::groups::group_path;
    use cide_ipc::TreeRowKind;
    use std::path::Path;

    struct Silent;
    impl FsEvents for Silent {
        fn status(&self, _: ProjectId, _: &cide_ipc::FsStatus) {}
        fn changed(&self, _: ProjectId, _: &cide_ipc::FsChange) {}
    }

    /// A project of one root under a temporary directory, walked through the real command
    /// layer, plus the drawer its primary root keys.
    async fn project(tag: &str) -> (FsRegistry, ProjectId, Arc<crate::files::ProjectFs>, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("cide-scratchcmd-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("seed");

        let registry = FsRegistry::default();
        let events: Arc<dyn FsEvents> = Arc::new(Silent);
        let id = ProjectId::new();
        index_project(events, &registry, id, vec![root.clone()])
            .await
            .expect("the walk");
        let fs = registry.get(id).expect("an indexed project");
        (registry, id, fs, root)
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(cide_core::scratch::dir_for(root));
        let _ = std::fs::remove_file(cide_core::scratch::origin_path(root));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The rows the groups contribute, which start where the walked index ends.
    fn group_rows(fs: &crate::files::ProjectFs) -> Vec<cide_ipc::TreeRow> {
        fs.groups().rows(0, 4096)
    }

    /// The sequence a user performs: open a project, make a scratch, find it in the tree,
    /// edit it, rename it, throw it away.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_scratch_is_created_listed_saved_renamed_and_deleted() {
        let (_registry, _id, fs, root) = project("lifecycle").await;
        let walked = fs.with_index(|index| index.count());

        // 1. The group is there before any scratch is, and says what makes one. A project with
        //    no Cargo.toml has no *External Libraries* header, so this is the only group.
        prepare(&fs, None);
        assert_eq!(
            tree_count(&fs),
            walked + 1,
            "one header, collapsed, for a project that has never had a scratch"
        );
        let header = group_rows(&fs).remove(0);
        assert_eq!(header.kind, TreeRowKind::Group);
        assert_eq!(header.name, "Scratches");
        assert_eq!(header.path, group_path(GROUP_ID));
        assert_eq!(header.detail, None);
        assert!(
            !cide_core::scratch::dir_for(&root).exists(),
            "and opening the project wrote nothing to disk"
        );

        fs.groups().expand(&header.path);
        let rows = group_rows(&fs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].kind, TreeRowKind::Note);
        assert!(
            rows[1].name.contains("No scratch files yet"),
            "{:?}",
            rows[1].name
        );

        // 2. Create one. The row is there when the command answers — nothing watches the
        //    drawer, so "the watcher will catch up" is not available as an excuse.
        let created = fs.groups().new_scratch(&root, "rs").expect("scratch");
        assert_eq!(
            created.file_name().and_then(|n| n.to_str()),
            Some("scratch.rs")
        );
        let rows = group_rows(&fs);
        assert_eq!(rows.len(), 2, "the note is replaced, not appended to");
        assert_eq!(rows[1].kind, TreeRowKind::File);
        assert_eq!(rows[1].path, created);
        assert_eq!(rows[0].detail.as_deref(), Some("1"));

        // 3. It is outside every root — that is what makes it a scratch — and the containment
        //    check that guards the disk knows it anyway.
        assert!(
            ops::check_within(&fs.root_paths(), &created).is_err(),
            "a scratch is deliberately outside the project, or it would show in git status"
        );
        assert!(ops::check_within(&fs.writable_paths(), &created).is_ok());

        // 4. Saving. This is the one that makes it a scratch rather than a decoration: the
        //    editor's own path (`file_write`) has never had a containment check, and the file
        //    tree's (`fs_write_file`) now admits the drawer.
        cide_core::document::write(&created, "fn main() {}\n").expect("the editor's save");
        assert_eq!(
            std::fs::read_to_string(&created).expect("read"),
            "fn main() {}\n"
        );
        assert!(
            cide_core::toolchain::read_only_reason(&created, &fs.root_paths()).is_none(),
            "a scratch must not be caught by the dependency-cache read-only rule"
        );

        // 5. Renaming, **through the handler's own body** and not through `ops::rename`.
        //    That distinction is the point: what is being pinned is which list the containment
        //    check is given, and a test that called `ops::check_within` itself would agree with
        //    whatever list it chose. Renaming also changes the language, because the extension
        //    is the whole of how `ui/src/editor/languages.ts` decides.
        let renamed = created.with_file_name("notes.md");
        assert!(
            rename_entry(&fs, &created, &renamed).expect("a scratch is renamed"),
            "…and the drawer says it changed, which is what becomes a cide://fs-status so the \
             second window redraws — nothing watches this directory"
        );
        assert_eq!(group_rows(&fs)[1].name, "notes.md");
        // A rename *out of* the drawer into the project is deliberately **allowed** — both
        // sides are in the writable set, and "this scratch turned out to be worth keeping" is a
        // real thing to want. Not asserted here as a success, because `ops::rename` is one
        // `rename(2)` and the drawer is under `$XDG_STATE_HOME` while this test's project is
        // under `/tmp`, which on a normal machine is a different filesystem: the assertion
        // would pass or fail on the device layout rather than on the rule.

        // 6. And the drawer itself is refused, exactly as a project root is: selecting the
        //    header and pressing Delete must not put a directory in the trash.
        let drawer = cide_core::scratch::dir_for(&root);
        assert!(delete_entries(&fs, std::slice::from_ref(&drawer)).is_err());
        assert!(drawer.is_dir(), "and nothing moved");

        // 7. Deleting the last one puts the note back — again through the handler's body, so
        //    the containment list and the re-listing are both the shipped ones.
        let (trashed, moved) =
            delete_entries(&fs, std::slice::from_ref(&renamed)).expect("a scratch is trashed");
        assert_eq!(trashed.len(), 1);
        assert!(moved, "and the drawer changed");
        assert!(!renamed.exists());
        let rows = group_rows(&fs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].kind, TreeRowKind::Note);
        assert_eq!(rows[0].detail, None);

        // 8. The negative half, which is the one that must not rot. Widening containment for
        //    the drawer must not have widened it for anything else.
        assert!(
            delete_entries(&fs, &[PathBuf::from("/etc/passwd")]).is_err(),
            "/etc/passwd is not in this project, its drawer, or anywhere else cide may write"
        );
        assert!(rename_entry(&fs, Path::new("/etc/passwd"), Path::new("/etc/passwd.bak")).is_err());

        cleanup(&root);
    }

    /// *Select Opened File* over a scratch: the reveal has to find a row for a path that is in
    /// no index and under no root, and it has to answer the **composed** index.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_scratch_is_revealed_at_its_row_in_the_composed_tree() {
        let (_registry, _id, fs, root) = project("reveal").await;
        prepare(&fs, None);
        let walked = fs.with_index(|index| index.count());

        let a = fs.groups().new_scratch(&root, "rs").expect("first");
        let b = fs.groups().new_scratch(&root, "rs").expect("second");

        let at = reveal_path(&fs, &b).expect("a scratch has a row");
        assert_eq!(
            at as usize,
            walked + 2,
            "the walked rows, then the header, then scratch.rs, then scratch_1.rs"
        );
        let rows = fs.groups().rows(at as usize - walked, 1);
        assert_eq!(rows[0].path, b);
        assert!(
            fs.groups().rows(0, 1)[0].expanded,
            "revealing into a collapsed group has to open it, or the row is not on screen"
        );

        assert_eq!(
            reveal_path(&fs, &a).expect("and the other one"),
            (walked + 1) as u32
        );
        let main = reveal_path(&fs, &root.join("src/main.rs"))
            .expect("and an ordinary project file still has a row");
        // Compared against the count *after* the reveal: `Index::reveal` expands `src` on the
        // way, which is the whole reason it is `with_index_mut`, so the pre-reveal count would
        // be the wrong side of the seam by exactly the row it just unfolded.
        assert!(
            (main as usize) < fs.with_index(|index| index.count()),
            "which is the *index's* answer and not the group's: {main} is past the walked rows"
        );
        assert_eq!(
            fs.with_index(|index| index.rows(main as usize, 1))[0].name,
            "main.rs"
        );
        assert_eq!(
            reveal_path(&fs, &root.join("src/never-existed.rs")),
            None,
            "a path in neither is honestly nothing, which is what the caller reports"
        );

        cleanup(&root);
    }

    /// The paths a mutating handler is allowed inside, which is also what the frontend greys
    /// its menu items from — `fs_writable_roots` answers this exact list.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_writable_set_is_the_roots_plus_the_drawer_and_nothing_else() {
        let (_registry, _id, fs, root) = project("writable").await;
        prepare(&fs, None);

        assert_eq!(
            fs.writable_paths(),
            vec![root.clone(), cide_core::scratch::dir_for(&root)],
            "the roots first, so a project path is still matched by the cheapest test"
        );
        // The negative half, and it is the one that must not rot: widening containment for
        // scratches must not widen it for anything else.
        for outside in ["/etc/passwd", "/tmp", "/"] {
            assert!(
                ops::check_within(&fs.writable_paths(), Path::new(outside)).is_err(),
                "{outside} must still be refused"
            );
        }
        let caches = cide_core::toolchain::dependency_roots();
        if let Some(cache) = caches.first() {
            assert!(
                ops::check_within(&fs.writable_paths(), &cache.join("serde-1.0/src/lib.rs"))
                    .is_err(),
                "a dependency source is readable, never writable — that group contributes no \
                 writable directory at all"
            );
        }

        cleanup(&root);
    }
}

/// The whole *External Libraries* feature, over this repository, through the real command layer.
///
/// `#[ignore]`d like every other test in this workspace that spawns a real binary: it needs
/// `cargo` on PATH, an authenticated toolchain and a populated registry, and it spends a quarter
/// of a second of somebody's CPU. Run it deliberately —
/// `cargo test -p cide-app external_libraries -- --ignored` — after touching either the resolver
/// or the composition, because everything else in this file exercises the two halves separately.
///
/// What it pins is the sequence a user actually performs, in the order they perform it: open a
/// project, look at the tree, expand the header, watch the rows arrive — and then the one that
/// does not start from the tree at all, *Select Opened File* over a tab Go to definition opened.
#[cfg(test)]
mod external_libraries_tests {
    use super::*;
    use crate::libraries::GROUP_ID;
    use cide_fs::groups::group_path;
    use cide_ipc::TreeRowKind;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Default)]
    struct Counting(AtomicU32);
    impl FsEvents for Counting {
        fn status(&self, _: ProjectId, _: &cide_ipc::FsStatus) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn changed(&self, _: ProjectId, _: &cide_ipc::FsChange) {}
    }

    fn workspace_root() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the workspace root")
            .to_path_buf()
    }

    async fn indexed(root: PathBuf) -> (FsRegistry, ProjectId, Arc<crate::files::ProjectFs>) {
        let registry = FsRegistry::default();
        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());
        let project = ProjectId::new();
        index_project(events, &registry, project, vec![root])
            .await
            .expect("the walk");
        let fs = registry.get(project).expect("an indexed project");
        (registry, project, fs)
    }

    /// The rows of the *External Libraries* group alone. It is drawn first, so its rows run
    /// until the next depth-0 row — which is the *Scratches* header.
    fn library_rows(fs: &crate::files::ProjectFs) -> Vec<cide_ipc::TreeRow> {
        let all = fs.groups().rows(0, 8192);
        let mut out = Vec::new();
        for row in all {
            if out.is_empty() {
                assert_eq!(row.name, "External Libraries", "the group is drawn first");
                out.push(row);
                continue;
            }
            if row.depth == 0 {
                break;
            }
            out.push(row);
        }
        out
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "spawns the real cargo against this repository"]
    async fn this_repository_grows_a_group_and_fills_it_in() {
        let root = workspace_root();
        let (_registry, project, fs) = indexed(root.clone()).await;
        let events: Arc<dyn FsEvents> = Arc::new(Counting::default());

        // 1. Opening a project resolves nothing. The header exists after the first tree read —
        //    which is `prepare` — and is collapsed, unresolved and one row. *Scratches* is the
        //    second header and is why this is `+ 2` rather than `+ 1`.
        prepare(&fs, Some((&events, project)));
        let walked = fs.with_index(|index| index.count());
        assert_eq!(
            tree_count(&fs),
            walked + 2,
            "each group contributes exactly one row before anybody opens it"
        );
        let header = library_rows(&fs).remove(0);
        assert_eq!(header.kind, TreeRowKind::Group);
        assert_eq!(header.name, "External Libraries");
        assert!(!header.expanded);
        assert_eq!(
            header.detail, None,
            "no count until there is something to count"
        );

        // 2. The header is the first row past the walked ones, and the seam serves it.
        let last = compose(
            walked,
            walked,
            1,
            |o, l| fs.with_index(|index| index.rows(o, l)),
            |o, l| fs.groups().rows(o, l),
        );
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].path, group_path(GROUP_ID));

        // 3. Expanding starts the resolver and returns at once, with a row already up saying so.
        let path = group_path(GROUP_ID);
        assert_eq!(
            fs.groups().expand(&path),
            Some(cide_fs::groups::Expanded::Resolve)
        );
        crate::libraries::spawn_resolve(Arc::clone(&fs), Arc::clone(&events), project);
        assert_eq!(
            tree_count(&fs),
            walked + 3,
            "the two headers plus the row that says one of them is working"
        );

        // 4. And the rows arrive. Polled rather than joined: `spawn_resolve` is deliberately
        //    fire-and-forget, which is the whole reason the click does not block.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while tree_count(&fs) <= walked + 3 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let rows = library_rows(&fs);
        assert!(
            rows.len() > 100,
            "cide has hundreds of dependencies, not {}: {:?}",
            rows.len(),
            rows.iter().take(4).collect::<Vec<_>>()
        );
        assert!(
            rows[0].detail.is_some(),
            "the header carries the count once there is one"
        );
        assert!(
            rows.iter().skip(1).all(|r| r.kind == TreeRowKind::Dir),
            "every row cargo reported has a directory to open"
        );
        let serde = rows
            .iter()
            .find(|r| r.name == "serde")
            .expect("cide depends on serde");
        assert!(
            serde
                .detail
                .as_deref()
                .is_some_and(|d: &str| d.starts_with('1')),
            "{:?}",
            serde.detail
        );

        // 5. Reveal into a package that has never been expanded — the *Select Opened File* path.
        let target = serde.path.join("src/lib.rs");
        let row = fs
            .groups()
            .reveal(&target)
            .expect("a chain nobody opened is materialised on demand");
        assert_eq!(fs.groups().rows(row, 1)[0].path, target);
        assert_eq!(
            fs.with_index_mut(|index| index.reveal(&target)),
            None,
            "the walked index cannot hold a path outside its roots — that is the whole reason \
             the group is a second tree"
        );

        // 6. And it is read-only, whatever its mode bits say.
        assert!(
            cide_core::toolchain::read_only_reason(&target, &[root]).is_some(),
            "a registry source opened from this group must not be writable: {}",
            target.display()
        );
    }

    /// *Select Opened File* on a tab Go to definition opened, with the group **never expanded**.
    ///
    /// This is the case the feature is most likely to be shipped without. Ctrl+B into `serde`
    /// gives a tab on a path that is in no index and under no root; the group that would hold it
    /// has never been opened, so it holds no packages, so a reveal finds nothing — and the
    /// command tells the user the file is *not in this project's file tree*, about a file they
    /// are looking at. `reveal_path` resolves the group rather than saying that.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "spawns the real cargo against this repository"]
    async fn revealing_a_dependency_source_resolves_the_group_it_needs() {
        let root = workspace_root();
        let (_registry, _project, fs) = indexed(root.clone()).await;
        prepare(&fs, None);
        assert_eq!(
            fs.groups().expand(&group_path(GROUP_ID)),
            Some(cide_fs::groups::Expanded::Resolve),
            "nothing has resolved this group, which is the state this test is about"
        );
        fs.groups().collapse(&group_path(GROUP_ID));

        // A real registry source, found the way Go to definition finds one: from the resolver.
        let cache = cide_core::toolchain::dependency_roots()
            .into_iter()
            .find(|root| root.ends_with("registry/src"))
            .expect("a cargo registry");
        let serde = std::fs::read_dir(&cache)
            .expect("registry index directories")
            .flatten()
            .flat_map(|index| {
                std::fs::read_dir(index.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .find(|entry| entry.file_name().to_string_lossy().starts_with("serde-1."))
            .expect("an unpacked serde")
            .path();
        let target = serde.join("src/lib.rs");
        assert!(target.is_file(), "{} is not unpacked", target.display());

        let walked = fs.with_index(|index| index.count());
        let at = reveal_path(&fs, &target).expect(
            "a dependency source has a row, and finding it must not depend on the user having \
             opened the group first",
        );
        assert!(at as usize >= walked, "the row is past the walked index");
        assert_eq!(fs.groups().rows(at as usize - walked, 1)[0].path, target);
    }
}
