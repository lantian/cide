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
    branch, changelist, commit as git_commit, diff, patch, push, shelf, stage, stash, status,
    tree_status,
};
use cide_ipc::git::{
    BranchInfo, BranchList, ChangesTree, CheckoutMode, CheckoutOutcome, CommitOutcome,
    CommitRequest, DiffSide, FetchOutcome, FileDiff, GitError, PathSelection, PushOutcome,
    RepoInfo, ShelfEntry, StashEntry, TreeStatusMap,
};
use cide_ipc::{ProjectId, RepoId, ToolTabId};
use tauri::State;

use crate::cmd::log::LogRegistry;
use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, GitError>;

/// The proxy environment cide's own `git` children are spawned with.
///
/// Reads `ProxyScope::git`, whose default is `Untouched` — an empty [`ProxyEnv`], meaning the
/// forked `git` inherits cide's environment exactly as it always has. That default is not
/// timidity: a corporate laptop whose `git push` works today works because cide's own
/// environment carries the proxy, and either of the other two answers would change that
/// silently for someone.
///
/// Read here rather than inside `cide-git`, which takes no configuration and must not learn
/// what a `WorkspaceState` is. Read *outside* [`blocking`] for the usual reason: the
/// workspace lock is `parking_lot` and must not be held across an await.
fn git_proxy(state: &WorkspaceState) -> cide_core::proxy::ProxyEnv {
    let proxy = state.with(|ws| ws.settings.proxy.clone());
    cide_core::proxy::ProxyEnv::for_target(&proxy, proxy.scope.git)
}

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

/// Which repositories a project actually contains, roots before their submodules.
///
/// The cheapest git question there is, and the reason it exists as its own command: it is
/// `Repository::discover` per root plus a `submodules()` walk — a handful of `stat`s — where
/// every other read in this file walks a working tree. A caller that only needs to know
/// *whether* there is a repository, or *which* ids to fan a push out over, was previously
/// paying for `git_status`.
///
/// It exists at all because the frontend used to answer this from `ProjectRoot::repo`, a field
/// `cide-core` documents as permanently `None` — see `cide_core::workspace::project_root`. That
/// made `reposOf` return an empty list for every project ever opened, which made the `repoOpen`
/// context flag permanently false, which filtered every git command out of the palette and made
/// three of their handlers bail before doing anything. The answer to "which repositories" is a
/// fact about the disk and belongs on this side of the wire, next to the discovery every other
/// git handler already uses; see `repo_root` above for the same argument about submodules.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_repos(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<Vec<RepoInfo>> {
    let roots = roots(&state, project)?;
    blocking(move || Ok(cide_git::repo::discover(&roots))).await
}

/// Which repository an absolute path belongs to, and its path inside it. (M18)
///
/// `None` — **not** an error — for a path no repository in this project contains: a scratch file,
/// a `~/.cargo/registry` source opened by go-to-definition, a tab left over from another project.
/// That is the answer that tells the caller to hide the blame gutter and grey the History item,
/// and making it an error would turn "this file has no history" into a failure the user cannot
/// act on.
///
/// The one seam between the two ways this app spells a path: the editor, the file tree and the
/// tab strip all hold absolute paths, and everything in `cide_ipc::git` and `cide_ipc::history`
/// speaks repo-relative ones. In Rust rather than as a prefix comparison in the webview, for the
/// reason `git_tree_status` gives about joining them: `canonical` resolves symlinks, the frontend
/// cannot, and a symlinked root compares unequal to the work tree it is actually inside. The
/// innermost repository wins, so a file in a submodule resolves to the submodule.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_locate(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    path: PathBuf,
) -> Result<Option<cide_ipc::git::RepoPath>> {
    let roots = roots(&state, project)?;
    blocking(move || Ok(cide_git::repo::locate(&roots, &path))).await
}

/// One page of one repository's log — and, with `query.path` set, one file's history. (M18)
///
/// One command rather than two, because the Log tab and a History tab differ by exactly one
/// field of [`LogQuery`]. The scope travels *inside* the query rather than beside it so there is
/// one source of truth for "which repositories", and so the resume token's identity hash can
/// cover it: a token replayed against a different question has to be refused rather than
/// silently answering the wrong one.
///
/// Does not broadcast. A log is a read, and the panel refreshes off the `cide://git-status` that
/// every mutation already emits, plus `FsChange.git` for a `git commit` run in a bash pane.
///
/// # `tab`, and why the walk goes through a registry
///
/// The one argument that is not part of the question. It names the tool tab the page is for, and
/// it exists so that the walk can be **stopped**: `cmd::log::LogRegistry` keys a cancellation flag
/// by `(project, tab)`, `cide_git::log::log_cancellable` polls it once per commit, and
/// `git_log_cancel` sets it. Per *tab* and not per project because the Log tab and N History tabs
/// are live at once in the same panel — see that module's header, which argues the whole shape.
///
/// A superseded page comes back `Ok` with [`cide_ipc::history::CommitPage::cancelled`] and the
/// rows it had, never as an error. Typing into the filter box produces one of these per keystroke,
/// and the caller's job is to drop it rather than to draw a failure.
///
/// The job is claimed **here**, on the caller's thread, and not inside [`blocking`]: a
/// `git_log_cancel` racing this has to find the job it is meant to stop, and a job created on the
/// pool would not exist yet for the moments the pool takes to pick the work up — which are exactly
/// the moments a busy pool has.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_log(
    state: State<'_, WorkspaceState>,
    logs: State<'_, LogRegistry>,
    project: ProjectId,
    tab: ToolTabId,
    query: cide_ipc::history::LogQuery,
) -> Result<cide_ipc::history::CommitPage> {
    let roots = roots(&state, project)?;
    let job = logs.job_for(project, tab, &query);
    // A second handle for the worker. The `State` guard cannot cross into `spawn_blocking` — it
    // borrows the app — and the walk has to be able to read the flag after this frame is gone,
    // so what moves is an `Arc`, not a reference.
    let walking = std::sync::Arc::clone(&job);
    let page = blocking(move || {
        // Resolved here rather than in `cide-git`, which knows nothing about projects: the scope
        // names `RepoId`s and only discovery can turn those into work trees.
        let all = cide_git::repo::discover(&roots);
        let wanted: Vec<cide_ipc::git::RepoInfo> = match &query.scope {
            cide_ipc::history::LogScope::One { repo } => {
                all.into_iter().filter(|i| i.id == *repo).collect()
            }
            cide_ipc::history::LogScope::Merged { repos } if repos.is_empty() => all,
            cide_ipc::history::LogScope::Merged { repos } => {
                all.into_iter().filter(|i| repos.contains(&i.id)).collect()
            }
        };
        // An empty scope is "this project has no repository at all", which every caller already
        // handles through `git_repos` — reporting the named one is the useful half, and for a
        // merged scope there is no single id to name, so the first requested one stands in.
        if wanted.is_empty() {
            let named = match &query.scope {
                cide_ipc::history::LogScope::One { repo } => Some(*repo),
                cide_ipc::history::LogScope::Merged { repos } => repos.first().copied(),
            };
            return match named {
                Some(repo) => Err(GitError::NoSuchRepo { repo }),
                None => Err(GitError::NoSuchProject {
                    project: project.to_string(),
                }),
            };
        }
        cide_git::log::log_cancellable(&cide_git::log::LogWalk {
            repos: &wanted,
            query: &query,
            // Borrowed for the length of this walk and never stored on the other side; the
            // crate holds no state, which is why the flag has to come from here.
            cancel: walking.cancel_flag(),
        })
    })
    .await;
    // After the walk, whatever it answered — an error is still a walk that is no longer running,
    // and leaving its job in the map would make the *next* request's `job_for` cancel a job that
    // had already stopped and, worse, would hold the query's resume token for ever. Guarded by
    // `Arc::ptr_eq` inside, so a newer request that replaced this job keeps its own flag.
    logs.finish(project, tab, &job);
    page
}

/// One commit: its full message, and the files it changed against `parent`.
///
/// `parent` selects which parent a merge is diffed against; `None` is the first, which is
/// `git show --first-parent`'s default and IDEA's. A root commit is diffed against the empty
/// tree, and [`CommitDetail::against`] says which of those happened rather than leaving the pane
/// to guess.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_commit_detail(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    rev: String,
    parent: Option<u32>,
) -> Result<cide_ipc::history::CommitDetail> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::show::detail(&root, &rev, parent, cide_git::show::DetailLimits::default())
    })
    .await
}

/// The `+`/`-` counts a capped [`git_commit_detail`] left uncounted.
///
/// Separate and on demand because the cap exists: past `show::MAX_COUNT_FILES` deltas the detail
/// generates **no patch at all**, so a four-thousand-file merge returns in the time of one
/// tree-to-tree diff. This is the button that pays for the rest, and it carries a wall-clock
/// deadline because there is no cancellation — a user who navigates away leaves it running.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_commit_line_counts(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    rev: String,
    parent: Option<u32>,
    paths: Vec<String>,
) -> Result<cide_ipc::history::CommitLineCounts> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::show::line_counts(
            &root,
            &rev,
            parent,
            &paths,
            cide_git::show::DetailLimits::default(),
        )
    })
    .await
}

/// One file's diff between two revisions. (M18)
///
/// `new`/`old` are [`RevSide`]s rather than a pair of strings, because there are three things a
/// side can be and only two of them are a commit: `FirstParent` is what "show me this commit"
/// means and is legal only as `old`, and `WorkingTree` is legal only as `new` — a diff whose
/// *old* side moves under it is not a diff of anything.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_diff_revision(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    new: cide_ipc::history::RevSide,
    old: cide_ipc::history::RevSide,
) -> Result<cide_ipc::history::RevisionDiff> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::revision::revision_diff(&root, &path, &new, &old)
    })
    .await
}

/// The changed-file list for an arbitrary pair of revisions.
///
/// Not a [`ChangesTree`]: that one is about the working tree and carries an index state, a
/// staged flag and a changelist, none of which a pair of commits has. The first reader of
/// `entry.staged` off one would get `false` and believe it.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_diff_revision_files(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    new: cide_ipc::history::RevSide,
    old: cide_ipc::history::RevSide,
) -> Result<cide_ipc::history::RevisionRange> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::revision::range_files(&root, &new, &old)
    })
    .await
}

/// One file's bytes as one commit left them — what a `TabKind::Revision` pane mounts with.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_file_at_revision(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    rev: String,
) -> Result<cide_ipc::history::RevisionBlob> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::revision::file_at_revision(&root, &path, &rev)
    })
    .await
}

/// Resolve anything `git rev-parse` accepts, so a name the user typed becomes an oid **before**
/// it is persisted into a tab.
///
/// A tab that stored `main` would name a different tree tomorrow, which is precisely the
/// staleness `DiffSpec` refuses to carry. The picker calls this and stores what it resolved to.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_resolve_rev(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    spec: String,
) -> Result<cide_ipc::history::ResolvedRev> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::revision::resolve_rev(&root, &spec)
    })
    .await
}

/// Annotate one file, line by line. (M18)
///
/// `contents` is the editor's buffer and must be sent **only when the tab is dirty**. Passing it
/// costs a copy of the file over the IPC wire; omitting it on a dirty tab is worse than stale —
/// every line after an unsaved insertion is attributed to the commit that wrote whatever used to
/// be there, and in a gutter sitting flush against live text that is a per-line falsehood rather
/// than an out-of-date view. `cide_git::blame` reads the working file when this is `None` and
/// falls back to HEAD when there is none, and says which it did in `BlameFile::source`.
///
/// Not cancellable, and the size cap is why. libgit2 exposes no hook inside `git_blame_file` —
/// no callback, no progress, nothing to poll — so a request already running cannot be stopped by
/// anything. `cide_git::blame::MAX_BLAME_BYTES` bounds the input instead, which is the honest
/// version of the same guarantee: the work is bounded even though it cannot be interrupted.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_blame(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    contents: Option<String>,
    request: cide_ipc::history::BlameRequest,
) -> Result<cide_ipc::history::BlameFile> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::blame::blame(
            &root,
            &path,
            contents.as_deref().map(str::as_bytes),
            &request,
        )
    })
    .await
}

/// The parent of `rev` *as it touched* `path` — what *Annotate previous revision* walks to.
///
/// Rust's answer and not a `rev^` the frontend builds. `rev^` is the first parent, which is the
/// wrong commit for a merge that took the file from its second parent, and it names the *new*
/// path across a rename — so the next blame would be of a file that does not exist at that commit
/// and would come back empty with no error at all.
///
/// `None` at a root commit and at the commit that introduced the file. That is the end of the
/// walk, not a failure, and the popup disables its button with that reason.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_blame_parent(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    rev: String,
) -> Result<Option<cide_ipc::history::BlameParent>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::blame::parent_of(&root, &path, &rev)
    })
    .await
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
    let proxy = git_proxy(&state);
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        push::push(
            &root,
            remote.as_deref(),
            refspec.as_deref(),
            set_upstream,
            &proxy,
        )
    })
    .await
}

// --- branches -----------------------------------------------------------------------------

/// Every repository's branches, in root order.
///
/// The whole project rather than one repository, because the selector is one control in a
/// status bar with one window's worth of room: in a superproject the branch the user means is
/// genuinely ambiguous, and the popup has to be able to name which repository each list
/// belongs to. One round trip for the whole popup, and one for the widget behind it.
///
/// A repository that cannot be read is skipped rather than failing the call — the same rule
/// `status::changes_tree` follows, for the same reason: three working repositories and one on
/// an unmounted share is still a usable list.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_branch_list(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<Vec<BranchList>> {
    let roots = roots(&state, project)?;
    blocking(move || Ok(lists(&roots))).await
}

fn lists(roots: &[PathBuf]) -> Vec<BranchList> {
    let mut out = Vec::new();
    for info in cide_git::repo::discover(roots) {
        match branch::list(&info) {
            Ok(list) => out.push(list),
            Err(error) => {
                tracing::warn!(root = %info.root.display(), %error, "skipping unreadable repository")
            }
        }
    }
    out
}

/// Create a branch, optionally switching to it.
///
/// **Never stashes and never overwrites.** When `checkout` is asked for and the start point is
/// not `HEAD`, the blockers are computed *before* the branch is created, so a refusal leaves
/// nothing behind — a create that succeeded followed by a switch that failed is a branch the
/// user did not ask for and now has to delete. Starting from `HEAD` cannot block: the tree is
/// the one already on disk.
///
/// To switch with a stash, call `git_branch_checkout` with a mode. Creating is not the gesture
/// that should be allowed to move a working tree around.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_branch_create(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    name: String,
    start_point: Option<String>,
    checkout: bool,
) -> Result<Vec<BranchList>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        if checkout && let Some(start) = start_point.as_deref() {
            let blocked = branch::checkout_blockers(&root, start)?;
            if !blocked.is_empty() {
                return Err(GitError::CheckoutWouldOverwrite {
                    branch: name.clone(),
                    paths: blocked,
                });
            }
        }
        branch::create(&root, &name, start_point.as_deref())?;
        if checkout {
            branch::checkout(&root, &name, CheckoutMode::Refuse)?;
        }
        let _ = refreshed(&app, &roots, project);
        Ok(lists(&roots))
    })
    .await
}

/// Switch branches.
///
/// The interesting outcome is the *refusal*: `CheckoutWouldOverwrite` carries the paths that
/// stand in the way, and the popup turns that list into the choice IDEA offers — stash, or
/// stash and bring them along. `mode` is what the second click sends back.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_branch_checkout(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    name: String,
    mode: CheckoutMode,
) -> Result<CheckoutOutcome> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = branch::checkout(&root, &name, mode)?;
        // A switch rewrites the working tree, so every open panel is looking at stale status.
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
    })
    .await
}

/// The paths a switch to `name` would overwrite, without switching.
///
/// Separate from the checkout so the popup can grey a row, or explain before the user commits
/// to anything. Empty means the switch is safe.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_branch_blockers(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    name: String,
) -> Result<Vec<String>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        branch::checkout_blockers(&root, &name)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn git_branch_rename(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    from: String,
    to: String,
) -> Result<Vec<BranchList>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        branch::rename(&root, &from, &to)?;
        let _ = refreshed(&app, &roots, project);
        Ok(lists(&roots))
    })
    .await
}

/// Delete a local branch. `force` is the answer to `GitError::BranchNotMerged`, and the UI is
/// expected to have said which commits would go before sending it.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_branch_delete(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    name: String,
    force: bool,
) -> Result<Vec<BranchList>> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        branch::delete(&root, &name, force)?;
        let _ = refreshed(&app, &roots, project);
        Ok(lists(&roots))
    })
    .await
}

/// `git fetch`. Network work, so it sits on the blocking pool for as long as the transport
/// takes — the same argument `git_push` makes.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_fetch(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    remote: Option<String>,
) -> Result<FetchOutcome> {
    let roots = roots(&state, project)?;
    let proxy = git_proxy(&state);
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = branch::fetch(&root, remote.as_deref(), &proxy)?;
        // Nothing in the working tree moved, but every branch row's behind-count did.
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
    })
    .await
}

/// Fetch, then fast-forward. Refuses a divergence rather than merging — see
/// `cide_git::branch::pull`.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_pull(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    remote: Option<String>,
) -> Result<FetchOutcome> {
    let roots = roots(&state, project)?;
    let proxy = git_proxy(&state);
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = branch::pull(&root, remote.as_deref(), &proxy)?;
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
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

// --- the commit actions: revert, cherry-pick, reset, tag, detach (M18) ------------------------

/*
 * These six are the log's right-click menu, and they are the reason `cide-git` grew
 * `replay.rs`, `reset.rs`, `tag.rs` and `branch::checkout_detached`. That code shipped with 33
 * differential tests against the real `git` binary and **nothing in the app could call it** —
 * no command, no client binding, no registry entry. This block is the seam that ends that.
 *
 * Three shapes worth naming, because each one differs from the block above it.
 *
 * **They answer with an *outcome*, not a [`ChangesTree`].** Every mutation further up returns
 * the tree because every one of them moves a tri-state checkbox in the commit panel and the
 * panel would otherwise ask a second time. These do not: the interesting result of a reset is
 * *what it dropped*, of a replay *which commit it made*, of a tag *whether it moved one that
 * already existed*. None of that survives being flattened into a status walk. The panel still
 * gets its tree — through the `refreshed` broadcast below, on `cide://git-status`, which is
 * the channel the watcher and every other window already listen on.
 *
 * **Every mutating one ends with `let _ = refreshed(&app, &roots, project);`.** A reset, a
 * revert, a cherry-pick and a detach all rewrite the working tree, so every open panel in
 * every window is looking at a `ChangesTree` describing a repository that no longer exists.
 * `git_tag_create` calls it too even though it moves no file: a tag changes the ref
 * decorations the log draws beside a row, and `cide://git-status` is what the log refreshes
 * off. `let _` and not `?`, for the reason `git_commit` states — the action succeeded, and
 * failing the call because the *re-read afterwards* failed would report a success as a
 * failure and leave the caller with no outcome at all.
 *
 * **`git_reset_preview` is a read and must not broadcast.** It is its own command rather than
 * a field of the reset request for the reason `cide_git::reset::preview`'s header gives: the
 * dialog's Hard row says *"DISCARD 3 changed files"* and **names all three**, and that
 * sentence has to exist on screen before the user commits to anything. Folding it into the
 * open of the dialog would mean either performing the reset to find out, or five round trips
 * that paint five times. It costs two tree diffs, a status walk and a revwalk bounded at
 * `DROPPED_COMMIT_CAP` — the same order as `git_branch_blockers`, which exists for exactly the
 * same reason on the branch side.
 *
 * **Two of the six are missing on purpose, and neither needs anything here.** *Amend* is
 * `git_commit` with [`CommitRequest::amend_of`] set to the row's oid: the field is on the wire,
 * `cide_git::commit::require_amend_head` checks it against the repository rather than against a
 * list drawn a second ago, and a mismatch is `GitError::NotHead`. *Branch from here* is
 * `git_branch_create` with the row's oid as `start_point`. A second command for either would be
 * a second implementation of a guard that already exists.
 */

/// Apply a commit's patch **inverted** onto `HEAD` — the log's *Revert*.
///
/// Not the Git panel's *Rollback*, which throws uncommitted work away; this one makes a new
/// commit (or, under [`ReplayMode::WorkingTree`], leaves the inverse in the working tree for
/// the user to look at first). The two words are IDEA's and the ambiguity is real, which is
/// part of why there is no bare *Revert* row in the command palette.
///
/// The interesting failures are all refusals computed **before** anything moves:
/// `ReplayWouldConflict` names the paths, `MergeNeedsMainline` carries the merge's parents so
/// the dialog can ask which side to keep, and `EmptyReplay` is git's own *"the previous
/// cherry-pick is now empty"*. See `cide_git::replay`'s header for why none of them leaves a
/// `REVERT_HEAD` behind.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_revert(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    request: cide_ipc::history::ReplayRequest,
) -> Result<cide_ipc::history::ReplayOutcome> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = cide_git::replay::revert(&root, &request)?;
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
    })
    .await
}

/// Apply a commit's patch **as it stands** onto `HEAD` — the log's *Cherry-pick*.
///
/// One command and not a `ReplayOp` argument on [`git_revert`], even though `cide-git` shares
/// one implementation behind them. A caller that passed the wrong enum variant would revert
/// where it meant to cherry-pick and there is no undo for that; two `#[tauri::command]`s make
/// the mistake unrepresentable on the wire, and cost one line each.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_cherry_pick(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    request: cide_ipc::history::ReplayRequest,
) -> Result<cide_ipc::history::ReplayOutcome> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = cide_git::replay::cherry_pick(&root, &request)?;
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
    })
    .await
}

/// What each kind of reset would discard. **Touches nothing**, and does not broadcast.
///
/// The one read in this block, and the confirmation dialog cannot be honest without it: it
/// carries the dropped commits (capped in Rust), the staged and dirty path lists that Mixed and
/// Hard respectively cost, whether `use_staging_area` is on — which is what makes a mixed reset
/// destroy a hand-built `git add -p` selection rather than merely a derived index — and
/// `commits_gained`, so a reset *forward* is described as what it is instead of as "0 commits
/// would be undone".
#[tauri::command(rename_all = "camelCase")]
pub async fn git_reset_preview(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    target: String,
) -> Result<cide_ipc::history::ResetPreview> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        cide_git::reset::preview(&root, &target)
    })
    .await
}

/// Move `HEAD` — and, per [`ResetKind`], the index and the working tree.
///
/// **A `hard` reset destroys uncommitted work.** This handler does not confirm, for
/// `git_rollback`'s reason: a confirmation the backend cannot show is not a safeguard. What it
/// does offer is the thing a confirmation cannot — [`ResetRequest::shelve_first`], which
/// captures the working tree into cide's own shelf *before* the reset and returns the entry, so
/// the toast can offer *Unshelve* without a second round trip.
///
/// `force` proceeds past the external-staging guard (Mixed and Hard take it; Soft is exempt
/// because it writes no index) and past a dirty tree. It deliberately does **not** proceed past
/// `operation_in_progress`, and there is no flag that does — see `cide_git::reset`'s header:
/// a `--hard` during a rebase leaves the todo list pointing at commits nothing can reach.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_reset(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    request: cide_ipc::history::ResetRequest,
) -> Result<cide_ipc::history::ResetOutcome> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = cide_git::reset::reset(&root, &request)?;
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
    })
    .await
}

/// Create — or, with [`TagRequest::force`], move — a tag.
///
/// Annotated when `request.message` is `Some`, lightweight when it is `None`; the distinction is
/// which field is present rather than a `bool` beside an optional message, because `git
/// describe` and most release tooling ignore lightweight tags and a release marked with the
/// wrong kind is one the build cannot name.
///
/// The refusal carries **where the tag points now**, not merely that the name is taken:
/// `TagExists { name, oid }` is what lets the dialog ask the question the user is actually
/// being asked, and `force` is the second click after reading the answer.
///
/// Broadcasts even though it moves no file — the log's ref decorations changed. It is also the
/// one action here allowed to run while an operation is in progress, which
/// `a_tag_can_still_be_created_during_a_rebase` pins on the `cide-git` side; pushing the tag is
/// deliberately not part of it.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_tag_create(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    request: cide_ipc::history::TagRequest,
) -> Result<cide_ipc::history::TagOutcome> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = cide_git::tag::create(&root, &request)?;
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
    })
    .await
}

/// Check out a commit, which necessarily detaches `HEAD`.
///
/// Separate from [`git_branch_checkout`] and answering [`DetachOutcome`] rather than
/// [`CheckoutOutcome`], because that type's `branch` field is documented as *the local branch
/// that is now checked out* and `chrome/branchModel.ts::checkoutNote` puts it straight into
/// *"Switched to {branch}"* — a short oid there reads as a branch name, and a detached `HEAD`
/// mistaken for a branch is how a day's commits end up on no ref at all. `DetachOutcome`
/// carries `previous` instead: the way back, which is the single most useful thing to know
/// while detached and the thing users least often work out for themselves.
///
/// `mode` is the same three-state answer a branch switch takes, and the first attempt is always
/// `Refuse`: the rejection carries the paths in the way, and `Stash` / `StashAndRestore` are
/// what the user's answer to that sends back.
#[tauri::command(rename_all = "camelCase")]
pub async fn git_checkout_detached(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    revision: String,
    mode: CheckoutMode,
) -> Result<cide_ipc::history::DetachOutcome> {
    let roots = roots(&state, project)?;
    blocking(move || {
        let root = repo_root(&roots, repo)?;
        let outcome = branch::checkout_detached(&root, &revision, mode)?;
        // The working tree is now some other commit's. Same reason as `git_branch_checkout`.
        let _ = refreshed(&app, &roots, project);
        Ok(outcome)
    })
    .await
}
