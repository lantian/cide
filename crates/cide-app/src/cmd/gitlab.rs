//! GitLab IPC: blocking transport off the UI thread and review-scoped language services.
use crate::{lsp::DiagnosticsRegistry, workspace_state::WorkspaceState};
use cide_ipc::{
    ProjectId, RepoId, RunId,
    gitlab::{GitLabRequest, GitLabResponse, GitLabReviewHarness},
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
    /// The service, for a caller outside the commands — `agent_rpc`'s review tools. (M85)
    pub(crate) fn service(&self) -> Result<Arc<cide_gitlab::GitLab>, String> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("Cide is closing".into());
        }
        self.get()
    }
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
    // The user's own draft changes reach every window the way an agent's do. (M85)
    let drafts_of = match &request {
        GitLabRequest::DraftEdit { review, .. }
        | GitLabRequest::DraftDiscard { review, .. }
        | GitLabRequest::DraftPublish { review, .. } => Some(review.clone()),
        _ => None,
    };
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
        // A review's agent runs stand in the checkout this close deletes (M85): stopped first,
        // so none is left working in a directory that is no longer there.
        if let Some(registry) = app.try_state::<Arc<crate::agents::AgentRegistry>>() {
            for id in &closing {
                for (project, run) in registry.review_runs(id) {
                    if let Err(error) = crate::cmd::agents::agents_stop_blocking(
                        &app,
                        project,
                        run,
                        crate::agents::StopBy::User,
                        Some("the MR review was closed".into()),
                        true,
                    ) {
                        tracing::warn!(%error, %run, "a review run did not stop with its review");
                    }
                }
            }
        }
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
        if let Some((id, sha)) = checkout
            && !stopping.load(Ordering::Acquire)
            && worker.review_open(&id)
            && let Some(root) = worker.workspace_root(&id, &sha)
        {
            app.state::<DiagnosticsRegistry>()
                .ensure(&app, review_project(&id, &sha)?, vec![root]);
        }
        Ok::<_, String>(result)
    })
    .await
    .map_err(|_| "GitLab worker did not finish")??;
    if changed {
        crate::emit::gitlab_changed(&app, &service.board());
    }
    if let Some(review) = drafts_of {
        crate::emit::gitlab_drafts_changed(&app, &review);
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

/// Every harness cide can run a review on, and why not where it cannot. (M85)
///
/// `defs::implemented` then `defs::installed`, the order `defs::load` asks them in, so the
/// dialog greys a harness out with the sentence the Agents panel would show for a role on it.
/// On a worker: `installed` probes `PATH`.
#[tauri::command(rename_all = "camelCase")]
pub async fn gitlab_review_harnesses() -> Result<Vec<GitLabReviewHarness>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        cide_agents::harness::registry()
            .iter()
            .map(|harness| {
                let harness = harness.kind();
                GitLabReviewHarness {
                    harness,
                    unavailable: cide_agents::defs::implemented(harness)
                        .or_else(|| cide_agents::defs::installed(harness)),
                }
            })
            .collect()
    })
    .await
    .map_err(|_| "GitLab worker did not finish".to_string())
}

/// Start an agent reviewing a merge request. (M85) Answers the run at once; the run's own
/// queue decides when it starts, and the panel opens its tab once it has a child.
///
/// The checkout is made **here**, before the run is enqueued, and not at the fork: it is a
/// network fetch that can fail for reasons the user must see (a token without `read_repository`,
/// a deleted source branch), and a failure inside a queued run surfaces as a failed row nobody
/// was looking at. Made under the operations gate, like the `Checkout` request's, so a Close
/// racing it cannot delete the directory half-way through.
#[tauri::command(rename_all = "camelCase")]
pub async fn gitlab_review_launch(
    app: AppHandle,
    state: State<'_, GitLabState>,
    workspace: State<'_, WorkspaceState>,
    project: ProjectId,
    review: String,
    harness: cide_ipc::Harness,
    prompt: Option<String>,
) -> Result<RunId, String> {
    let (registry, spec) = plan_review(
        &app,
        &state,
        &workspace,
        project,
        review,
        harness,
        Opening::Fresh(prompt),
    )
    .await?;
    let run = registry
        .enqueue_unique(spec)
        .map_err(|_| "A review of this MR is already queued".to_string())?;
    registry.mark_changed(&app, project);
    registry.pump(&app);
    Ok(run)
}

/// What a review run reads first.
enum Opening {
    /// A new review: `opening_line`, with the user's extra instructions if any.
    Fresh(Option<String>),
    /// A reviewer revived to answer on one of its drafts (M101): this line, typed into the
    /// conversation it resumes, standing in the checkout of the head it reviewed — `claude
    /// --resume` finds a transcript by the directory it was written in, so the revival must
    /// stand where the original did, not in a newer head's checkout.
    Discuss { line: String, head: String },
}

/// Everything a review run needs before it is queued: the checkout made, the brief composed.
/// The launch and a Discuss revival share it, so a revived reviewer has the same brief, tools
/// and directory as the one that wrote the draft.
async fn plan_review(
    app: &AppHandle,
    state: &GitLabState,
    workspace: &WorkspaceState,
    project: ProjectId,
    review: String,
    harness: cide_ipc::Harness,
    opening: Opening,
) -> Result<
    (
        Arc<crate::agents::AgentRegistry>,
        crate::agents::DispatchSpec,
    ),
    String,
> {
    let service = state.service()?;
    let snapshot = workspace.snapshot();
    service.set_proxy(snapshot.settings.proxy.clone());
    let roots: Vec<PathBuf> = snapshot
        .projects
        .get(&project)
        .ok_or("Open a project to host the review: its run lives in that project's Agents panel")?
        .roots
        .iter()
        .map(|r| r.path.clone())
        .collect();
    let root = roots
        .first()
        .cloned()
        .ok_or("The project hosting the review has no folder")?;
    let registry = app
        .try_state::<Arc<crate::agents::AgentRegistry>>()
        .map(|r| Arc::clone(&r))
        .ok_or("This window has no agent registry")?;
    let operations = state.operations.clone();
    let worker = service.clone();
    let spec = tauri::async_runtime::spawn_blocking(move || {
        let brief_for = cide_agents::harness::for_kind(harness)
            .ok_or_else(|| format!("This build cannot run {harness:?}"))?;
        let mr = worker.detail(&review)?;
        let version = worker.latest_version(&review)?;
        let head = match &opening {
            Opening::Discuss { head, .. } => head.clone(),
            Opening::Fresh(_) => version["head_commit_sha"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        };
        let base = version["base_commit_sha"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (checkout, with_base) = {
            let _gate = operations.lock();
            worker.review_checkout(&review, &head, &base)?
        };
        let checkout = PathBuf::from(checkout);
        let prompt = match opening {
            Opening::Discuss { line, .. } => line,
            Opening::Fresh(extra) => {
                let host = worker.account_of(&review)?.host;
                let projects: Vec<String> = [
                    mr["web_url"]
                        .as_str()
                        .and_then(|url| cide_gitlab::project_from_url(&host, url).ok()),
                    mr["source_project"]["path_with_namespace"]
                        .as_str()
                        .map(str::to_string),
                ]
                .into_iter()
                .flatten()
                .collect();
                let local = crate::mr_review::local_repo_for(&roots, &host, &projects);
                crate::mr_review::opening_line(
                    &mr,
                    &version,
                    &checkout,
                    with_base,
                    local.as_deref(),
                    extra.as_deref(),
                )
            }
        };
        let config = cide_agents::load_project(&root).config.agents;
        Ok::<_, String>(crate::agents::DispatchSpec {
            project,
            agent: cide_ipc::AgentId(crate::mr_review::REVIEW_AGENT.into()),
            agent_label: format!("Review !{}", mr["iid"]),
            harness,
            task: None,
            task_title: None,
            change: None,
            prompt,
            // Several MRs may be reviewed at once; the project's own cap still holds them all.
            agent_limit: 4,
            project_limit: config.max_concurrent,
            checkout: None,
            // Resolved at its fork, like every run before pools were stamped at dispatch: a
            // review names no pool override of its own. `stamp_and_choose` stamps an empty one.
            pool: Vec::new(),
            // A review reports to nobody's conversation: its findings are drafts, its tab is
            // where it is watched, and the product owner has no verdict to give on it.
            notify: cide_ipc::RunNotify::Silent,
            purpose: crate::agents::RunPurpose::MrReview {
                review: review.clone(),
                cwd: checkout,
                brief: cide_agents::review::review_brief(brief_for),
                tools: crate::mr_review::claude_allowed_tools(),
                harness,
            },
        })
    })
    .await
    .map_err(|_| "GitLab worker did not finish")??;
    Ok((registry, spec))
}

/// The user discussing an agent's draft (M101): the message is saved under the draft, then
/// reaches the reviewer that wrote it — typed into its conversation when that run is alive
/// (held until its turn is over), or by reviving that conversation as a new run when it is gone.
/// The reviewer answers with `cide_mr_draft_reply`, which lands under the same draft.
///
/// Answers the revived run's id, so the panel can follow it as it follows a launched review;
/// `None` when the message went to a run that was still there.
///
/// **The reply is saved before anything is delivered**, and a delivery that fails does not undo
/// it: the user's words are theirs whatever the reviewer's state, and the error says why no
/// answer is coming rather than making the text vanish from under the composer.
#[tauri::command(rename_all = "camelCase")]
pub async fn gitlab_draft_discuss(
    app: AppHandle,
    state: State<'_, GitLabState>,
    workspace: State<'_, WorkspaceState>,
    project: ProjectId,
    review: String,
    draft: String,
    body: String,
) -> Result<Option<RunId>, String> {
    let service = state.service()?;
    let registry = app
        .try_state::<Arc<crate::agents::AgentRegistry>>()
        .map(|r| Arc::clone(&r))
        .ok_or("This window has no agent registry")?;
    let found = service.draft(&review, &draft)?;
    let old_run = found
        .author
        .run
        .clone()
        .ok_or("Only a draft an agent wrote can be discussed: nobody would answer this one")?;
    let harness = found
        .author
        .harness
        .ok_or("This draft does not say which agent wrote it")?;
    service.draft_reply(
        &review,
        &draft,
        cide_ipc::gitlab::GitLabDraftAuthor {
            label: "You".into(),
            harness: None,
            run: None,
            conversation: None,
        },
        body.clone(),
        None,
    )?;
    crate::emit::gitlab_drafts_changed(&app, &review);

    let line = discuss_line(&found, &body, harness);
    // The run that wrote it, if the registry still has it alive — in whichever project hosts it,
    // which need not be the one in front now.
    let live = old_run.parse::<RunId>().ok().and_then(|run| {
        registry
            .review_runs(&review)
            .into_iter()
            .find(|(project, r)| *r == run && registry.follow_up_reachable(*project, run))
    });
    if let Some((host, run)) = live {
        registry
            .follow_up(&app, host, run, &line)
            .map_err(|error| {
                format!(
                    "Your message is saved on the draft, but the reviewer was not told: {error}"
                )
            })?;
        return Ok(None);
    }

    let conversation = found.author.conversation.clone().ok_or(
        "Your message is saved on the draft, but the reviewer that wrote it is gone and cide \
         did not record its conversation (drafts from before this version). Start a new review \
         to have it looked at again.",
    )?;
    let (registry, spec) = plan_review(
        &app,
        &state,
        &workspace,
        project,
        review.clone(),
        harness,
        Opening::Discuss {
            line,
            head: found.head_sha.clone(),
        },
    )
    .await
    .map_err(|error| {
        format!(
            "Your message is saved on the draft, but the reviewer could not be revived: {error}"
        )
    })?;
    let run = registry
        .enqueue_continuing(spec, conversation)
        .map_err(|_| "A review of this MR is already queued".to_string())?;
    // Its drafts become the revived run's, or it could not answer on the draft it was asked
    // about: a run answers and edits only what it wrote.
    if let Err(error) = service.draft_reassign(&review, &old_run, &run.to_string()) {
        tracing::warn!(%error, %run, "a revived reviewer did not inherit its drafts");
    }
    crate::emit::gitlab_drafts_changed(&app, &review);
    registry.mark_changed(&app, project);
    registry.pump(&app);
    Ok(Some(run))
}

/// What the reviewer is told when the user discusses one of its drafts. **One line** — typed
/// into a TUI, where every newline is an Enter — so the user's text is flattened and clipped
/// here, and the whole of it is in `cide_mr_drafts`, which the line points at.
fn discuss_line(
    draft: &cide_ipc::gitlab::GitLabDraft,
    body: &str,
    harness: cide_ipc::Harness,
) -> String {
    let tool = |name: &str| {
        cide_agents::harness::for_kind(harness)
            .map_or_else(|| name.to_string(), |h| h.tool_name(name))
    };
    let at = match (&draft.path, draft.line) {
        (Some(path), Some(line)) => format!(" on {path}:{line}"),
        _ => " (general comment)".into(),
    };
    let message = crate::agent_rpc::one_line(body);
    let message = match message.char_indices().nth(DISCUSS_QUOTE) {
        Some((at, _)) => format!(
            "{}… (the whole message is in {})",
            &message[..at],
            tool(cide_agents::review::tool::MR_DRAFTS)
        ),
        None => message,
    };
    format!(
        "The user is discussing your draft {id}{at}. They wrote: \"{message}\". Answer them with \
         {reply} on draft {id} — they read the answer under the draft, not here. If they ask for \
         a change, make it with {edit} (or {discard}) and say in your answer what you changed.",
        id = draft.id,
        reply = tool(cide_agents::review::tool::MR_DRAFT_REPLY),
        edit = tool(cide_agents::review::tool::MR_DRAFT_EDIT),
        discard = tool(cide_agents::review::tool::MR_DRAFT_DISCARD),
    )
}

/// How much of the user's message the typed line quotes; the rest is one tool call away.
const DISCUSS_QUOTE: usize = 600;

/// One review run as the Agents panel would draw it, or `None` once the registry has forgotten
/// it. (M85) The MR panel follows its own review through this rather than the roster: a
/// project whose subagents are switched off answers a roster with no runs in it at all, and a
/// review is allowed to run there.
#[tauri::command(rename_all = "camelCase")]
pub async fn gitlab_review_run(
    app: AppHandle,
    project: ProjectId,
    run: RunId,
) -> Result<Option<cide_ipc::AgentRun>, String> {
    let registry = app
        .try_state::<Arc<crate::agents::AgentRegistry>>()
        .map(|r| Arc::clone(&r))
        .ok_or("This window has no agent registry")?;
    Ok(registry
        .runs_for(project)
        .into_iter()
        .find(|live| live.run == run))
}

#[cfg(test)]
mod discuss_tests {
    use super::*;
    use cide_ipc::gitlab::{GitLabDraft, GitLabDraftAuthor, GitLabSeverity, GitLabSide};

    fn draft() -> GitLabDraft {
        GitLabDraft {
            id: "d1".into(),
            review: "r".into(),
            severity: GitLabSeverity::Major,
            body: "b".into(),
            path: Some("src/a.rs".into()),
            old_path: None,
            side: Some(GitLabSide::New),
            line: Some(12),
            position: None,
            head_sha: "h".into(),
            author: GitLabDraftAuthor {
                label: "Review !1".into(),
                harness: Some(cide_ipc::Harness::Claude),
                run: Some("run".into()),
                conversation: Some("conv".into()),
            },
            created_unix_ms: 0,
            replies: Vec::new(),
        }
    }

    #[test]
    fn the_discuss_line_is_one_line_that_names_the_draft_and_the_reply_tool() {
        let line = discuss_line(
            &draft(),
            "Why major?\n\nIt looks\tminor to me.",
            cide_ipc::Harness::Claude,
        );
        assert!(
            !line.contains('\n') && !line.contains('\r'),
            "a newline is an Enter: {line}"
        );
        assert!(line.contains("draft d1 on src/a.rs:12"), "{line}");
        assert!(
            line.contains("\"Why major? It looks minor to me.\""),
            "{line}"
        );
        assert!(line.contains("mcp__cide__cide_mr_draft_reply"), "{line}");
        let opencode = discuss_line(&draft(), "q", cide_ipc::Harness::Opencode);
        assert!(
            !opencode.contains("mcp__cide__"),
            "each harness spells its tools: {opencode}"
        );
    }

    #[test]
    fn a_long_message_is_clipped_and_points_at_the_whole_of_it() {
        let long = "word ".repeat(400);
        let line = discuss_line(&draft(), &long, cide_ipc::Harness::Claude);
        assert!(line.len() < long.len() + 600, "{}", line.len());
        assert!(line.contains("mcp__cide__cide_mr_drafts"), "{line}");
    }
}
