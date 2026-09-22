//! GitLab IPC: blocking transport off the UI thread and review-scoped language services.
use crate::{lsp::DiagnosticsRegistry, workspace_state::WorkspaceState};
use cide_ipc::{
    ProjectId, RepoId,
    gitlab::{GitLabRequest, GitLabResponse},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
pub struct GitLabState {
    service: OnceLock<Result<Arc<cide_gitlab::GitLab>, String>>,
    operations: Arc<parking_lot::Mutex<()>>,
    stopping: Arc<AtomicBool>,
}
impl GitLabState {
    fn get(&self) -> Result<Arc<cide_gitlab::GitLab>, String> {
        self.service
            .get_or_init(|| {
                cide_gitlab::GitLab::load(
                    cide_core::persist::state_dir().join("gitlab.json"),
                    cide_core::persist::cache_dir().join("gitlab-reviews"),
                )
                .map(Arc::new)
            })
            .clone()
    }
    pub fn shutdown(&self, app: &AppHandle) {
        self.stopping.store(true, Ordering::Release);
        if let Some(Ok(service)) = self.service.get() {
            let reviews = service.board().reviews;
            for review in &reviews {
                service.cancel_workspace(&review.id);
            }
            let _gate = self.operations.lock();
            for review in reviews {
                for sha in service.workspace_versions(&review.id) {
                    if let Ok(project) = review_project(&review.id, &sha) {
                        if let Some(diagnostics) = app.state::<DiagnosticsRegistry>().get(project) {
                            diagnostics.stop_servers();
                        }
                        app.state::<DiagnosticsRegistry>().close(project);
                    }
                }
            }
            service.shutdown();
        }
    }
}
fn review_project(id: &str, sha: &str) -> Result<ProjectId, String> {
    format!(
        "{}{}",
        id.get(..16).ok_or("Invalid review identity")?,
        sha.get(..16).ok_or("Invalid review revision")?
    )
    .parse()
    .map_err(|_| "Invalid review identity".into())
}
#[tauri::command(rename_all = "camelCase")]
pub async fn gitlab_request(
    app: AppHandle,
    state: State<'_, GitLabState>,
    workspace: State<'_, WorkspaceState>,
    request: GitLabRequest,
) -> Result<GitLabResponse, String> {
    if state.stopping.load(Ordering::Acquire) {
        return Err("Cide is closing".into());
    }
    let stopping = state.stopping.clone();
    let service = state.get()?;
    service.set_proxy(workspace.snapshot().settings.proxy);
    let closing = match &request {
        GitLabRequest::Close { review } => vec![review.clone()],
        GitLabRequest::Disconnect { account } => service
            .board()
            .reviews
            .into_iter()
            .filter(|r| r.account == *account)
            .map(|r| r.id)
            .collect(),
        _ => vec![],
    };
    for id in &closing {
        if !matches!(&request, GitLabRequest::Checkout { .. }) {
            service.cancel_workspace(id);
        }
    }
    let operations = state.operations.clone();
    let checkout = if let GitLabRequest::Checkout { review, sha } = &request {
        Some((review.clone(), sha.clone()))
    } else {
        None
    };
    let changed = matches!(
        &request,
        GitLabRequest::Connect { .. }
            | GitLabRequest::Disconnect { .. }
            | GitLabRequest::Preferences { .. }
            | GitLabRequest::Open { .. }
            | GitLabRequest::Close { .. }
    );
    let needs_gate = !closing.is_empty()
        || matches!(
            &request,
            GitLabRequest::Open { .. } | GitLabRequest::Checkout { .. }
        );
    let worker_app = app.clone();
    let worker = service.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _gate = needs_gate.then(|| operations.lock());
        let app = worker_app;
        if !closing.is_empty() {
            app.state::<WorkspaceState>()
                .update(|workspace| close_review_tabs(workspace, &closing))
                .map_err(|e| e.to_string())?;
        }
        for id in closing {
            for sha in worker.workspace_versions(&id) {
                if let Ok(project) = review_project(&id, &sha) {
                    if let Some(diagnostics) = app.state::<DiagnosticsRegistry>().get(project) {
                        diagnostics.stop_servers();
                    }
                    app.state::<DiagnosticsRegistry>().close(project);
                }
            }
        }
        let result = worker.execute(request)?;
        if let Some((id, sha)) = checkout {
            if !stopping.load(Ordering::Acquire) && worker.review_open(&id) {
                if let Some(root) = worker.workspace_root(&id, &sha) {
                    app.state::<DiagnosticsRegistry>().ensure(
                        &app,
                        review_project(&id, &sha)?,
                        vec![root],
                    );
                }
            }
        }
        Ok::<_, String>(result)
    })
    .await
    .map_err(|_| "GitLab worker did not finish")??;
    if changed {
        crate::emit::gitlab_changed(&app, &service.board());
    }
    Ok(result)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn gitlab_after_push(
    state: State<'_, GitLabState>,
    workspace: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    remote: String,
    branch: String,
) -> Result<serde_json::Value, String> {
    let service = state.get()?;
    let snapshot = workspace.snapshot();
    service.set_proxy(snapshot.settings.proxy);
    let roots: Vec<PathBuf> = snapshot
        .projects
        .get(&project)
        .ok_or("Project closed")?
        .roots
        .iter()
        .map(|r| r.path.clone())
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        let info = cide_git::repo::find(&roots, repo).map_err(|_| "Repository unavailable")?;
        let repository = cide_git::repo::open(&info.root).map_err(|_| "Repository unavailable")?;
        let remote = repository
            .find_remote(&remote)
            .map_err(|_| "Push remote unavailable")?;
        let url = remote.url().map_err(|_| "Push remote has no URL")?;
        let accounts: Vec<_> = service
            .board()
            .accounts
            .into_iter()
            .filter(|a| cide_gitlab::project_from_url(&a.host, url).is_ok())
            .collect();
        if accounts.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        if accounts.len() > 1 {
            return Ok(serde_json::json!({"accounts":accounts,"remoteUrl":url,"branch":branch}));
        }
        let account = accounts[0].id.clone();
        let result = service.execute(GitLabRequest::AfterPush {
            account: account.clone(),
            remote_url: url.into(),
            branch,
        })?;
        Ok(serde_json::json!({"account":account,"result":result.data}))
    })
    .await
    .map_err(|_| "GitLab worker did not finish")?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn gitlab_local_file(
    state: State<'_, GitLabState>,
    workspace: State<'_, WorkspaceState>,
    review: String,
    project: ProjectId,
    source_url: String,
    path: String,
) -> Result<Option<String>, String> {
    let service = state.get()?;
    let board = service.board();
    let r = board
        .reviews
        .iter()
        .find(|r| r.id == review)
        .ok_or("Review closed")?;
    let account = board
        .accounts
        .iter()
        .find(|a| a.id == r.account)
        .ok_or("Account disconnected")?;
    let host = account.host.clone();
    let source = cide_gitlab::project_from_url(&host, &source_url)?;
    let snapshot = workspace.snapshot();
    let roots: Vec<PathBuf> = snapshot
        .projects
        .get(&project)
        .ok_or("Project closed")?
        .roots
        .iter()
        .map(|r| r.path.clone())
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        for info in cide_git::repo::discover(&roots) {
            let repo = cide_git::repo::open(&info.root).map_err(|_| "Repository unavailable")?;
            for name in repo
                .remotes()
                .map_err(|_| "Cannot read remotes")?
                .iter()
                .flatten()
                .flatten()
            {
                if repo
                    .find_remote(name)
                    .ok()
                    .and_then(|r| r.url().ok().map(str::to_string))
                    .is_some_and(|url| {
                        cide_gitlab::project_from_url(&host, &url).ok().as_deref() == Some(&source)
                    })
                {
                    let candidate = info
                        .root
                        .join(&path)
                        .canonicalize()
                        .map_err(|_| "File is absent from the current working tree")?;
                    if !candidate.starts_with(
                        info.root
                            .canonicalize()
                            .map_err(|_| "Repository unavailable")?,
                    ) {
                        return Err("File is outside the repository".into());
                    }
                    return Ok(Some(candidate.to_string_lossy().into()));
                }
            }
        }
        Ok(None)
    })
    .await
    .map_err(|_| "GitLab worker did not finish")?
}

#[tauri::command(rename_all = "camelCase")]
pub fn gitlab_open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    if !url.starts_with("https://") {
        return Err("Only HTTPS GitLab links can be opened".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "Could not open your browser".into())
}

/// Review documents use the same tab activation, deduplication and close lifecycle as git diffs.
#[tauri::command(rename_all = "camelCase")]
pub async fn gitlab_open_document(
    app: AppHandle,
    state: State<'_, GitLabState>,
    project: ProjectId,
    document: cide_ipc::gitlab::GitLabDocument,
) -> Result<cide_ipc::TabId, String> {
    let service = state.get()?;
    let operations = state.operations.clone();
    let stopping = state.stopping.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = operations.lock();
        if stopping.load(Ordering::Acquire) || !service.review_open(&document.review) {
            return Err("This review is closed".into());
        }
        app.state::<WorkspaceState>()
            .update(|workspace| open_review_tab(workspace, project, &document))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "Could not open the review tab")?
}

fn open_review_tab(
    workspace: &mut cide_ipc::Workspace,
    project: ProjectId,
    document: &cide_ipc::gitlab::GitLabDocument,
) -> cide_core::Result<cide_ipc::TabId> {
    use cide_core::workspace as ws;
    use cide_ipc::{DiffOrigin, DiffSpec, TabKind};
    let existing = ws::project(workspace, project)?
        .tabs
        .iter()
        .find_map(|tab| {
            matches!(&tab.kind, TabKind::Diff { spec, .. }
                if matches!(&spec.origin, DiffOrigin::GitLab { document: d } if d == document))
            .then_some(tab.id)
        });
    if let Some(tab) = existing {
        ws::activate_tab(workspace, project, tab)?;
        return Ok(tab);
    }
    let name = document.path.rsplit('/').next().unwrap_or(&document.path);
    let mode = match document.mode {
        cide_ipc::gitlab::GitLabDocumentMode::Diff => "MR diff",
        cide_ipc::gitlab::GitLabDocumentMode::Source => "MR source",
        cide_ipc::gitlab::GitLabDocumentMode::Base => "MR base",
    };
    let sha = if document.mode == cide_ipc::gitlab::GitLabDocumentMode::Base {
        &document.base_sha
    } else {
        &document.head_sha
    };
    let title = format!(
        "{name} · {mode} @ {}",
        sha.chars().take(7).collect::<String>()
    );
    ws::open_tab(
        workspace,
        project,
        TabKind::Diff {
            spec: DiffSpec {
                title: title.clone(),
                old_path: document.old_path.clone().into(),
                new_path: document.path.clone().into(),
                origin: DiffOrigin::GitLab {
                    document: document.clone(),
                },
            },
            preview: false,
        },
        super::file::diff_pane(title),
    )
}

fn close_review_tabs(
    workspace: &mut cide_ipc::Workspace,
    reviews: &[String],
) -> cide_core::Result<()> {
    let tabs: Vec<_> = workspace
        .projects
        .iter()
        .flat_map(|(project, p)| {
            p.tabs.iter().filter_map(|tab| match &tab.kind {
                cide_ipc::TabKind::Diff { spec, .. }
                    if matches!(&spec.origin,
                cide_ipc::DiffOrigin::GitLab { document } if reviews.contains(&document.review)) =>
                {
                    Some((*project, tab.id))
                }
                _ => None,
            })
        })
        .collect();
    for (project, tab) in tabs {
        cide_core::workspace::close_tab(workspace, project, tab, true)?;
    }
    Ok(())
}

#[cfg(test)]
mod review_tab_tests {
    use super::*;
    use cide_core::workspace as ws;
    use cide_ipc::gitlab::{GitLabDocument, GitLabDocumentMode};
    #[test]
    fn review_tabs_coexist_deduplicate_pin_revisions_and_close_together() {
        let mut workspace = cide_ipc::Workspace::default();
        let project =
            ws::open_project(&mut workspace, vec!["/local-project".into()], None).unwrap();
        let original = workspace.projects[&project].active_tab;
        let doc = GitLabDocument {
            review: "remote-review".into(),
            path: "main.go".into(),
            old_path: "main.go".into(),
            base_sha: "a".repeat(40),
            start_sha: "b".repeat(40),
            head_sha: "c".repeat(40),
            mode: GitLabDocumentMode::Diff,
            new_file: false,
            deleted_file: false,
        };
        let diff = open_review_tab(&mut workspace, project, &doc).unwrap();
        assert_eq!(
            diff,
            open_review_tab(&mut workspace, project, &doc).unwrap()
        );
        ws::activate_tab(&mut workspace, project, original).unwrap();
        assert_eq!(workspace.projects[&project].active_tab, original);
        let source = open_review_tab(
            &mut workspace,
            project,
            &GitLabDocument {
                mode: GitLabDocumentMode::Source,
                ..doc.clone()
            },
        )
        .unwrap();
        assert_ne!(source, diff);
        let later = open_review_tab(
            &mut workspace,
            project,
            &GitLabDocument {
                head_sha: "d".repeat(40),
                ..doc.clone()
            },
        )
        .unwrap();
        assert_ne!(later, diff);
        let second_project =
            ws::open_project(&mut workspace, vec!["/other-project".into()], None).unwrap();
        open_review_tab(&mut workspace, second_project, &doc).unwrap();
        let other = open_review_tab(
            &mut workspace,
            project,
            &GitLabDocument {
                review: "another-review".into(),
                ..doc
            },
        )
        .unwrap();
        close_review_tabs(&mut workspace, &["remote-review".into()]).unwrap();
        assert!(
            workspace.projects[&project]
                .tabs
                .iter()
                .any(|t| t.id == original)
        );
        assert!(
            workspace.projects[&project]
                .tabs
                .iter()
                .any(|t| t.id == other)
        );
        assert_eq!(workspace.projects[&project].tabs.len(), 2);
        assert_eq!(workspace.projects[&second_project].tabs.len(), 1);
    }
}
