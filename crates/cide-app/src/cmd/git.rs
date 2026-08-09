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
fn repo_root(state: &WorkspaceState, project: ProjectId, repo: RepoId) -> Result<PathBuf> {
    let roots = roots(state, project)?;
    Ok(cide_git::repo::find(&roots, repo)?.root)
}

fn tree(state: &WorkspaceState, project: ProjectId, include_ignored: bool) -> Result<ChangesTree> {
    let roots = roots(state, project)?;
    status::changes_tree(&roots, status::StatusRequest { include_ignored })
}

/// Recompute the tree, broadcast it, and return it.
fn refreshed(
    app: &tauri::AppHandle,
    state: &WorkspaceState,
    project: ProjectId,
) -> Result<ChangesTree> {
    let tree = tree(state, project, false)?;
    crate::emit::git_status(app, project, &tree);
    Ok(tree)
}

// --- reading ------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub fn git_status(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    include_ignored: bool,
) -> Result<ChangesTree> {
    tree(&state, project, include_ignored)
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
#[tauri::command(rename_all = "camelCase")]
pub fn git_tree_status(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<TreeStatusMap> {
    Ok(tree_status::tree_status(&roots(&state, project)?))
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_branch_info(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<BranchInfo> {
    let root = repo_root(&state, project, repo)?;
    let handle = cide_git::repo::open(&root)?;
    status::branch_info(&handle)
}

/// One file's diff, on the side the caller names.
///
/// The `rev` on the returned [`FileDiff`] is what a later [`PathSelection`] carries back, so
/// a selection made against this diff is refused if the file has moved since.
#[tauri::command(rename_all = "camelCase")]
pub fn git_diff_file(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    side: DiffSide,
) -> Result<FileDiff> {
    let root = repo_root(&state, project, repo)?;
    let handle = cide_git::repo::open(&root)?;
    // Renames are looked for here and not during staging: the panel wants to *show* a rename,
    // and the staging path refuses one anyway.
    let request = diff::DiffRequest::new(side).renames(true);
    let file = diff::file_diff(&handle, &path, request)?
        .ok_or(GitError::NoSuchChange { path: path.clone() })?;
    Ok(diff::view(&file, side))
}

/// Which hunks and lines a selection resolves to, without applying it.
///
/// The panel uses this to draw a tri-state checkbox for a file whose selection came from
/// somewhere else — a changelist row, a "select all in this hunk" gesture — rather than
/// re-deriving the answer in TypeScript, where it would drift from the Rust that acts on it.
#[tauri::command(rename_all = "camelCase")]
pub fn git_resolve_selection(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    side: DiffSide,
    selection: PathSelection,
) -> Result<Vec<cide_ipc::git::LineRef>> {
    let root = repo_root(&state, project, repo)?;
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
}

// --- staging ------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub fn git_stage(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    selections: Vec<PathSelection>,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    stage::stage(&root, &selections)?;
    refreshed(&app, &state, project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_unstage(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    selections: Vec<PathSelection>,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    stage::unstage(&root, &selections)?;
    refreshed(&app, &state, project)
}

/// Throw the selected changes away. **This destroys uncommitted work** — the frontend is
/// expected to confirm first, and this handler does not, because a confirmation the backend
/// cannot show is not a safeguard.
#[tauri::command(rename_all = "camelCase")]
pub fn git_rollback(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    selections: Vec<PathSelection>,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    stage::rollback(&root, &selections)?;
    refreshed(&app, &state, project)
}

// --- commit and push ------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub fn git_commit(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    request: CommitRequest,
) -> Result<CommitOutcome> {
    let root = repo_root(&state, project, repo)?;
    let outcome = git_commit::commit(&root, &request)?;
    let _ = refreshed(&app, &state, project);
    Ok(outcome)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_push(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    remote: Option<String>,
    refspec: Option<String>,
    set_upstream: bool,
) -> Result<PushOutcome> {
    let root = repo_root(&state, project, repo)?;
    push::push(&root, remote.as_deref(), refspec.as_deref(), set_upstream)
}

// --- the external-staging guard ---------------------------------------------------------------

/// Accept the index as it now stands — the "reload" half of the guard bar.
///
/// The other half is `git_commit` with `force: true`, which overwrites. Both are explicit
/// gestures; neither is ever the default, which is the entire point of the guard.
#[tauri::command(rename_all = "camelCase")]
pub fn git_adopt_index(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    let handle = cide_git::repo::open(&root)?;
    changelist::record_index(&root, &handle)?;
    refreshed(&app, &state, project)
}

/// IDEA's "use Git staging area instead" toggle.
#[tauri::command(rename_all = "camelCase")]
pub fn git_set_use_staging_area(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    enabled: bool,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    changelist::update(&root, |data| {
        data.use_staging_area = enabled;
        Ok(())
    })?;
    refreshed(&app, &state, project)
}

// --- changelists ------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub fn git_changelist_create(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    name: String,
    comment: String,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    changelist::update(&root, |data| data.create(&name, &comment))?;
    refreshed(&app, &state, project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_changelist_rename(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
    name: String,
    comment: String,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    changelist::update(&root, |data| data.rename(&id, &name, &comment))?;
    refreshed(&app, &state, project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_changelist_delete(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    changelist::update(&root, |data| data.delete(&id))?;
    refreshed(&app, &state, project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_changelist_move_paths(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
    paths: Vec<String>,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    changelist::update(&root, |data| data.move_paths(&id, &paths))?;
    refreshed(&app, &state, project)
}

/// Make a changelist the one new changes land in.
///
/// The live path set is read first and passed in, because switching the active list has to
/// pin the *existing* unfiled changes to the outgoing list — otherwise every change the user
/// has not filed silently follows the switch.
#[tauri::command(rename_all = "camelCase")]
pub fn git_changelist_set_active(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    let live = status::live_paths(&root)?;
    changelist::update(&root, |data| data.set_active(&id, &live))?;
    refreshed(&app, &state, project)
}

// --- shelf ------------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub fn git_shelve(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    name: String,
    selections: Vec<PathSelection>,
) -> Result<ShelfEntry> {
    let root = repo_root(&state, project, repo)?;
    let entry = shelf::shelve(&root, &name, &selections)?;
    let _ = refreshed(&app, &state, project);
    Ok(entry)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_unshelve(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
    keep: bool,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    shelf::unshelve(&root, &id, keep)?;
    refreshed(&app, &state, project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_shelf_list(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<Vec<ShelfEntry>> {
    let root = repo_root(&state, project, repo)?;
    Ok(shelf::list(&root))
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_shelf_patch(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
) -> Result<String> {
    let root = repo_root(&state, project, repo)?;
    shelf::read_patch(&root, &id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_shelf_drop(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    id: String,
) -> Result<Vec<ShelfEntry>> {
    let root = repo_root(&state, project, repo)?;
    shelf::drop_entry(&root, &id)?;
    Ok(shelf::list(&root))
}

// --- stash --------------------------------------------------------------------------------------

#[tauri::command(rename_all = "camelCase")]
pub fn git_stash_save(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    message: String,
    include_untracked: bool,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    stash::save(&root, &message, include_untracked)?;
    refreshed(&app, &state, project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_stash_list(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<Vec<StashEntry>> {
    let root = repo_root(&state, project, repo)?;
    stash::list(&root)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_stash_pop(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    index: u32,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    stash::pop(&root, index)?;
    refreshed(&app, &state, project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_stash_apply(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    index: u32,
) -> Result<ChangesTree> {
    let root = repo_root(&state, project, repo)?;
    stash::apply(&root, index)?;
    refreshed(&app, &state, project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_stash_drop(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    index: u32,
) -> Result<Vec<StashEntry>> {
    let root = repo_root(&state, project, repo)?;
    stash::drop_entry(&root, index)?;
    stash::list(&root)
}
