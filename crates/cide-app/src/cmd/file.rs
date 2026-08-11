//! File-tab and document commands. (M9)
//!
//! Two pairs, and they are unrelated to each other. `tab_open_file` / `tab_set_dirty` are
//! workspace mutations in the same shape as everything in `cmd::project`. `file_read` /
//! `file_write` touch the disk instead, and are the only commands in the app that do
//! arbitrary blocking IO on a path the user chose — so they are the only ones that go
//! through `spawn_blocking`. A `cargo build` saturating the page cache, or a project on a
//! stalled NFS mount, would otherwise freeze the event loop and with it every terminal in
//! the window.

use std::path::PathBuf;

use cide_core::document;
use cide_core::workspace;
use cide_core::{CoreError, Result};
use cide_ipc::git::DiffSide;
use cide_ipc::{
    DiffOrigin, DiffSpec, FileDoc, Pane, PaneId, PaneKind, PaneRole, ProjectId, RepoId, TabId,
    TabKind,
};
use tauri::{Manager, State};

use crate::cmd::project::Mutated;
use crate::workspace_state::WorkspaceState;

/// Open a file tab, or activate the one already showing this path.
///
/// Re-opening rather than duplicating is not a nicety: two tabs over one path are two
/// buffers over one file, and whichever saves second silently discards the other's edits.
/// The domain has no opinion about paths, so the search is here.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_file(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    path: String,
) -> Result<TabId> {
    let path = PathBuf::from(path);
    state.update(|ws| {
        let existing = workspace::project(ws, project)?
            .tabs
            .iter()
            .find(|t| matches!(&t.kind, TabKind::File { path: p, .. } if *p == path))
            .map(|t| t.id);
        if let Some(id) = existing {
            workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }

        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        workspace::open_tab(
            ws,
            project,
            TabKind::File { path, dirty: false },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                // Auxiliary, like every pane outside the pinned console: splitting a file
                // tab and closing one of the halves must not be refused, and closing the
                // last one closes the tab.
                role: PaneRole::Auxiliary,
                session: None,
                title,
            },
        )
    })
}

/// The tab a git diff opens as, given its fetch key.
///
/// Split out of the command so it can be tested without a `WorkspaceState`, and because it
/// is the one place that decides what a diff tab is *called*: the basename plus a marker, so
/// a `main.rs` diff and a `main.rs` editor are two distinguishable rows in the tab strip
/// rather than two identical ones.
///
/// `old_path` is git's pre-image path — set for a rename, absent otherwise — and the pair is
/// display only. Both sides stay **repo-relative**, matching the fetch key; see `DiffSpec`.
fn git_diff_spec(repo: RepoId, path: &str, side: DiffSide, old_path: Option<String>) -> DiffSpec {
    let name = path.rsplit('/').next().unwrap_or(path);
    DiffSpec {
        title: format!("{name} — diff"),
        old_path: PathBuf::from(old_path.unwrap_or_else(|| path.to_owned())),
        new_path: PathBuf::from(path),
        origin: DiffOrigin::Git {
            repo,
            path: path.to_owned(),
            side,
        },
    }
}

/// Whether an open tab is already showing this file's git diff.
///
/// Keyed on the repository and the path and **not** on the side. The pane switches sides in
/// place — the same file's staged and unstaged diffs are two views of one thing, and the
/// selection is cleared when it switches — so keying on the side as well would answer a
/// second double-click with a second tab over the same file, which is exactly the
/// duplicate-tab problem `tab_open_file` exists to avoid.
fn shows_git_diff(kind: &TabKind, repo: RepoId, path: &str) -> bool {
    matches!(
        kind,
        TabKind::Diff { spec, .. } if matches!(
            &spec.origin,
            DiffOrigin::Git { repo: r, path: p, .. } if *r == repo && p == path
        )
    )
}

/// Open a diff tab for one file in one repository, or activate the one already showing it.
///
/// # Why the diff text is not an argument
///
/// The caller — the git panel — is holding a `FileDiff` when it calls this, and handing that
/// over would save the pane a round trip. It is deliberately not accepted. `DiffSpec` is
/// persisted inside `Workspace`, which `cide-core::persist` debounces to `workspace.json`,
/// so a diff passed in here would be a diff written to disk; and a saved diff is a *stale*
/// diff the moment anything writes to the file, which on this code path is constantly (an
/// agent is editing, a build is running, a bash pane is committing). The pane calls
/// `git_diff_file` with the key in [`DiffOrigin::Git`] instead, exactly as a Claude diff
/// calls `claude_diff_content` with its `request_id`.
///
/// It is also what makes the tab survive a restart: a key still resolves tomorrow.
///
/// Nothing here touches the disk — it is a workspace mutation, like `tab_open_file` — so it
/// is deliberately *not* `async`. The blocking git read happens in `git_diff_file`, which
/// already goes through `spawn_blocking`.
///
/// # This is the *double-click* half
///
/// The tab it produces is `preview: false` — kept. Its partner is [`tab_retarget_diff`],
/// which the panel calls for a single click on a changelist row while a diff is already up,
/// and which re-points one scratch tab instead of adding another. Double-click has meant
/// "open properly" since the click rules landed; this is what "properly" now buys you.
///
/// Finding the file already open **promotes** it, which is the same rule read backwards: a
/// double-click on the file currently sitting in the preview slot means the user wants to
/// keep it, so the next single click must leave it alone and start a new scratch tab.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_diff(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    side: DiffSide,
    old_path: Option<String>,
) -> Result<TabId> {
    state.update(|ws| open_git_diff(ws, project, repo, &path, side, old_path))
}

/// The pane a diff tab opens with. One shape for both gestures, so a preview tab and a kept
/// one differ in exactly the flag and nothing a renderer could accidentally key off.
fn diff_pane(title: String) -> Pane {
    Pane {
        id: PaneId::new(),
        kind: PaneKind::Diff,
        // Auxiliary like every pane a tab is opened with: a diff holds no conversation, so
        // there is nothing about it that must not be closed.
        role: PaneRole::Auxiliary,
        // No process, ever. A diff is a document.
        session: None,
        title,
    }
}

/// [`tab_open_diff`] without Tauri.
///
/// Split out for the tests below, which is not a formality here: what this and
/// [`retarget_git_diff`] do to a workspace *is* the fix for the thirty-tab report, and a
/// policy that can only be exercised through a `State<WorkspaceState>` is a policy that gets
/// tested at the level of `shows_git_diff` and nowhere else — which is exactly how a tab
/// lookup that was individually correct produced thirty tabs in a row.
fn open_git_diff(
    ws: &mut cide_ipc::Workspace,
    project: ProjectId,
    repo: RepoId,
    path: &str,
    side: DiffSide,
    old_path: Option<String>,
) -> Result<TabId> {
    let existing = workspace::project(ws, project)?
        .tabs
        .iter()
        .find(|t| shows_git_diff(&t.kind, repo, path))
        .map(|t| t.id);
    if let Some(id) = existing {
        workspace::promote_diff(ws, project, id)?;
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let spec = git_diff_spec(repo, path, side, old_path);
    let title = spec.title.clone();
    workspace::open_tab(
        ws,
        project,
        TabKind::Diff {
            spec,
            preview: false,
        },
        diff_pane(title),
    )
}

/// Point the preview diff tab at this file — the *single-click* half.
///
/// > *"in git files tree when i do one click on element - we should select it, but not open
/// > the diff. Only when diff is already opened one click should change current diff to
/// > selected file."*
///
/// The click rule shipped and routed to [`tab_open_diff`], which reuses a tab only when the
/// repo *and* the path match — so every other file got a new tab and clicking down a 30-file
/// changelist produced 30 tabs, the exact opposite of what was asked for. "Change the current
/// diff" is retargeting, a different operation from opening, and this is it.
///
/// Three outcomes, in this order, and the order is the whole design:
///
/// 1. **A tab already shows this file** — activate it, retarget nothing. Whether it is the
///    preview tab or a kept one, a second copy of a diff the user can already see is never
///    the answer, and stealing the scratch slot to duplicate an open tab would cost them the
///    file that was in it for no gain.
/// 2. **A preview tab exists** — retarget it. One tab, however far down the list they click.
/// 3. **Neither** — open one, marked `preview: true`. Reached when every diff tab on screen
///    was opened by double-click, i.e. deliberately kept; the honest answer there is a new
///    scratch tab rather than eating one the user asked for. It costs exactly one extra tab,
///    once, and every click after it lands in that tab. This is VS Code's rule for a
///    single-click in the explorer when the only open editors are permanent, and it is the
///    reason the flag exists at all — see [`cide_ipc::TabKind::Diff`].
///
/// Not `async`, for the same reason as [`tab_open_diff`]: no disk is touched here. The pane
/// notices its tab's `spec` changed and refetches through `git_diff_file`, which is where the
/// blocking read lives and is already on `spawn_blocking`.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_retarget_diff(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    side: DiffSide,
    old_path: Option<String>,
) -> Result<TabId> {
    state.update(|ws| retarget_git_diff(ws, project, repo, &path, side, old_path))
}

/// [`tab_retarget_diff`] without Tauri. See [`open_git_diff`] for why it is split out.
fn retarget_git_diff(
    ws: &mut cide_ipc::Workspace,
    project: ProjectId,
    repo: RepoId,
    path: &str,
    side: DiffSide,
    old_path: Option<String>,
) -> Result<TabId> {
    let showing = workspace::project(ws, project)?
        .tabs
        .iter()
        .find(|t| shows_git_diff(&t.kind, repo, path))
        .map(|t| t.id);
    if let Some(id) = showing {
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let spec = git_diff_spec(repo, path, side, old_path);
    if let Some(id) = workspace::preview_diff_tab(ws, project)? {
        workspace::retarget_diff(ws, project, id, spec)?;
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let title = spec.title.clone();
    workspace::open_tab(
        ws,
        project,
        TabKind::Diff {
            spec,
            preview: true,
        },
        diff_pane(title),
    )
}

/// Record whether a file tab has unsaved edits.
///
/// The dirty flag lives in the Rust-owned tree rather than in the editor component because
/// it outlives the component: a tab switch unmounts nothing today, but a detached editor
/// window is a different JavaScript realm, and the tab strip that draws the dot is in the
/// other one.
///
/// It is also the guard. `cide_core::workspace::close_tab` refuses a tab carrying this flag
/// unless the caller passes `force`, `close_project` refuses a project holding one, and
/// `app_quit_requested` reports every one of them by name. Setting it therefore has a
/// consequence beyond the dot in the tab strip: an editor that stops reporting a clean save
/// leaves a tab the user can no longer close without a dialog, and one that stops reporting
/// a dirty buffer takes the guard down with it.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_set_dirty(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    dirty: bool,
) -> Result<Mutated> {
    state.update(|ws| {
        let t = workspace::tab_mut(ws, project, tab)?;
        let TabKind::File { dirty: flag, .. } = &mut t.kind else {
            return Err(CoreError::Invariant(format!(
                "tab {tab} is not a file tab and has no dirty state"
            )));
        };
        // Bumping `rev` on a no-op would broadcast a snapshot per keystroke: the editor
        // reports on every transaction, and only the transitions are news.
        if *flag == dirty {
            return Ok(Mutated { rev: ws.rev });
        }
        *flag = dirty;
        workspace::bump(ws);
        Ok(Mutated { rev: ws.rev })
    })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn file_read(path: String) -> Result<FileDoc> {
    blocking(move || document::read(&PathBuf::from(path))).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn file_write(path: String, text: String) -> Result<()> {
    blocking(move || document::write(&PathBuf::from(path), &text)).await
}

/// Run a fallible blocking job on the pool, reporting a lost worker as an IO error.
///
/// The join can only fail if the task panicked or the runtime is shutting down. Neither is
/// something a caller can act on differently from a failed read, and inventing a variant
/// for it would put a case in the frontend's error switch that no test can reach.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tauri::async_runtime::spawn_blocking(job).await {
        Ok(result) => result,
        Err(error) => Err(CoreError::Io(format!("file worker failed: {error}"))),
    }
}

/// Report the editor's selection to the project's Claude sessions.
///
/// The gap this closes: `IdeServer::selection_changed` existed, was unit-tested, and had no
/// producer — nothing in `cide-app` called it and there was no command to reach it from.
/// A handler with no caller is a feature that passes its tests and has never run.
///
/// **Line numbers are 0-based on the wire** and 1-based everywhere a human sees them, so the
/// conversion happens here, at the boundary, exactly once. CodeMirror counts lines from 1.
///
/// Debounced on the frontend, not here: a selection drag fires per animation frame, and a
/// command per frame would be an IPC round trip per frame.
#[tauri::command(rename_all = "camelCase")]
pub fn claude_selection_changed(
    app: tauri::AppHandle,
    project: cide_ipc::ProjectId,
    path: String,
    text: String,
    start_line: u32,
    end_line: u32,
) {
    let Some(servers) = app.try_state::<crate::ide::IdeServers>() else {
        return;
    };
    servers.selection_changed(
        project,
        cide_ide_mcp::SelectionChanged {
            file_path: path,
            text,
            start_line: start_line.saturating_sub(1),
            end_line: end_line.saturating_sub(1),
        },
    );
}

/// Put `@path#L1-2` into a Claude pane's prompt.
///
/// The pane is chosen by the caller, not here: the frontend knows which Claude the user was
/// last looking at, and this layer would have to guess. Lines are 1-based from the caller
/// and 0-based on the wire, converted once at this boundary — the same convention
/// `claude_selection_changed` uses, for the same reason.
///
/// `None` for both lines mentions the whole file, which is what a Ctrl+P pick means.
#[tauri::command(rename_all = "camelCase")]
pub fn claude_mention_file(
    app: tauri::AppHandle,
    project: cide_ipc::ProjectId,
    pane: cide_ipc::PaneId,
    path: String,
    line_start: Option<u32>,
    line_end: Option<u32>,
) {
    let Some(servers) = app.try_state::<crate::ide::IdeServers>() else {
        return;
    };
    servers.at_mentioned(
        project,
        pane,
        cide_ide_mcp::AtMentioned {
            file_path: path,
            line_start: line_start.map(|l| l.saturating_sub(1)),
            line_end: line_end.map(|l| l.saturating_sub(1)),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(path: &str, old: Option<&str>) -> DiffSpec {
        git_diff_spec(
            RepoId::new(),
            path,
            DiffSide::Combined,
            old.map(str::to_owned),
        )
    }

    #[test]
    fn a_git_diff_tab_is_titled_by_its_basename() {
        assert_eq!(
            spec("crates/cide-git/src/patch.rs", None).title,
            "patch.rs — diff"
        );
        // A path with no separator is its own basename. `rsplit` always yields at least one
        // item, which is why the fallback in `git_diff_spec` can never actually fire — it is
        // there so the reader does not have to prove that.
        assert_eq!(spec("README.md", None).title, "README.md — diff");
    }

    /// The fetch key is what `git_diff_file` is called with, so it must stay repo-relative
    /// and must not pick up the display paths' shape.
    #[test]
    fn the_origin_carries_the_repo_relative_path_and_no_content() {
        let repo = RepoId::new();
        let s = git_diff_spec(repo, "src/main.rs", DiffSide::Unstaged, None);
        let DiffOrigin::Git {
            repo: r,
            path,
            side,
        } = s.origin
        else {
            panic!("a git diff must carry a git origin");
        };
        assert_eq!(r, repo);
        assert_eq!(path, "src/main.rs");
        assert_eq!(side, DiffSide::Unstaged);
    }

    /// A rename shows both names; everything else shows one path twice, which is what
    /// `DiffPane`'s path label collapses to a single label.
    #[test]
    fn a_rename_keeps_both_sides() {
        let renamed = spec("src/new.rs", Some("src/old.rs"));
        assert_eq!(renamed.old_path, PathBuf::from("src/old.rs"));
        assert_eq!(renamed.new_path, PathBuf::from("src/new.rs"));

        let plain = spec("src/main.rs", None);
        assert_eq!(plain.old_path, plain.new_path);
    }

    /// Reuse ignores the side and the display paths, and never matches another repository's
    /// file of the same name — the case a path-keyed lookup gets wrong in a monorepo.
    #[test]
    fn reuse_is_keyed_on_the_repository_and_the_path_alone() {
        let repo = RepoId::new();
        let other = RepoId::new();
        let tab = TabKind::Diff {
            spec: git_diff_spec(repo, "src/main.rs", DiffSide::Combined, None),
            preview: false,
        };

        assert!(shows_git_diff(&tab, repo, "src/main.rs"));
        assert!(!shows_git_diff(&tab, other, "src/main.rs"));
        assert!(!shows_git_diff(&tab, repo, "src/other.rs"));

        // A Claude diff over the same file is a different tab: it is holding an agent turn
        // open, and answering it is not the same act as staging.
        let claude = TabKind::Diff {
            spec: DiffSpec {
                title: "main.rs".into(),
                old_path: PathBuf::from("src/main.rs"),
                new_path: PathBuf::from("src/main.rs"),
                origin: DiffOrigin::ClaudeMcp {
                    request_id: "r1".into(),
                },
            },
            preview: false,
        };
        assert!(!shows_git_diff(&claude, repo, "src/main.rs"));
    }

    /// A workspace with one project and nothing but its pinned console.
    fn bare() -> (cide_ipc::Workspace, ProjectId) {
        let mut ws = cide_ipc::Workspace::default();
        let project =
            workspace::open_project(&mut ws, vec![PathBuf::from("/repo")], None).expect("opens");
        (ws, project)
    }

    /// Every git diff tab in strip order, as (path, preview).
    fn diff_tabs(ws: &cide_ipc::Workspace, project: ProjectId) -> Vec<(String, bool)> {
        workspace::project(ws, project)
            .expect("exists")
            .tabs
            .iter()
            .filter_map(|t| match &t.kind {
                TabKind::Diff { spec, preview } => match &spec.origin {
                    DiffOrigin::Git { path, .. } => Some((path.clone(), *preview)),
                    DiffOrigin::ClaudeMcp { .. } => None,
                },
                _ => None,
            })
            .collect()
    }

    /// The bug report, as a loop, because that is how the user hit it.
    ///
    /// > *"in git files tree when i do one click on element - we should select it, but not
    /// > open the diff. Only when diff is already opened one click should change current
    /// > diff to selected file."*
    ///
    /// Thirty single clicks down a changelist used to be thirty calls to `tab_open_diff`,
    /// whose reuse is keyed on `(repo, path)` — so every row after the first missed and
    /// opened a tab. One tab now, showing the last file clicked.
    #[test]
    fn thirty_single_clicks_leave_one_diff_tab() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        let before = workspace::project(&ws, project).expect("exists").tabs.len();

        for i in 0..30 {
            retarget_git_diff(
                &mut ws,
                project,
                repo,
                &format!("src/file{i}.rs"),
                DiffSide::Combined,
                None,
            )
            .expect("retargets");
        }

        assert_eq!(
            diff_tabs(&ws, project),
            vec![("src/file29.rs".into(), true)]
        );
        assert_eq!(
            workspace::project(&ws, project).expect("exists").tabs.len(),
            before + 1,
            "thirty clicks may add one tab, and only on the first of them"
        );
    }

    /// The same loop with a tab the user opened on purpose already up.
    ///
    /// The kept tab is untouched — that is what double-click bought — and the thirty clicks
    /// still share a single scratch tab between them. Two tabs, not thirty-one.
    #[test]
    fn a_double_clicked_tab_survives_thirty_single_clicks() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        open_git_diff(
            &mut ws,
            project,
            repo,
            "src/keep.rs",
            DiffSide::Combined,
            None,
        )
        .expect("opens");

        for i in 0..30 {
            retarget_git_diff(
                &mut ws,
                project,
                repo,
                &format!("src/file{i}.rs"),
                DiffSide::Combined,
                None,
            )
            .expect("retargets");
        }

        assert_eq!(
            diff_tabs(&ws, project),
            vec![
                ("src/keep.rs".into(), false),
                ("src/file29.rs".into(), true),
            ]
        );
    }

    /// Clicking a file that is already on screen activates its tab instead of dragging the
    /// scratch slot onto a duplicate — which would cost the user whatever was in it.
    #[test]
    fn clicking_a_file_that_is_already_open_activates_it() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        let kept = open_git_diff(
            &mut ws,
            project,
            repo,
            "src/keep.rs",
            DiffSide::Combined,
            None,
        )
        .expect("opens");
        let preview =
            retarget_git_diff(&mut ws, project, repo, "src/a.rs", DiffSide::Combined, None)
                .expect("retargets");

        let again = retarget_git_diff(
            &mut ws,
            project,
            repo,
            "src/keep.rs",
            DiffSide::Combined,
            None,
        )
        .expect("retargets");

        assert_eq!(again, kept);
        assert_eq!(
            workspace::project(&ws, project).expect("exists").active_tab,
            kept
        );
        assert_eq!(
            diff_tabs(&ws, project),
            vec![("src/keep.rs".into(), false), ("src/a.rs".into(), true)],
            "the preview tab still holds what it held"
        );
        assert_eq!(
            workspace::preview_diff_tab(&ws, project).expect("exists"),
            Some(preview)
        );
    }

    /// Double-clicking the file in the scratch slot keeps it there — VS Code's promotion,
    /// and the reason the *next* single click has to start a new preview tab rather than
    /// eating this one.
    #[test]
    fn double_clicking_the_preview_promotes_it() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        let preview =
            retarget_git_diff(&mut ws, project, repo, "src/a.rs", DiffSide::Combined, None)
                .expect("retargets");

        let promoted = open_git_diff(&mut ws, project, repo, "src/a.rs", DiffSide::Combined, None)
            .expect("opens");

        assert_eq!(promoted, preview, "no second tab over the same file");
        assert_eq!(diff_tabs(&ws, project), vec![("src/a.rs".into(), false)]);
        assert_eq!(
            workspace::preview_diff_tab(&ws, project).expect("exists"),
            None
        );

        // And now the slot is free again, so the next click opens one rather than stealing
        // the tab that was just promoted.
        retarget_git_diff(&mut ws, project, repo, "src/b.rs", DiffSide::Combined, None)
            .expect("retargets");
        assert_eq!(
            diff_tabs(&ws, project),
            vec![("src/a.rs".into(), false), ("src/b.rs".into(), true)]
        );
    }
}
