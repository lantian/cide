//! Resolve a native tool call through the existing durable Waiting decisions.
use cide_core::{persist, user_requests::Requests};
use cide_ipc::{
    Harness, NativeQuestionSource, ProjectId, RunId, TaskAuthor, UserRequest, UserRequestKind,
    UserRequestQuestion, UserRequestResponse,
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::AppHandle;
use tauri::Manager;

#[derive(Default)]
pub(crate) struct Runtime {
    active: parking_lot::Mutex<std::collections::HashMap<String, usize>>,
    claude:
        parking_lot::Mutex<std::collections::HashMap<(cide_ipc::SessionId, String), Arc<Question>>>,
    qwen:
        parking_lot::Mutex<std::collections::HashMap<(cide_ipc::SessionId, String), Arc<Question>>>,
}

pub(crate) fn qwen_event(
    app: &AppHandle,
    session: cide_ipc::SessionId,
    value: &serde_json::Value,
    pty: &Arc<cide_pty::PtySession>,
) {
    use cide_agents::native_questions::{self, Dialect};
    let Some(runtime) = app.try_state::<Runtime>() else {
        return;
    };
    if value["type"] == "user" && value["parent_tool_use_id"].is_null() {
        for result in value["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_result")
        {
            if let Some(call) = result["tool_use_id"].as_str()
                && let Some(question) = runtime.qwen.lock().remove(&(session, call.into()))
            {
                question.finish();
            }
        }
    }
    if value["type"] != "control_request"
        || value["request"]["tool_name"] != "ask_user_question"
        || !value["parent_tool_use_id"].is_null()
    {
        return;
    }
    let Some(plan) = crate::native_plan::get(app, session).filter(|p| p.pending() && p.valid())
    else {
        return;
    };
    let Some(call) = value["request"]["tool_use_id"].as_str() else {
        return;
    };
    let key = (session, call.to_string());
    if runtime.qwen.lock().contains_key(&key) {
        return;
    }
    let Some(conversation) = plan.conversation() else {
        return;
    };
    let opened =
        native_questions::questions(&value["request"]["input"]["questions"], Dialect::Qwen)
            .and_then(|questions| plan.ask(&conversation, call, questions));
    let question = match opened {
        Ok(q) => Arc::new(q),
        Err(error) => {
            tracing::warn!(%session, %error, "could not route the native Qwen question");
            return;
        }
    };
    runtime.qwen.lock().insert(key, Arc::clone(&question));
    let app = app.clone();
    let pty = Arc::downgrade(pty);
    let _ = std::thread::Builder::new()
        .name("cide-qwen-question".into())
        .spawn(move || {
            while question.active() {
                let Some(pty) = pty.upgrade().filter(|p| !p.has_exited()) else {
                    break;
                };
                if let Some(response) = question.response() {
                    if !question.valid() {
                        continue;
                    }
                    if question.claim() {
                        let result = crate::peer_chat::native_input(&app, session, || {
                            answer_qwen(&question, &pty, &response)
                        });
                        if let Err(error) = result {
                            question.error(&error);
                        }
                    }
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
}

fn answer_qwen(
    question: &Question,
    pty: &cide_pty::PtySession,
    response: &UserRequestResponse,
) -> Result<(), String> {
    use cide_agents::native_questions::{answer_values, qwen_dialog};
    use std::time::{Duration, Instant};
    let dialog = || {
        if !question.valid() || pty.has_exited() {
            return Err("The native question ended before delivery".to_string());
        }
        qwen_dialog(&pty.capture_screen(), &question.questions).ok_or_else(|| "The native question dialog changed or is clipped. Open the original conversation to finish answering.".into())
    };
    let wait_for = |index: usize, selected: usize| -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(current) = dialog()
                && current.question == index
                && current.selected == selected
            {
                return Ok(());
            }
            if !question.valid() || Instant::now() >= deadline {
                return Err("The native dialog did not acknowledge navigation. Open the original conversation.".into());
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    };
    let Some(answers) = answer_values(&question.questions, response) else {
        dialog()?;
        pty.write(vec![27]);
        question.cancel_turn();
        return Ok(());
    };
    for (index, values) in answers.iter().enumerate() {
        let text = values.join(", ");
        if text.is_empty()
            || text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return Err("The native dialog cannot safely receive this answer. Open the original conversation.".into());
        }
        let mut current = dialog()?;
        if current.question != index {
            return Err("The native dialog was already edited. Open the original conversation to finish answering.".into());
        }
        let target = question.questions[index].question.options().len();
        while current.selected < target {
            let screen = pty.capture_screen();
            pty.write(if screen.info.app_cursor {
                b"\x1bOB".to_vec()
            } else {
                b"\x1b[B".to_vec()
            });
            wait_for(index, current.selected + 1)?;
            current = dialog()?;
        }
        if !current.empty_custom {
            return Err("The native answer contains a draft. Open the original conversation to finish answering.".into());
        }
        let capture = pty.capture_screen();
        if text.contains('\n') && !capture.info.bracketed_paste {
            return Err("This native dialog cannot safely accept multiline text. Open the original conversation.".into());
        }
        pty.write(cide_remote::keys::paste(&text, &capture.info));
        // Qwen's own answer payload is one string per question, including for multiple
        // choices. Its custom input accepts the same joined value without numeric-key
        // auto-submit or checkbox races.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pasted = dialog()?;
            if pasted.question != index || pasted.selected != target {
                return Err("The native question changed while pasting".into());
            }
            if !pasted.empty_custom {
                break;
            }
            if Instant::now() >= deadline {
                return Err("The native input did not acknowledge the answer".into());
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        dialog()?;
        pty.write(vec![b'\r']);
        if question.questions.len() > 1 {
            wait_for(index + 1, 0)?;
        }
    }
    if question.questions.len() > 1 {
        let current = dialog()?;
        if current.question != question.questions.len() || current.selected != 0 {
            return Err("The native answer summary changed before submission".into());
        }
        pty.write(vec![b'\r']);
    }
    Ok(())
}

pub(crate) fn observe_hook(app: &AppHandle, frame: &cide_claude::HookFrame) {
    let Some(session) = frame.owner().and_then(|s| s.parse().ok()) else {
        return;
    };
    let Some(runtime) = app.try_state::<Runtime>() else {
        return;
    };
    if matches!(frame.event.as_str(), "Interrupt" | "SessionEnd") {
        runtime
            .claude
            .lock()
            .retain(|(owner, _), _| *owner != session);
    } else if matches!(frame.event.as_str(), "PostToolUse" | "PostToolUseFailure")
        && frame.payload["tool_name"] == "AskUserQuestion"
        && let Some(id) = frame.payload["tool_use_id"].as_str()
        && let Some(question) = runtime.claude.lock().remove(&(session, id.into()))
    {
        if frame.event == "PostToolUse" {
            question.finish();
        } else {
            question.retire("The native question tool failed before accepting the answer.");
        }
    }
}

pub(crate) fn claude_hook(
    app: &AppHandle,
    frame: &cide_claude::HookFrame,
    stream: &std::os::unix::net::UnixStream,
) {
    use cide_agents::native_questions::{self, Dialect};
    use std::{
        io::{Read, Write},
        time::Duration,
    };
    // The invocation-local hook stays installed after the native plan has been
    // accepted. Outside its planning lifecycle it returns no decision, letting
    // the harness's own question UI run normally.
    let planning = frame
        .owner()
        .and_then(|s| s.parse().ok())
        .and_then(|session| crate::native_plan::get(app, session))
        .is_some_and(|plan| plan.pending());
    if !planning {
        let mut connection = stream;
        let _ = writeln!(connection, "{{}}");
        return;
    }
    let work = || -> Result<(), String> {
        let session = frame
            .owner()
            .and_then(|s| s.parse().ok())
            .ok_or("Missing worker session")?;
        let plan = crate::native_plan::get(app, session).ok_or("This is not a planning worker")?;
        if frame.payload["tool_name"] != "AskUserQuestion" {
            return Err("Not a native question".into());
        }
        let conversation = frame.payload["session_id"]
            .as_str()
            .ok_or("Missing conversation")?;
        let call = frame.payload["tool_use_id"]
            .as_str()
            .ok_or("Missing native tool call")?;
        let questions = native_questions::questions(
            &frame.payload["tool_input"]["questions"],
            Dialect::Claude,
        )?;
        let question = Arc::new(plan.ask(conversation, call, questions)?);
        let mut connection = stream.try_clone().map_err(|e| e.to_string())?;
        connection
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| e.to_string())?;
        connection
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;
        while question.active() {
            if let Some(response) = question.response() {
                if !question.valid() {
                    continue;
                }
                if !question.claim() {
                    return Err("The native answer was already submitted. Inspect the original conversation.".into());
                }
                let reply = native_questions::claude_reply(
                    &frame.payload["tool_input"],
                    &question.questions,
                    &response,
                );
                let cancelled = !matches!(response, UserRequestResponse::Answers { .. });
                if !cancelled {
                    let runtime = app.try_state::<Runtime>().ok_or("Cide is shutting down")?;
                    runtime
                        .claude
                        .lock()
                        .insert((session, call.into()), Arc::clone(&question));
                }
                if let Err(error) = writeln!(connection, "{reply}") {
                    if let Some(runtime) = app.try_state::<Runtime>() {
                        runtime.claude.lock().remove(&(session, call.into()));
                    }
                    return Err(error.to_string());
                }
                if cancelled {
                    question.finish();
                    question.cancel_turn();
                }
                return Ok(());
            }
            let mut byte = [0];
            match connection.read(&mut byte) {
                Ok(0) => return Ok(()),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e.to_string()),
                _ => {}
            }
        }
        Err("The original planning operation ended".into())
    };
    if let Err(error) = work() {
        let mut connection = stream;
        let reply = json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":error}});
        let _ = writeln!(connection, "{reply}");
    }
}

pub(crate) struct Question {
    app: AppHandle,
    project: ProjectId,
    root: PathBuf,
    store: Arc<Requests>,
    plan: crate::native_plan::Handle,
    pub source: NativeQuestionSource,
    pub questions: Vec<UserRequestQuestion>,
    id: String,
    closed: AtomicBool,
}

impl Question {
    pub fn open(
        app: &AppHandle,
        project: ProjectId,
        plan: crate::native_plan::Handle,
        run: Option<RunId>,
        harness: Harness,
        source: NativeQuestionSource,
        questions: Vec<UserRequestQuestion>,
    ) -> Result<Self, String> {
        let root = crate::user_requests::root(app, project).map_err(|e| e.to_string())?;
        let store = crate::user_requests::store(app).map_err(|e| e.to_string())?;
        let now = persist::now_ms();
        let mut origin = crate::sessions_state::list(app, project)
            .ok()
            .and_then(|listing| {
                listing
                    .rows
                    .into_iter()
                    .map(|r| r.record)
                    .filter(|r| r.session == Some(source.session) || run.is_some() && r.run == run)
                    .max_by_key(|r| (r.id == source.conversation, r.known, r.last_seen_unix_ms))
            })
            .unwrap_or_else(|| {
                serde_json::from_value(json!({
            "id":source.conversation,"known":true,"kind":if run.is_some(){"subagent"}else{"worker"},
            "session":source.session,"run":run,"harness":harness,"cwd":root,
            "title":"Planning worker","startedUnixMs":now,"lastSeenUnixMs":now
        })).expect("native question origin")
            });
        origin.id = source.conversation.clone();
        origin.known = true;
        origin.session = Some(source.session);
        origin.harness = harness;
        let title = questions
            .first()
            .ok_or("No questions")?
            .question
            .text()
            .chars()
            .take(100)
            .collect();
        let author = origin
            .agent
            .clone()
            .map(|agent| TaskAuthor::Agent {
                agent: cide_ipc::AgentId(agent),
                label: origin.title.clone(),
            })
            .unwrap_or(TaskAuthor::Orchestrator);
        let runtime = app.try_state::<Runtime>().ok_or("Cide is shutting down")?;
        let mut active = runtime.active.lock();
        let request = store
            .create(
                &root,
                UserRequest {
                    id: uuid::Uuid::new_v4().to_string(),
                    title,
                    message: questions
                        .iter()
                        .map(|q| q.question.text())
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                    // Native batches do not change a task's independent legacy question field.
                    task: origin.task.clone(),
                    author,
                    origin,
                    wait: true,
                    created_unix_ms: now,
                    request: UserRequestKind::Questions {
                        source: source.clone(),
                        questions: questions.clone(),
                    },
                    response: None,
                    applying: false,
                    delivered: false,
                    delivery_error: None,
                },
            )
            .map_err(|e| e.to_string())?;
        if let UserRequestKind::Questions {
            questions: original,
            ..
        } = &request.request
            && original != &questions
        {
            return Err("The native question changed after it was displayed. Answer it in the original conversation.".into());
        }
        *active.entry(request.id.clone()).or_default() += 1;
        drop(active);
        crate::user_requests::broadcast(app, project);
        plan.state(cide_ipc::SessionState::AwaitingInput);
        Ok(Self {
            app: app.clone(),
            project,
            root,
            store,
            plan,
            source,
            questions,
            id: request.id,
            closed: AtomicBool::new(request.delivered),
        })
    }
    pub fn valid(&self) -> bool {
        self.active() && self.plan.valid()
    }
    pub fn active(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && self.plan.owns_question(self.source.generation)
    }
    pub fn response(&self) -> Option<UserRequestResponse> {
        self.valid()
            .then(|| {
                self.store
                    .get(&self.root, &self.id)
                    .ok()
                    .filter(|r| !r.delivered)
                    .and_then(|r| r.response)
            })
            .flatten()
    }
    pub fn claim(&self) -> bool {
        self.valid()
            && self
                .store
                .claim_native(&self.root, &self.id)
                .unwrap_or(false)
    }
    pub fn finish(&self) {
        if self.store.delivered(&self.root, &self.id).is_ok() {
            self.closed.store(true, Ordering::Release);
            self.plan.state(cide_ipc::SessionState::Busy);
            crate::user_requests::broadcast(&self.app, self.project);
        }
    }
    pub fn error(&self, error: &str) {
        if self
            .store
            .get(&self.root, &self.id)
            .is_ok_and(|r| r.delivery_error.as_deref() != Some(error))
        {
            let _ = self
                .store
                .delivery_error(&self.root, &self.id, error.into());
            crate::user_requests::broadcast(&self.app, self.project);
        }
    }
    pub fn retire(&self, reason: &str) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let _ = self.store.retire_native(&self.root, &self.id, reason);
            crate::user_requests::broadcast(&self.app, self.project);
        }
    }
    pub fn cancel_turn(&self) {
        self.plan.interrupt_question();
    }
}
impl Drop for Question {
    fn drop(&mut self) {
        if let Some(runtime) = self.app.try_state::<Runtime>() {
            let mut active = runtime.active.lock();
            if let Some(count) = active.get_mut(&self.id) {
                *count -= 1;
                if *count > 0 {
                    return;
                }
            }
            active.remove(&self.id);
        }
        self.retire("The native question is no longer pending in this worker.");
    }
}

/// Restored requests cannot fall through to chat delivery. A fresh native event must
/// identify the same generation and pending call before an adapter may claim an answer.
pub(crate) fn reconcile(app: &AppHandle, project: ProjectId, request: &UserRequest) {
    let UserRequestKind::Questions { source, .. } = &request.request else {
        return;
    };
    if request.delivered {
        return;
    }
    if let Some(runtime) = app.try_state::<Runtime>() {
        runtime.claude.lock().retain(|_, q| q.active());
        runtime.qwen.lock().retain(|_, q| q.active());
    }
    let obsolete = !crate::native_plan::get(app, source.session)
        .is_some_and(|plan| plan.owns_question(source.generation))
        || !app
            .try_state::<Runtime>()
            .is_some_and(|runtime| runtime.active.lock().contains_key(&request.id));
    if obsolete
        && let (Ok(root), Ok(store)) = (
            crate::user_requests::root(app, project),
            crate::user_requests::store(app),
        )
    {
        let _ = store.retire_native(&root, &request.id, "The original planning operation ended. This answer will not be sent to a different run.");
        crate::user_requests::broadcast(app, project);
    }
}
