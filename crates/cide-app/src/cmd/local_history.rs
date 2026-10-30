//! Local History's commands. Thin, as `cmd/` is: the rules are in `cide-local-history` and in
//! the worker, and these turn rows into wire types and paths into roots.
//!
//! Everything goes through `blocking`, including the reads. A read can wait behind a batch that
//! holds the store's lock while it deflates, and a blob is a file read: neither belongs on a
//! thread that draws the window.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cide_core::{CoreError, Result, workspace};
use cide_ipc::git::FileState;
use cide_ipc::history::{RevSide, RevisionDiff};
use cide_ipc::local_history::{
    LocalFileHistory, LocalHistoryGroup, LocalHistoryStatus, LocalRestoreFailure, LocalRestorePlan,
    LocalRestoreReport, LocalRestored, LocalSearchResult, LocalSide, LocalVersion,
};
use cide_ipc::{ProjectId, TabId, TabKind};
use cide_local_history::{RootId, TargetState, Version};
use tauri::{AppHandle, State};

use crate::local_history::{Capture, Registry, RestoreRequest, Restored, TreeRequest};
use crate::workspace_state::WorkspaceState;

/// The largest current file a diff will read. Versions are bounded by the size limit, and this
/// holds the other side to the same order: a diff of a 2 GB log is not something to draw.
const DIFF_LIMIT: u64 = 16 << 20;

/// Full text is sent for whole-file rendering only below this. Same purpose as the cap the git
/// diff has: past it the hunks are drawn and the folded regions stay folded.
const TEXT_LIMIT: usize = 2 << 20;

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tauri::async_runtime::spawn_blocking(job).await {
        Ok(result) => result,
        Err(error) => Err(CoreError::Io(format!("Local History failed: {error}"))),
    }
}

fn io(message: impl Into<String>) -> CoreError {
    CoreError::Io(message.into())
}

fn capture_of(history: &Registry, project: ProjectId) -> Result<Arc<Capture>> {
    history
        .capture(project)
        .ok_or_else(|| io("Local History is not available for this project."))
}

fn wire_version(capture: &Capture, version: Version) -> LocalVersion {
    let path = capture
        .path_of(version.root, &version.rel)
        .map_or_else(|| version.rel.clone(), |p| p.display().to_string());
    LocalVersion {
        id: version.id,
        ts: version.ts,
        group: version.group,
        path,
        kind: version.kind,
        source: version.source,
        size: version.size,
        kept: version.hash.is_some(),
        moved: version.moved,
        label: version.label,
    }
}

/// What a folder or project scope is, for the store: a root and a prefix.
///
/// `None` scope means the primary root, whole. A project with several roots has one timeline per
/// root as far as the store is concerned, and the tree's own rows are what reach the others.
fn scope_of(capture: &Capture, path: Option<&Path>) -> Result<(RootId, String)> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => capture.primary_root().to_path_buf(),
    };
    if let Some(found) = capture.root_scope(&path) {
        return Ok(found);
    }
    Err(io("This path is outside the project."))
}

#[tauri::command(rename_all = "camelCase")]
pub fn local_history_status(
    state: State<'_, WorkspaceState>,
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
) -> Result<LocalHistoryStatus> {
    let enabled = state.with(|ws| cide_core::local_history_settings::effective(ws, project));
    let empty = LocalHistoryStatus {
        enabled,
        loading: enabled,
        read_only: false,
        baseline_done: false,
        rows: 0,
        paths: 0,
        disk_bytes: 0,
    };
    let Some(capture) = history.capture(project) else {
        return Ok(empty);
    };
    let scope = scope_of(&capture, None).ok();
    Ok(capture
        .with_store(|store| {
            let stats = store.stats();
            LocalHistoryStatus {
                enabled,
                loading: false,
                read_only: !store.is_writable(),
                baseline_done: scope.is_some_and(|(root, _)| store.baseline_done(root)),
                rows: stats.rows as u64,
                paths: stats.paths as u64,
                disk_bytes: stats.disk_bytes,
            }
        })
        .unwrap_or(empty))
}

/// One file's versions, newest first, following cide's own renames back.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_file(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: String,
) -> Result<LocalFileHistory> {
    let history = Arc::clone(&history);
    blocking(move || {
        let path = PathBuf::from(path);
        let Some(capture) = history.capture(project) else {
            return Ok(LocalFileHistory {
                versions: Vec::new(),
                untracked: None,
            });
        };
        let untracked = capture.untracked_reason(&path).map(str::to_owned);
        let Some((root, rel)) = capture.locate(&path) else {
            return Ok(LocalFileHistory {
                versions: Vec::new(),
                untracked,
            });
        };
        let versions = capture
            .with_store(|store| store.versions(root, &rel))
            .unwrap_or_default();
        Ok(LocalFileHistory {
            versions: versions
                .into_iter()
                .map(|version| wire_version(&capture, version))
                .collect(),
            untracked,
        })
    })
    .await
}

/// A folder's or project's timeline, a page at a time. `before` is the last `group` already held.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_groups(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: Option<String>,
    before: Option<u64>,
    limit: u32,
) -> Result<Vec<LocalHistoryGroup>> {
    let history = Arc::clone(&history);
    blocking(move || {
        let Some(capture) = history.capture(project) else {
            return Ok(Vec::new());
        };
        let (root, prefix) = scope_of(&capture, path.as_deref().map(Path::new))?;
        let groups = capture
            .with_store(|store| store.groups(root, &prefix, before, limit as usize))
            .unwrap_or_default();
        Ok(groups
            .into_iter()
            .map(|group| LocalHistoryGroup {
                group: group.group,
                ts: group.ts,
                source: group.source,
                files: group.files,
                label: group.label,
                snapshot: group.snapshot,
            })
            .collect())
    })
    .await
}

/// The files in one group, under a scope.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_group_files(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: Option<String>,
    group: u64,
) -> Result<Vec<LocalVersion>> {
    let history = Arc::clone(&history);
    blocking(move || {
        let capture = capture_of(&history, project)?;
        let (root, prefix) = scope_of(&capture, path.as_deref().map(Path::new))?;
        let rows = capture
            .with_store(|store| store.group_rows(root, &prefix, group))
            .unwrap_or_default();
        Ok(rows
            .into_iter()
            .map(|version| wire_version(&capture, version))
            .collect())
    })
    .await
}

/// A side's bytes, and its Unix mode. An absent side is empty with mode zero, which is how
/// `RevisionDiff` spells "this side does not exist".
fn side_bytes(capture: &Capture, side: LocalSide, path: &Path) -> Result<(Vec<u8>, u32)> {
    match side {
        LocalSide::Version { id } => {
            let (handle, mode) = capture
                .with_store(|store| (store.blob(id), store.version(id).map_or(0, |v| v.mode)))
                .ok_or_else(|| io("Local History is still opening."))?;
            let Some(handle) = handle else {
                return Ok((Vec::new(), 0));
            };
            let bytes = handle.read().map_err(|error| io(error.to_string()))?;
            Ok((bytes, mode))
        }
        LocalSide::Current => {
            use std::os::unix::fs::MetadataExt as _;
            let Ok(meta) = std::fs::symlink_metadata(path) else {
                return Ok((Vec::new(), 0));
            };
            if !meta.is_file() {
                return Ok((Vec::new(), 0));
            }
            if meta.len() > DIFF_LIMIT {
                return Err(io("The current file is too large to compare."));
            }
            let bytes = std::fs::read(path).map_err(|error| io(error.to_string()))?;
            Ok((bytes, meta.mode()))
        }
    }
}

fn side_label(side: LocalSide) -> RevSide {
    match side {
        // `RevSide` is git's vocabulary, and a read-only `GitDiffView` draws neither of these: it
        // takes its labels from its `revisions` prop. So a version id in the `oid` slot is inert,
        // and adding a variant here would ripple through every match on a type that is about git.
        LocalSide::Version { id } => RevSide::Commit {
            oid: id.to_string(),
        },
        LocalSide::Current => RevSide::WorkingTree,
    }
}

fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// The diff between two sides of one file, for the read-only `GitDiffView`.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_diff(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: String,
    from: LocalSide,
    to: LocalSide,
) -> Result<RevisionDiff> {
    let history = Arc::clone(&history);
    blocking(move || {
        let capture = capture_of(&history, project)?;
        let path = PathBuf::from(path);
        let (old, old_mode) = side_bytes(&capture, from, &path)?;
        let (new, new_mode) = side_bytes(&capture, to, &path)?;
        let name = capture
            .locate(&path)
            .map_or_else(|| path.display().to_string(), |(_, rel)| rel);
        let rel = Path::new(&name);
        let text = is_text(&old) && is_text(&new);
        let hunks = if text {
            cide_git::diff::hunks_between(&old, rel, &new, rel)
                .map_err(|error| io(format!("{error:?}")))?
        } else {
            Vec::new()
        };
        let keep_text = text && old.len() <= TEXT_LIMIT && new.len() <= TEXT_LIMIT;
        let as_text = |bytes: Vec<u8>, mode: u32| {
            (keep_text && mode != 0).then(|| String::from_utf8_lossy(&bytes).into_owned())
        };
        Ok(RevisionDiff {
            images: None,
            path: name.clone(),
            old_path: None,
            new: side_label(to),
            old: side_label(from),
            new_oid: None,
            old_oid: None,
            status: match (old_mode, new_mode) {
                (0, _) => FileState::Added,
                (_, 0) => FileState::Deleted,
                _ => FileState::Modified,
            },
            binary: !text,
            old_mode,
            new_mode,
            hunks,
            old_text: as_text(old, old_mode),
            new_text: as_text(new, new_mode),
            texts_omitted: text && !keep_text,
        })
    })
    .await
}

/// A version's text, for a surface that needs the whole of it. Refuses a binary version.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_text(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    version: u64,
) -> Result<String> {
    let history = Arc::clone(&history);
    blocking(move || {
        let capture = capture_of(&history, project)?;
        let (bytes, _) = side_bytes(&capture, LocalSide::Version { id: version }, Path::new(""))?;
        String::from_utf8(bytes).map_err(|_| io("This version is not text."))
    })
    .await
}

/// A patch from one side to the other, for the clipboard. Local History is personal, so this is
/// how a change in it reaches anybody else.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_patch(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: String,
    from: LocalSide,
    to: LocalSide,
) -> Result<String> {
    let history = Arc::clone(&history);
    blocking(move || {
        let capture = capture_of(&history, project)?;
        let path = PathBuf::from(path);
        let (old, _) = side_bytes(&capture, from, &path)?;
        let (new, _) = side_bytes(&capture, to, &path)?;
        let name = capture
            .locate(&path)
            .map_or_else(|| path.display().to_string(), |(_, rel)| rel);
        let rel = Path::new(&name);
        cide_git::diff::patch_between(&old, rel, &new, rel)
            .map_err(|error| io(format!("{error:?}")))
    })
    .await
}

/// Put a name on this moment. Refused while the store is being read by another cide.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_label(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    text: String,
) -> Result<()> {
    let history = Arc::clone(&history);
    blocking(move || {
        let capture = capture_of(&history, project)?;
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err(io("A label needs a name."));
        }
        capture.label(text).map_err(CoreError::Io)
    })
    .await
}

/// Every spelling of this file an open tab might use.
///
/// `EditorPane` matches `cide://session-tool` paths with `includes`, exactly, so a tab that
/// reached a file through a link would otherwise miss the refresh and keep showing a buffer the
/// restore has just replaced.
fn spellings(state: &WorkspaceState, path: &Path) -> Vec<String> {
    let real = path.canonicalize().ok();
    let mut out = vec![path.display().to_string()];
    if let Some(real) = &real {
        out.push(real.display().to_string());
    }
    state.with(|ws| {
        for project in ws.projects.values() {
            for tab in &project.tabs {
                let (TabKind::File { path: open, .. } | TabKind::ClipboardDiff { path: open, .. }) =
                    &tab.kind
                else {
                    continue;
                };
                if real.is_some() && open.canonicalize().ok() == real {
                    out.push(open.display().to_string());
                }
            }
        }
    });
    out.sort();
    out.dedup();
    out
}

/// Make a file what a version says. The caller has already saved any unsaved buffer.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_restore(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: String,
    version: u64,
) -> Result<LocalRestored> {
    let history = Arc::clone(&history);
    let open_roots = super::file::open_roots(&state);
    let caches = cide_core::toolchain::dependency_roots();
    let target = PathBuf::from(&path);
    let restored = blocking(move || {
        let capture = capture_of(&history, project)?;
        capture
            .restore(RestoreRequest {
                path: PathBuf::from(path),
                version,
                open_roots,
                caches,
            })
            .map_err(CoreError::Io)
    })
    .await?;

    // The one event that already reaches everything showing this file: the editor, the drawing
    // pane, the diff panes and the git panel. None of them reads the session name.
    crate::emit::session_tool(&app, "local-history", spellings(&state, &target));
    Ok(match restored {
        Restored::Rewritten => LocalRestored::Rewritten,
        Restored::Recreated => {
            // A row that came back. `fs_delete`'s reason in reverse: the event is the only road
            // into another window's tree.
            let change = cide_ipc::FsChange {
                paths: vec![target],
                truncated: false,
                git: false,
            };
            crate::emit::fs_changed(&app, project, &change);
            LocalRestored::Recreated
        }
    })
}

/// How much a content search reads before it says it stopped. Unique blobs only, so a file with a
/// thousand versions that differ by a line costs its distinct versions and no more.
const SEARCH_BLOBS: usize = 2_000;
const SEARCH_BYTES: u64 = 256 << 20;

fn holds(haystack: &[u8], lowered_needle: &[u8]) -> bool {
    haystack
        .windows(lowered_needle.len())
        .any(|window| window.eq_ignore_ascii_case(lowered_needle))
}

/// Which versions of a file contain this text. Each distinct content is read once.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_search(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: String,
    query: String,
) -> Result<LocalSearchResult> {
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;

    let history = Arc::clone(&history);
    let epoch = history.search_epoch.fetch_add(1, Ordering::AcqRel) + 1;
    blocking(move || {
        let capture = capture_of(&history, project)?;
        let (root, rel) = capture
            .locate(Path::new(&path))
            .ok_or_else(|| io("This file is outside the project."))?;
        let needle = query.to_ascii_lowercase().into_bytes();
        if needle.is_empty() {
            return Ok(LocalSearchResult {
                matches: Vec::new(),
                partial: false,
            });
        }
        // Handles first, then the lock goes: inflating thousands of blobs must not hold a store
        // the worker is waiting to write to.
        let versions = capture
            .with_store(|store| {
                store
                    .versions(root, &rel)
                    .into_iter()
                    .filter_map(|v| Some((v.id, v.hash?, store.blob(v.id)?)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut seen = HashMap::new();
        let mut matches = Vec::new();
        let mut read = 0_u64;
        let mut partial = false;
        for (id, hash, blob) in versions {
            if history.search_epoch.load(Ordering::Acquire) != epoch {
                partial = true;
                break;
            }
            if !seen.contains_key(&hash) {
                if seen.len() >= SEARCH_BLOBS || read >= SEARCH_BYTES {
                    partial = true;
                    break;
                }
                let bytes = blob.read().unwrap_or_default();
                read += bytes.len() as u64;
                seen.insert(hash, holds(&bytes, &needle));
            }
            if seen.get(&hash) == Some(&true) {
                matches.push(id);
            }
        }
        Ok(LocalSearchResult { matches, partial })
    })
    .await
}

/// Forget every version of this project. Asked of the user first, and it cannot be undone.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_clear(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
) -> Result<()> {
    let history = Arc::clone(&history);
    blocking(move || {
        capture_of(&history, project)?
            .clear()
            .map_err(CoreError::Io)
    })
    .await
}

/// What restoring a folder to the state after a group would do, for the confirmation.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_plan(
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: Option<String>,
    group: u64,
) -> Result<LocalRestorePlan> {
    let history = Arc::clone(&history);
    blocking(move || {
        let capture = capture_of(&history, project)?;
        let (root, prefix) = scope_of(&capture, path.as_deref().map(Path::new))?;
        let plan = capture
            .with_store(|store| {
                let at = store.group_end(group)?;
                Some(store.plan_at(root, &prefix, at))
            })
            .flatten()
            .ok_or_else(|| io("This change is no longer kept."))?;
        let mut out = LocalRestorePlan {
            write: Vec::new(),
            remove: Vec::new(),
            left: Vec::new(),
        };
        for file in plan {
            match file.target {
                TargetState::Unknown => out.left.push(file.rel),
                _ if file.unchanged => {}
                TargetState::Content(_) => out.write.push(file.rel),
                TargetState::Absent => out.remove.push(file.rel),
            }
        }
        Ok(out)
    })
    .await
}

/// Put a folder back to the state after a group.
#[tauri::command(rename_all = "camelCase")]
pub async fn local_history_restore_tree(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    history: State<'_, Arc<Registry>>,
    project: ProjectId,
    path: Option<String>,
    group: u64,
    label: String,
) -> Result<LocalRestoreReport> {
    let history = Arc::clone(&history);
    let open_roots = super::file::open_roots(&state);
    let caches = cide_core::toolchain::dependency_roots();
    let report = blocking(move || {
        let capture = capture_of(&history, project)?;
        let (root, prefix) = scope_of(&capture, path.as_deref().map(Path::new))?;
        let at = capture
            .with_store(|store| store.group_end(group))
            .flatten()
            .ok_or_else(|| io("This change is no longer kept."))?;
        capture
            .restore_tree(TreeRequest {
                root,
                prefix,
                at,
                label,
                open_roots,
                caches,
                trash: cide_fs::trash::move_to_trash,
            })
            .map_err(CoreError::Io)
    })
    .await?;

    let mut touched = Vec::new();
    for path in report.written.iter().chain(&report.removed) {
        touched.extend(spellings(&state, path));
    }
    crate::emit::session_tool(&app, "local-history", touched);
    let change = cide_ipc::FsChange {
        paths: report
            .written
            .iter()
            .chain(&report.removed)
            .cloned()
            .collect(),
        truncated: false,
        git: false,
    };
    crate::emit::fs_changed(&app, project, &change);
    Ok(LocalRestoreReport {
        written: report.written.len() as u32,
        removed: report.removed.len() as u32,
        left: report.left,
        failed: report
            .failed
            .into_iter()
            .map(|(path, reason)| LocalRestoreFailure {
                path: path.display().to_string(),
                reason,
            })
            .collect(),
    })
}

/// Open a history tab, or bring the one already open forward.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_local_history(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    path: Option<String>,
    folder: bool,
) -> Result<TabId> {
    let path = path.map(PathBuf::from);
    state.update(|ws| {
        let existing = workspace::project(ws, project)?.tabs.iter().find_map(|t| {
            matches!(&t.kind, TabKind::LocalHistory { path: open, .. } if open == &path)
                .then_some(t.id)
        });
        if let Some(id) = existing {
            workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }
        let kind = TabKind::LocalHistory { path, folder };
        let pane = super::file::diff_pane(kind.title());
        workspace::open_tab(ws, project, kind, pane)
    })
}
