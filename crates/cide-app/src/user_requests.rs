//! User requests: durable decisions, task transactions, and delivery to the asking conversation.
use crate::{
    agents::AgentRegistry, state::SessionRegistry, tasks_state::TasksStores,
    workspace_state::WorkspaceState,
};
use cide_core::{
    CoreError, Result, persist,
    user_requests::{AnswerSubmission, AnswerTarget, Requests},
};
use cide_ipc::{
    LinkType, ProjectId, RunId, SessionId, TaskAuthor, TaskId, TaskQuestion, UserRequest,
    UserRequestKind, UserRequestResponse, UserRequestTask, UserRequestsView,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tauri::{AppHandle, Manager};

pub fn store(app: &AppHandle) -> Result<Arc<Requests>> {
    app.try_state::<Arc<Requests>>()
        .map(|s| Arc::clone(&s))
        .ok_or_else(|| CoreError::Io("cide is shutting down".into()))
}
pub fn root(app: &AppHandle, project: ProjectId) -> Result<PathBuf> {
    let workspace = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| CoreError::Io("cide is shutting down".into()))?;
    crate::tasks_state::project_root(&workspace, project)
}
pub fn broadcast(app: &AppHandle, project: ProjectId) {
    if let (Ok(root), Ok(store)) = (root(app, project), store(app)) {
        crate::emit::user_requests_changed(app, project, store.view(&root));
        crate::running::mark(app);
    }
}
pub fn waiting_count(app: &AppHandle, project: ProjectId, tasks: &[cide_ipc::TaskRow]) -> usize {
    let requests = root(app, project)
        .ok()
        .and_then(|root| store(app).ok().map(|s| s.view(&root).requests))
        .unwrap_or_default();
    cide_core::user_requests::waiting_count(&requests, tasks)
}
pub fn holds_planner(app: &AppHandle, project: ProjectId) -> bool {
    if let (Ok(root), Ok(store)) = (root(app, project), store(app)) {
        store.view(&root).requests.iter().any(|r| {
            r.origin.kind == cide_ipc::sessions::SessionKind::Planner && r.wait && !r.delivered
        })
    } else {
        false
    }
}

pub fn create(
    app: &AppHandle,
    project: ProjectId,
    session: Option<SessionId>,
    run: Option<RunId>,
    author: TaskAuthor,
    args: &Value,
) -> Result<UserRequest> {
    let root = root(app, project)?;
    let listing = crate::sessions_state::list(app, project)?;
    let origin = listing.rows.into_iter().filter(|row| {
        run.is_some_and(|run| row.record.run == Some(run)) || (run.is_none() && session.is_some_and(|session| {
            row.record.session == Some(session)
                && crate::conversations::live(app, &row.record) == Some(session)
        }))
    }).max_by_key(|row| (row.record.known, row.record.last_seen_unix_ms))
        .map(|row| row.record).ok_or_else(|| CoreError::Io("The asking conversation is not yet in the session journal. Retry this call; do not finish or continue dependent work.".into()))?;
    let question: TaskQuestion =
        serde_json::from_value(args.get("question").cloned().unwrap_or(Value::Null))
            .map_err(|e| CoreError::Io(format!("Invalid question: {e}")))?;
    question.validate().map_err(CoreError::Io)?;
    let wait = match args.get("wait") {
        None => true,
        Some(Value::Bool(v)) => *v,
        _ => return Err(CoreError::Io("wait must be a boolean".into())),
    };
    let task = args
        .get("task")
        .and_then(Value::as_str)
        .map(|id| TaskId(id.to_owned()));
    let message = question.text().to_owned();
    let kind = match args
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("question")
    {
        "question" => {
            if question.options().iter().any(|o| o.image.is_some()) {
                let tasks = app.state::<Arc<TasksStores>>();
                let current = task
                    .as_ref()
                    .and_then(|id| tasks.get(project).and_then(|store| store.get(id)))
                    .ok_or_else(|| {
                        CoreError::Io(
                            "Image choices must name a task containing their attachments.".into(),
                        )
                    })?;
                if question
                    .options()
                    .iter()
                    .filter_map(|o| o.image.as_ref())
                    .any(|id| {
                        !current
                            .attachments
                            .iter()
                            .any(|a| &a.id == id && !a.deleted)
                    })
                {
                    return Err(CoreError::Io(
                        "A choice image is not attached to the linked task. Attach it first."
                            .into(),
                    ));
                }
            }
            UserRequestKind::Question { question }
        }
        "moveTasks" => {
            if !cide_agents::config::load_tracker(&root) {
                return Err(CoreError::Io(
                    "Task tracking is disabled in this project.".into(),
                ));
            }
            let milestone = args
                .get("milestone")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Io("Name a target milestone.".into()))?;
            let plan = cide_agents::config::load_milestones(&root);
            let goal = plan
                .items
                .iter()
                .find(|m| m.id == milestone)
                .and_then(|m| m.task.clone())
                .ok_or_else(|| CoreError::Io("That milestone has no existing goal task.".into()))?;
            let ids: Vec<TaskId> =
                serde_json::from_value(args.get("tasks").cloned().unwrap_or(Value::Null))
                    .map_err(|e| CoreError::Io(format!("Invalid task list: {e}")))?;
            if ids.is_empty() {
                return Err(CoreError::Io("Propose at least one task.".into()));
            }
            let tasks_store = app.state::<Arc<TasksStores>>().ensure(project, &root);
            let mut tasks = Vec::new();
            for (i, id) in ids.iter().enumerate() {
                if ids[..i].contains(id) || plan.items.iter().any(|m| m.task.as_ref() == Some(id)) {
                    return Err(CoreError::Io(
                        "List each task once, excluding milestone goal tasks.".into(),
                    ));
                }
                let current = tasks_store
                    .get(id)
                    .ok_or_else(|| CoreError::Io(format!("No task {id}.")))?;
                let parents: Vec<_> = current
                    .links
                    .iter()
                    .filter(|l| l.link == LinkType::SubtaskOf && !l.deleted)
                    .map(|l| l.target.clone())
                    .collect();
                if parents.len() > 1 {
                    return Err(CoreError::Io(format!(
                        "{id} has several parents; resolve them before proposing its move."
                    )));
                }
                tasks.push(UserRequestTask {
                    id: id.clone(),
                    title: current.title,
                    status: current.status,
                    parent: parents.into_iter().next(),
                });
            }
            UserRequestKind::MoveTasks {
                milestone: milestone.to_owned(),
                goal,
                tasks,
            }
        }
        _ => return Err(CoreError::Io("kind must be question or moveTasks".into())),
    };
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| message.chars().take(100).collect());
    let request = store(app)?.create(
        &root,
        UserRequest {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            message,
            task,
            author,
            request: kind,
            origin,
            wait,
            created_unix_ms: persist::now_ms(),
            response: None,
            applying: false,
            delivered: false,
            delivery_error: None,
        },
    )?;
    broadcast(app, project);
    Ok(request)
}

pub fn result_value(request: &UserRequest) -> Value {
    let (applied, excluded) = match (&request.request, &request.response) {
        (
            UserRequestKind::MoveTasks { tasks, .. },
            Some(UserRequestResponse::Approve { selected_ids, .. }),
        ) if !request.applying => (
            selected_ids.clone(),
            tasks
                .iter()
                .filter(|t| !selected_ids.contains(&t.id))
                .map(|t| t.id.clone())
                .collect(),
        ),
        _ => (Vec::new(), Vec::new()),
    };
    json!({ "id": request.id, "request": request.request, "message": request.message, "response": request.response, "pending": request.response.is_none() || request.applying, "applied": applied, "excluded": excluded })
}

fn apply_response(app: &AppHandle, project: ProjectId, request: &UserRequest) -> Result<()> {
    if matches!(request.request, UserRequestKind::MoveTasks { .. }) {
        return apply_move(app, project, request);
    }
    static ANSWER: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _answering = ANSWER.lock();
    let root = root(app, project)?;
    let requests = store(app)?;
    let request = requests.get(&root, &request.id)?;
    if !request.applying {
        return Ok(());
    }
    if let (Some(task), UserRequestKind::Question { question }) = (&request.task, &request.request)
    {
        let tasks = app.state::<Arc<TasksStores>>().ensure(project, &root);
        if tasks
            .get(task)
            .is_some_and(|t| t.question.as_ref() == Some(question))
        {
            match &request.response {
                Some(UserRequestResponse::Answer { text, selected_ids }) => {
                    tasks.answer_question(task, text, selected_ids, Some(question))?;
                }
                Some(UserRequestResponse::Cancel | UserRequestResponse::Reject { .. }) => {
                    tasks.edit(
                        task,
                        cide_ipc::TaskEdit::SetQuestion { question: None },
                        TaskAuthor::User,
                    )?;
                }
                _ => {}
            }
        }
        if tasks.get(task).is_some() {
            tasks.write_now();
            tasks.confirm_task_saved(task)?;
            crate::tasks_state::broadcast(app, project, &tasks);
        }
    }
    requests.complete_application(&root, &request.id)?;
    Ok(())
}

fn apply_move(app: &AppHandle, project: ProjectId, request: &UserRequest) -> Result<()> {
    static APPLY: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _applying = APPLY.lock();
    let fresh = store(app)?.get(&root(app, project)?, &request.id)?;
    if !fresh.applying {
        return Ok(());
    }
    let request = &fresh;
    let (
        UserRequestKind::MoveTasks {
            milestone,
            goal,
            tasks,
        },
        Some(UserRequestResponse::Approve { selected_ids, .. }),
    ) = (&request.request, &request.response)
    else {
        return Ok(());
    };
    let root = root(app, project)?;
    let plan = cide_agents::config::load_milestones(&root);
    if !plan
        .items
        .iter()
        .any(|m| &m.id == milestone && m.task.as_ref() == Some(goal))
    {
        let error = "The target milestone changed. Request an updated proposal.";
        store(app)?.reset_application(&root, &request.id, error.into())?;
        return Err(CoreError::Io(error.into()));
    }
    let tasks_store = app.state::<Arc<TasksStores>>().ensure(project, &root);
    let before = tasks_store.list();
    let changed = match tasks_store.approve_moves(&request.id, goal, tasks, selected_ids) {
        Ok(changed) => changed,
        Err(error) => {
            store(app)?.reset_application(&root, &request.id, error.to_string())?;
            return Err(error);
        }
    };
    tasks_store.write_now();
    // The checkpoint remains until the complete transaction is durable.
    tasks_store.confirm_moves_saved(&request.id, goal, selected_ids)?;
    store(app)?.complete_application(&root, &request.id)?;
    crate::tasks_state::broadcast(app, project, &tasks_store);
    let mutations = changed
        .into_iter()
        .map(|after| crate::task_triggers::TaskMutation {
            before: before
                .iter()
                .find(|t| t.id == after.id)
                .map(|t| task_before(t, &after)),
            after,
            author: TaskAuthor::User,
            assign_gesture: false,
            fresh_text: Vec::new(),
        })
        .collect();
    crate::task_triggers::consider(app, project, mutations, cide_ipc::RunNotify::Primary);
    Ok(())
}
fn task_before(row: &cide_ipc::TaskRow, after: &cide_ipc::Task) -> cide_ipc::Task {
    let mut task = after.clone();
    task.status = row.status;
    task.links = row.links.clone();
    task
}

#[tauri::command(rename_all = "camelCase")]
pub async fn user_requests_list(app: AppHandle, project: ProjectId) -> Result<UserRequestsView> {
    tauri::async_runtime::spawn_blocking(move || Ok(store(&app)?.view(&root(&app, project)?)))
        .await
        .map_err(|e| CoreError::Io(e.to_string()))?
}
#[tauri::command(rename_all = "camelCase")]
pub async fn user_request_dismiss(
    app: AppHandle,
    project: ProjectId,
    id: String,
) -> Result<UserRequestsView> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = root(&app, project)?;
        let requests = store(&app)?;
        requests.dismiss(&root, &id)?;
        broadcast(&app, project);
        Ok(requests.view(&root))
    })
    .await
    .map_err(|e| CoreError::Io(e.to_string()))?
}
#[tauri::command(rename_all = "camelCase")]
pub async fn user_request_respond(
    app: AppHandle,
    project: ProjectId,
    id: String,
    response: UserRequestResponse,
) -> Result<UserRequestsView> {
    tauri::async_runtime::spawn_blocking(move || respond(&app, project, &id, response))
        .await
        .map_err(|e| CoreError::Io(e.to_string()))?
}

pub fn respond(
    app: &AppHandle,
    project: ProjectId,
    id: &str,
    response: UserRequestResponse,
) -> Result<UserRequestsView> {
    let root = root(app, project)?;
    let requests = store(app)?;
    let request = requests.decide(&root, id, response)?;
    if request.applying
        && let Err(error) = apply_response(app, project, &request)
    {
        let _ = requests.delivery_error(&root, id, error.to_string());
        broadcast(app, project);
        return Err(error);
    }
    broadcast(app, project);
    Ok(requests.view(&root))
}

/// A profile-local loop retries delivery only at safe, idle boundaries, including after restart.
pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("user-requests".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(workspace) = app.try_state::<WorkspaceState>() else {
                    break;
                };
                let projects = workspace.with(|ws| {
                    ws.projects
                        .values()
                        .filter_map(|p| p.roots.first().map(|r| (p.id, r.path.clone())))
                        .collect::<Vec<_>>()
                });
                for (project, root) in projects {
                    let Ok(requests) = store(&app) else {
                        return;
                    };
                    for mut request in requests.view(&root).requests {
                        if matches!(request.request, UserRequestKind::Questions { .. }) {
                            crate::native_questions::reconcile(&app, project, &request);
                            continue;
                        }
                        if !request.origin.known
                            && !request.delivered
                            && let Ok(listing) = crate::sessions_state::list(&app, project)
                            && let Some(origin) =
                                listing.rows.into_iter().map(|r| r.record).find(|r| {
                                    r.known
                                        && (r.session == request.origin.session
                                            && r.session.is_some_and(|session| {
                                                crate::conversations::live(&app, r) == Some(session)
                                            })
                                            || r.run == request.origin.run && r.run.is_some())
                                })
                            && let Ok(updated) = requests.resolve_origin(&root, &request.id, origin)
                        {
                            request = updated;
                            broadcast(&app, project);
                        }

                        if request.applying {
                            if let Err(error) = apply_response(&app, project, &request) {
                                let _ =
                                    requests.delivery_error(&root, &request.id, error.to_string());
                            }
                            broadcast(&app, project);
                            continue;
                        }
                        if request.response.is_none()
                            || request.delivered
                            || requests.has_waiter(&request.id)
                        {
                            continue;
                        }
                        if app
                            .try_state::<Arc<AgentRegistry>>()
                            .is_some_and(|agents| !agents.dispatching(project))
                        {
                            continue;
                        }
                        match deliver(&app, project, &request) {
                            Ok(true) => {
                                if requests.get(&root, &request.id).is_ok_and(|r| !r.delivered) {
                                    let _ = requests.delivered(&root, &request.id);
                                }
                                broadcast(&app, project);
                            }
                            Ok(false) => {
                                if requests
                                    .get(&root, &request.id)
                                    .is_ok_and(|r| r.delivery_error != request.delivery_error)
                                {
                                    broadcast(&app, project);
                                }
                            }
                            Err(error)
                                if request.delivery_error.as_deref()
                                    != Some(&error.to_string()) =>
                            {
                                let _ =
                                    requests.delivery_error(&root, &request.id, error.to_string());
                                broadcast(&app, project);
                            }
                            Err(_) => {}
                        }
                    }
                }
            }
        })
        .expect("user request delivery thread");
}

fn deliver(app: &AppHandle, project: ProjectId, request: &UserRequest) -> Result<bool> {
    if matches!(request.request, UserRequestKind::Questions { .. }) {
        return Ok(false);
    }
    store(app)?.deliver_answer(
        &root(app, project)?,
        &request.id,
        &ConversationDelivery { app, project },
    )
}

fn answer_marker(request: &UserRequest) -> String {
    format!("The user resolved request {}:", request.id)
}

fn answer_line(request: &UserRequest) -> String {
    let result = result_value(request);
    format!(
        "{} {}. Continue this conversation using this decision. Do not ask the same question again; rejection or cancellation does not authorize the proposed work.",
        answer_marker(request),
        result
    )
}

struct ConversationDelivery<'a> {
    app: &'a AppHandle,
    project: ProjectId,
}

impl AnswerTarget for ConversationDelivery<'_> {
    fn received(&self, request: &UserRequest) -> Result<bool> {
        crate::conversations::received_answer(
            self.app,
            self.project,
            &request.origin,
            &answer_marker(request),
            request,
            &store(self.app)?.tool_reply_turns(&request.id),
        )
    }

    fn submit(
        &self,
        request: &UserRequest,
        begin: &dyn Fn() -> Result<bool>,
    ) -> Result<AnswerSubmission> {
        let (app, project) = (self.app, self.project);
        let origin = &request.origin;
        let line = answer_line(request);
        if !origin.known {
            return Ok(AnswerSubmission::Waiting);
        }
        if origin.run.is_some() {
            return app
                .state::<Arc<AgentRegistry>>()
                .deliver_user_request(app, project, origin, &line, begin);
        }
        // The journal's old PTY id is not sufficient after /new, /clear or /resume.
        if let Some(session) = crate::conversations::live(app, origin)
            && let Some(pty) = app
                .try_state::<SessionRegistry>()
                .and_then(|sessions| sessions.get(session))
                .filter(|pty| !pty.has_exited())
        {
            if origin.harness == cide_ipc::Harness::Opencode
                && let Some(console) = crate::sessions_state::handle(app).and_then(|sessions| {
                    sessions
                        .conversations
                        .consoles
                        .get(&session)
                        .map(|c| c.clone())
                })
            {
                if console.conversation().id != origin.id
                    || !console.peer_ready().map_err(CoreError::Io)?
                    || !begin()?
                {
                    return Ok(AnswerSubmission::Waiting);
                }
                console
                    .prompt_conversation(&origin.id, &line)
                    .map_err(CoreError::Io)?;
                return Ok(AnswerSubmission::Confirmed);
            }
            if origin.harness == cide_ipc::Harness::Codex
                && let Some(console) = app
                    .try_state::<crate::peer_chat::Runtime>()
                    .and_then(|runtime| runtime.codex_console(session))
            {
                let Some(receiver) = console.receiver().map_err(CoreError::Io)? else {
                    return Ok(AnswerSubmission::Waiting);
                };
                if receiver.thread != origin.id || !begin()? {
                    return Ok(AnswerSubmission::Waiting);
                }
                let delivery_app = app.clone();
                let pinned = origin.clone();
                let valid = Box::new(move || {
                    (crate::conversations::live(&delivery_app, &pinned) == Some(session))
                        .then_some(())
                        .ok_or_else(|| {
                            "The asking conversation changed; the answer is saved.".into()
                        })
                });
                return match console.peer_prompt(receiver, line, valid) {
                    Ok(true) => Ok(AnswerSubmission::Confirmed),
                    Ok(false) => Ok(AnswerSubmission::Waiting),
                    Err((false, error)) => Ok(AnswerSubmission::NotSent(error)),
                    Err((true, error)) => Err(CoreError::Io(error)),
                };
            }
            let state = crate::emit::last_session_states()
                .get(&session)
                .copied()
                .or_else(|| {
                    app.try_state::<crate::hooks::HookServer>()
                        .map(|hooks| hooks.state(session))
                });
            if !matches!(
                state,
                Some(cide_ipc::SessionState::Idle | cide_ipc::SessionState::AwaitingInput)
            ) || !cide_core::conversations::composer(origin.harness, &pty.capture_screen())
                .is_some_and(|body| body.trim().is_empty())
            {
                return Ok(AnswerSubmission::Waiting);
            }
            if !begin()? {
                return Ok(AnswerSubmission::Waiting);
            }
            crate::peer_chat::submit_mobile(app, session, &pty, origin.harness, &line)
                .map_err(CoreError::Io)?;
            // An Enter is not a receipt. The next delivery pass checks the transcript.
            return Ok(AnswerSubmission::Submitted);
        }
        if !origin.known || !origin.cwd.is_dir() {
            return Err(CoreError::Io("The original conversation or its working directory is unavailable. The answer is saved.".into()));
        }
        if !crate::conversations::resume_enabled(app, project, origin.harness) {
            return Err(CoreError::Io("Restoring the asking conversation is disabled for this harness. The answer is saved.".into()));
        }
        let harness = cide_agents::tools::SESSION_HARNESSES
            .iter()
            .find(|h| **h == origin.harness)
            .ok_or_else(|| CoreError::Io("This harness has no resumable console.".into()))?;
        let tab_harness = match harness {
            cide_ipc::Harness::Claude => {
                crate::claude_tab::TabHarness::Console(cide_ipc::ConsoleHarness::Claude)
            }
            cide_ipc::Harness::Codex => {
                crate::claude_tab::TabHarness::Console(cide_ipc::ConsoleHarness::Codex)
            }
            _ => crate::claude_tab::TabHarness::Opencode,
        };
        let mode = cide_agents::config::load(&root(app, project)?)
            .agents
            .unattended();
        if !begin()? {
            return Ok(AnswerSubmission::Waiting);
        }
        crate::claude_tab::open_continuing(
            app,
            project,
            crate::claude_tab::Open {
                title: &origin.title,
                session_name: None,
                prompt: &line,
                mode: crate::claude_tab::TabMode {
                    unattended: mode,
                    behind: true,
                },
                cwd: Some(origin.cwd.clone()),
                harness: Some(tab_harness),
                model: None,
                voice: match origin.kind {
                    cide_ipc::sessions::SessionKind::Worker => crate::cmd::session::Voice::Worker,
                    cide_ipc::sessions::SessionKind::Planner
                    | cide_ipc::sessions::SessionKind::Reviewer => {
                        crate::cmd::session::Voice::Acting
                    }
                    _ => crate::cmd::session::Voice::Pane,
                },
                origin: match origin.kind {
                    cide_ipc::sessions::SessionKind::Planner => Some(cide_ipc::PaneOrigin::Planner),
                    cide_ipc::sessions::SessionKind::Worker => Some(cide_ipc::PaneOrigin::Worker),
                    cide_ipc::sessions::SessionKind::Reviewer => {
                        Some(cide_ipc::PaneOrigin::Reviewer)
                    }
                    _ => None,
                },
            },
            Some(cide_ipc::HarnessSession {
                harness: origin.harness,
                id: origin.id.clone(),
                cwd: origin.cwd.clone(),
            }),
        )?;
        Ok(AnswerSubmission::Submitted)
    }
}
