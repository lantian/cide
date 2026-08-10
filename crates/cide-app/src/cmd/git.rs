//! Git commands.
//!
//! Thin by policy, like every other module here: resolve the project's roots, map a
//! [`RepoId`] to a work tree, call one `cide-git` function, hand the result back. Nothing in
//! this file knows what a hunk is.
//!
//! Two shapes are worth naming.
//!
//! **No cached repositories.** There is no `GitState` and nothing is managed. `cide-git` opens
//! what it needs per call and drops it, because a `Repository` handle held across calls goes
//! stale the moment anything outside cide touches the repo — and inside cide, a bash pane
//! always does. That decision lives in `cide-git`; the consequence here is that these handlers
//! need no state beyond the workspace.
//!
//! **Mutations answer with the new tree.** `stage`, `commit`, `rollback` and the changelist
//! operations return a fresh [`ChangesTree`] rather than an acknowledgement, and also
//! broadcast one. The panel is a tree of tri-state checkboxes whose every row can change when
//! one file moves; a round trip that returned `{ok: true}` would be followed immediately by a
//! second asking what happened, and the intermediate frame is a visibly wrong tree.
//!
//! **Nothing here runs on the main thread.** Every handler is `async` and hands its git work to
//! [`blocking`]; see that function for why `#[tauri::command(async)]` on its own is not enough.

use std::path::PathBuf;

use cide_git::{
    changelist, commit as git_commit, diff, patch, push, shelf, stage, stash, status, tree_status,
};
use cide_ipc::git::{
    BranchInfo, ChangesTree, CommitOutcome, CommitRequest, DiffSide, FileDiff, GitError,
    PathSelection, PushOutcome, ShelfEntry, StashEntry, TreeStatusMap,
};
use cide_ipc::{ProjectId, RepoId};
use tauri::State;

use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, GitError>;

/// Every root of an open project.
fn roots(state: &WorkspaceState, project: ProjectId) -> Result<Vec<PathBuf>> {
    state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .map(|p| p.roots.iter().map(|r| r.path.clone()).collect())
            .map_err(|_| GitError::NoSuchProject {
                project: project.to_string(),
            })
    })
}

/// The work tree of one repository inside a project.
///
/// Resolved through discovery rather than from `ProjectRoot::repo`, because a submodule has a
/// `RepoId` and no `ProjectRoot` — and a submodule's changes are exactly what a monorepo user
/// opens the panel for.
///
/// Takes the roots rather than the `WorkspaceState` so it can run inside [`blocking`]: reading
/// the workspace takes a lock that must not be held across an await, while *this* is the part
/// that touches the disk.
fn repo_root(roots: &[PathBuf], repo: RepoId) -> Result<PathBuf> {
    Ok(cide_git::repo::find(roots, repo)?.root)
}

fn tree(roots: &[PathBuf], include_ignored: bool) -> Result<ChangesTree> {
    status::changes_tree(roots, status::StatusRequest { include_ignored })
}

/// Recompute the tree, broadcast it, and return it.
fn refreshed(app: &tauri::AppHandle, roots: &[PathBuf], project: ProjectId) -> Result<ChangesTree> {
    let tree = tree(roots, false)?;
    crate::emit::git_status(app, project, &tree);
    Ok(tree)
}

/// Run git work on the blocking pool.
///
/// Tauri 2 polls a plain `#[tauri::command]` **on the main thread**, so every synchronous handler
/// in this file used to freeze the window for as long as libgit2 took — and `git_status` walks
/// every repository under every root while every mutation calls [`refreshed`], which walks them
/// all again. On a monorepo that is hundreds of milliseconds per click.
///
/// `#[tauri::command(async)]` is not the fix, and neither is an `async fn` whose body never
/// awaits: both only move the call onto the async runtime, where a blocking `git2` call still
/// occupies a runtime worker for its whole duration and starves every other task sharing it.
/// `spawn_blocking` is the pool built for exactly this. It also happens to be the only shape in
/// which a `git2::Repository` — which is neither `Send` nor `Sync` — can be opened and dropped
/// without ever crossing a suspension point.
///
/// The workspace lock is *not* taken in here. Callers resolve their roots first, on the caller's
/// thread, and move a plain `Vec<PathBuf>` in; a `State` guard held across the await would be
/// both un-`Send` and a lock held for the length of a git walk.
async fn blocking<T>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| GitError::Io {
            // A panic in libgit2's thread is a bug, not a git condition — but it has to reach
            // the panel as *something*, or the command hangs on a channel that never answers.
            detail: format!("the git worker did not finish: {error}"),
        })?
}

// --- reading ------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub async fn git_status(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    include_ignored: bool,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || tree(&roots, include_ignored)).await
}

/// Per-path status for the file tree, keyed by absolute path.
///
/// Separate from `fs_tree_rows` on purpose, and the separation is the requirement: the tree's
/// first paint must not wait on git. A repository with a slow `git status` shows an untagged
/// tree and then tags, never an empty pane — which is only possible while the rows and the
/// tags are two independent round trips.
///
/// There is no `paths` argument. The map is bounded by the number of *changed* paths rather
/// than by the size of the repository, so windowing the question would cost a round trip per
/// scroll tick and save nothing underneath; see `cide_git::tree_status` for the full
/// comparison. The frontend re-reads this on `cide://git-status` and on a `cide://fs-changed`
/// that touched a git file, not on scroll and not on a timer.
///
/// This one is the *watcher's* rather than a user gesture's — it can run several times a second
/// for as long as a build is writing files — which is why it was the first command in this file
/// to be taken off the main thread. It now shares [`blocking`] with the rest, which is a
/// strictly stronger guarantee: `command(async)` only moved the walk onto the async runtime,
/// where a blocking libgit2 call still holds a runtime worker for its whole duration.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_tree_status(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<TreeStatusMap> {
    let roots = roots(&state, project)?;
    blocking(move || Ok(tree_status::tree_status(&roots))).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_branch_info(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<BranchInfo> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let handle = cide_git::repo::open(&root)?;
        status::branch_info(&handle)
    })
    .await
}

/// One file's diff, on the side the caller names.
///
/// The `rev` on the returned [`FileDiff`] is what a later [`PathSelection`] carries back, so
/// a selection made against this diff is refused if the file has moved since.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_diff_file(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    side: DiffSide,
) -> Result<FileDiff> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let handle = cide_git::repo::open(&root)?;
        // Renames are asked for here and not during staging: the panel wants to *show* a rename,
        // and the staging path refuses one anyway.
        //
        // The flag does not currently do anything, and this is the honest record of that rather
        // than a silent dead call. `file_diff` passes a pathspec, libgit2 applies it while building
        // the diff, and `find_similar` then has only one side of the rename to look at — so a
        // renamed file comes back `Added`, with no `oldPath` and with `partialOk: true`. The panel
        // therefore offers per-line staging of a rename's new side. Nothing is corrupted (staging
        // re-diffs without renames and writes a valid whole-new-file patch), and the changed-files
        // list still labels the row `Renamed`, because `status.rs` detects renames by a different
        // mechanism. Fixing it means widening this diff to the whole repo before the similarity
        // pass, on a call the UI makes on every file click, which is a cost worth deciding on
        // deliberately. `cide-git/tests/patch_props.rs::a_rename_is_only_detected_when_both_sides_are_in_the_diff`
        // pins the mechanism and fails here when it changes.
        let request = diff::DiffRequest::new(side).renames(true);
        let file = diff::file_diff(&handle, &path, request)?
            .ok_or(GitError::NoSuchChange { path: path.clone() })?;
        Ok(diff::view(&file, side))
    })
    .await
}

/// Which hunks and lines a selection resolves to, without applying it.
///
/// The panel uses this to draw a tri-state checkbox for a file whose selection came from
/// somewhere else — a changelist row, a "select all in this hunk" gesture — rather than
/// re-deriving the answer in TypeScript, where it would drift from the Rust that acts on it.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_resolve_selection(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    side: DiffSide,
    selection: PathSelection,
) -> Result<Vec<cide_ipc::git::LineRef>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let handle = cide_git::repo::open(&root)?;
        let file = diff::file_diff(&handle, &selection.path, diff::DiffRequest::new(side))?.ok_or(
            GitError::NoSuchChange {
                path: selection.path.clone(),
            },
        )?;
        let chosen = patch::choose(&file, &selection.selection)?;
        Ok(chosen
            .lines
            .into_iter()
            .map(|(hunk, line)| cide_ipc::git::LineRef {
                hunk: hunk as u32,
                line: line as u32,
            })
            .collect())
    })
    .await
}

// --- staging ------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub async fn git_stage(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    selections: Vec<PathSelection>,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        stage::stage(&root, &selections)?;
        refreshed(&app, &roots, project)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_unstage(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    selections: Vec<PathSelection>,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        stage::unstage(&root, &selections)?;
        refreshed(&app, &roots, project)
    })
    .await
}

/// Throw the selected changes away. **This destroys uncommitted work** — the frontend is
/// expected to confirm first, and this handler does not, because a confirmation the backend
/// cannot show is not a safeguard.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_rollback(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    selections: Vec<PathSelection>,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        stage::rollback(&root, &selections)?;
        refreshed(&app, &roots, project)
    })
    .await
}

// --- commit and push ------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub async fn git_commit(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    request: CommitRequest,
) -> Result<CommitOutcome> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = git_commit::commit(&root, &request)?;
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
    })
    .await
}

/// `push` shells out to `git` when a credential helper is configured, so this can sit on a
/// network round trip for minutes. Doubly worth keeping off both the main thread and the
/// async runtime's workers.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_push(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    remote: Option<String>,
    refspec: Option<String>,
    set_upstream: bool,
) -> Result<PushOutcome> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        push::push(&root, remote.as_deref(), refspec.as_deref(), set_upstream)
    })
    .await
}

// --- the external-staging guard ---------------------------------------------------------------

/// Accept the index as it now stands — the "reload" half of the guard bar.
///
/// The other half is `git_commit` with `force: true`, which overwrites. Both are explicit
/// gestures; neither is ever the default, which is the entire point of the guard.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_adopt_index(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let handle = cide_git::repo::open(&root)?;
        changelist::record_index(&root, &handle)?;
        refreshed(&app, &roots, project)
    })
    .await
}

/// IDEA's "use Git staging area instead" toggle.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_set_use_staging_area(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    enabled: bool,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        changelist::update(&root, |data| {
            data.use_staging_area = enabled;
            Ok(())
        })?;
        refreshed(&app, &roots, project)
    })
    .await
}

// --- changelists ------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub async fn git_changelist_create(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    name: String,
    comment: String,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        changelist::update(&root, |data| data.create(&name, &comment))?;
        refreshed(&app, &roots, project)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_changelist_rename(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
    name: String,
    comment: String,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        changelist::update(&root, |data| data.rename(&id, &name, &comment))?;
        refreshed(&app, &roots, project)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_changelist_delete(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        changelist::update(&root, |data| data.delete(&id))?;
        refreshed(&app, &roots, project)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_changelist_move_paths(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
    paths: Vec<String>,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        changelist::update(&root, |data| data.move_paths(&id, &paths))?;
        refreshed(&app, &roots, project)
    })
    .await
}

/// Make a changelist the one new changes land in.
///
/// The live path set is read first and passed in, because switching the active list has to
/// pin the *existing* unfiled changes to the outgoing list — otherwise every change the user
/// has not filed silently follows the switch.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_changelist_set_active(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let live = status::live_paths(&root)?;
        changelist::update(&root, |data| data.set_active(&id, &live))?;
        refreshed(&app, &roots, project)
    })
    .await
}

// --- shelf ------------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub async fn git_shelve(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    name: String,
    selections: Vec<PathSelection>,
) -> Result<ShelfEntry> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let entry = shelf::shelve(&root, &name, &selections)?;
        let _ = refreshed(&app, &roots, project);
        Ok(entry)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_unshelve(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
    keep: bool,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        shelf::unshelve(&root, &id, keep)?;
        refreshed(&app, &roots, project)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_shelf_list(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<Vec<ShelfEntry>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        Ok(shelf::list(&root))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_shelf_patch(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
) -> Result<String> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        shelf::read_patch(&root, &id)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_shelf_drop(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
) -> Result<Vec<ShelfEntry>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        shelf::drop_entry(&root, &id)?;
        Ok(shelf::list(&root))
    })
    .await
}

// --- stash --------------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub async fn git_stash_save(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    message: String,
    include_untracked: bool,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        stash::save(&root, &message, include_untracked)?;
        refreshed(&app, &roots, project)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_stash_list(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<Vec<StashEntry>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        stash::list(&root)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_stash_pop(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    index: u32,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        stash::pop(&root, index)?;
        refreshed(&app, &roots, project)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_stash_apply(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    index: u32,
) -> Result<ChangesTree> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        stash::apply(&root, index)?;
        refreshed(&app, &roots, project)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_stash_drop(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    index: u32,
) -> Result<Vec<StashEntry>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        stash::drop_entry(&root, index)?;
        stash::list(&root)
    })
    .await
}
