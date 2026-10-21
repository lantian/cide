//! Cide owns the native plan-approval transition for delegated workers only.
//! Ordinary consoles, questions and tool permissions never enter this controller.
use cide_agents::native_plan::{Phase, State};
use cide_ipc::{Harness, HarnessSession, ProjectId, RunId, SessionId};
use cide_pty::PtySession;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

#[derive(Clone, Serialize, Deserialize)]
struct SavedWorker {
    conversation: HarnessSession,
    state: State,
    execution: String,
}

#[derive(Default)]
pub(crate) struct Runtime {
    active: Mutex<HashMap<SessionId, Weak<Worker>>>,
    saved: Mutex<Option<Vec<SavedWorker>>>,
}

#[derive(Clone)]
pub(crate) struct Handle(Arc<Worker>);
struct Worker {
    app: AppHandle,
    project: Option<ProjectId>,
    session: SessionId,
    run: Option<RunId>,
    harness: Harness,
    state: Mutex<State>,
    execution: String,
    conversation: Mutex<Option<HarnessSession>>,
    cancelled: AtomicBool,
    plan_exit: AtomicBool,
    ready: AtomicBool,
    request: Mutex<Option<String>>,
    accepting: AtomicBool,
}

fn path() -> std::path::PathBuf {
    cide_core::persist::state_dir().join("native-worker-plans.json")
}

pub(crate) fn claude_args(args: &[String], execution: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--settings=") {
            result.extend(["--settings".into(), value.into()]);
            continue;
        }
        if let Some(value) = arg
            .strip_prefix("--allowedTools=")
            .or_else(|| arg.strip_prefix("--allowed-tools="))
        {
            result.extend(["--allowedTools".into(), value.into()]);
            continue;
        }
        if arg == "--permission-mode" {
            args.next();
            continue;
        }
        if arg.starts_with("--permission-mode=")
            || matches!(
                arg.as_str(),
                "--dangerously-skip-permissions" | "--allow-dangerously-skip-permissions"
            )
        {
            continue;
        }
        result.push(arg.clone());
    }
    result.extend(["--permission-mode".into(), "plan".into()]);
    if execution == "bypassPermissions" {
        result.push("--allow-dangerously-skip-permissions".into());
    }
    // Native plan mode can ask about MCP calls even though these two tools only read.
    // Preapprove task discovery without approving task mutations or arbitrary tools.
    let reads = [
        "mcp__cide__cide_task_get".to_string(),
        "mcp__cide__cide_task_list".to_string(),
    ];
    if let Some(index) = result
        .iter()
        .position(|arg| arg == "--allowedTools" || arg == "--allowed-tools")
    {
        let end = result[index + 1..]
            .iter()
            .position(|arg| arg.starts_with('-'))
            .map_or(result.len(), |end| index + 1 + end);
        result.splice(end..end, reads);
    } else {
        result.push("--allowedTools".into());
        result.extend(reads);
    }
    // Only delegated Plan-mode invocations get a blocking question hook. Ordinary
    // telemetry hooks retain their short, fire-and-forget socket contract.
    for index in 0..result.len().saturating_sub(1) {
        if result[index] != "--settings" {
            continue;
        }
        let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&result[index + 1]) else {
            continue;
        };
        let Some(matchers) = settings["hooks"]["PreToolUse"].as_array_mut() else {
            continue;
        };
        let command = matchers
            .iter()
            .flat_map(|m| m["hooks"].as_array().into_iter().flatten())
            .filter_map(|h| h["command"].as_str())
            .find(|c| c.contains("cide-hook") && c.ends_with(" PreToolUse"))
            .map(|c| format!("{} native-question", c.trim_end_matches(" PreToolUse")));
        if let Some(command) = command {
            matchers.retain(|matcher| {
                !matcher["hooks"]
                    .as_array()
                    .is_some_and(|hooks| hooks.iter().any(|hook| hook["command"] == command))
            });
            matchers.push(serde_json::json!({"matcher":"^AskUserQuestion$","hooks":[{"type":"command","command":command,"timeout":86400,"onFailure":"block"}]}));
            result[index + 1] = settings.to_string();
        }
    }
    result
}

pub(crate) fn claude_observable(args: &[String]) -> bool {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--bare" | "--safe-mode"))
    {
        return false;
    }
    args.iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            arg.strip_prefix("--settings=").or_else(|| {
                (arg == "--settings")
                    .then(|| args.get(index + 1).map(String::as_str))
                    .flatten()
            })
        })
        .filter_map(|settings| serde_json::from_str::<serde_json::Value>(settings).ok())
        .any(|settings| {
            ["PreToolUse", "PostToolUse"].into_iter().all(|event| {
                settings["hooks"][event].as_array().is_some_and(|matchers| {
                    matchers.iter().any(|matcher| {
                        matcher["hooks"].as_array().is_some_and(|hooks| {
                            hooks.iter().any(|hook| {
                                hook["command"]
                                    .as_str()
                                    .is_some_and(|command| command.contains("cide-hook"))
                            })
                        })
                    })
                })
            })
        })
}

pub(crate) fn get(app: &AppHandle, session: SessionId) -> Option<Handle> {
    app.try_state::<Runtime>()?
        .active
        .lock()
        .get(&session)
        .and_then(Weak::upgrade)
        .map(Handle)
}

/// Workers restored from before this feature have no saved state and retain their behavior.
pub(crate) fn restored(app: &AppHandle, conversation: &HarnessSession) -> Option<(State, String)> {
    let runtime = app.try_state::<Runtime>()?;
    let mut saved = runtime.saved.lock();
    let records = saved.get_or_insert_with(|| {
        std::fs::read(path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    });
    records
        .iter()
        .rev()
        .find(|record| record.conversation == *conversation)
        .map(|record| (record.state.clone(), record.execution.clone()))
}

pub(crate) fn register(
    app: &AppHandle,
    project: Option<ProjectId>,
    session: SessionId,
    run: Option<RunId>,
    harness: Harness,
    state: State,
    execution: String,
) -> Handle {
    let handle = Handle(Arc::new(Worker {
        app: app.clone(),
        project,
        session,
        run,
        harness,
        state: Mutex::new(state),
        execution,
        conversation: Mutex::new(None),
        cancelled: AtomicBool::new(false),
        plan_exit: AtomicBool::new(false),
        ready: AtomicBool::new(false),
        request: Mutex::new(None),
        accepting: AtomicBool::new(false),
    }));
    if let Some(runtime) = app.try_state::<Runtime>() {
        let old = {
            let mut active = runtime.active.lock();
            active.retain(|_, worker| worker.strong_count() != 0);
            active
                .insert(session, Arc::downgrade(&handle.0))
                .and_then(|worker| worker.upgrade())
        };
        if let Some(old) = old {
            Handle(old).cancel();
        }
    }
    handle
}

impl Handle {
    pub fn owns_question(&self, generation: SessionId) -> bool {
        self.pending()
            && self.generation() == generation
            && get(&self.0.app, self.0.session).is_some_and(|h| Arc::ptr_eq(&h.0, &self.0))
    }
    pub fn conversation(&self) -> Option<String> {
        self.0.conversation.lock().as_ref().map(|c| c.id.clone())
    }
    pub fn interrupt_question(&self) {
        self.abandon();
        if let Some(sessions) = self.0.app.try_state::<crate::state::SessionRegistry>()
            && let Some(pty) = sessions.get(self.0.session)
        {
            pty.write(vec![3]);
        }
    }
    pub fn generation(&self) -> SessionId {
        self.0.state.lock().generation
    }
    pub fn ask(
        &self,
        conversation: &str,
        request: &str,
        questions: Vec<cide_ipc::UserRequestQuestion>,
    ) -> Result<crate::native_questions::Question, String> {
        if !self.pending() || !self.valid() {
            return Err("This worker is no longer planning".into());
        }
        if self
            .conversation()
            .as_deref()
            .is_some_and(|id| id != conversation)
        {
            return Err("The worker changed conversation".into());
        }
        crate::native_questions::Question::open(
            &self.0.app,
            self.0.project.ok_or("The worker has no project")?,
            self.clone(),
            self.0.run,
            self.0.harness,
            cide_ipc::NativeQuestionSource {
                session: self.0.session,
                generation: self.generation(),
                conversation: conversation.into(),
                request: request.into(),
            },
            questions,
        )
    }
    pub fn phase(&self) -> Phase {
        self.0.state.lock().phase
    }
    pub fn pending(&self) -> bool {
        self.0.state.lock().pending() && !self.0.cancelled.load(Ordering::SeqCst)
    }
    pub fn execution(&self) -> &str {
        &self.0.execution
    }
    pub fn valid(&self) -> bool {
        !self.0.cancelled.load(Ordering::SeqCst)
            && self.0.run.is_none_or(|run| {
                self.0
                    .app
                    .try_state::<Arc<crate::agents::AgentRegistry>>()
                    .is_some_and(|registry| {
                        registry.planning_active(
                            run,
                            self.0.session,
                            self.0.state.lock().generation,
                        )
                    })
            })
            && crate::emit::last_session_states().get(&self.0.session)
                != Some(&cide_ipc::SessionState::Paused)
            && get(&self.0.app, self.0.session)
                .is_some_and(|current| Arc::ptr_eq(&current.0, &self.0))
    }
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        if let Some(runtime) = self.0.app.try_state::<Runtime>() {
            let mut active = runtime.active.lock();
            if active
                .get(&self.0.session)
                .and_then(Weak::upgrade)
                .is_some_and(|current| Arc::ptr_eq(&current, &self.0))
            {
                active.remove(&self.0.session);
            }
        }
    }
    pub fn restart(&self, state: State) {
        *self.0.state.lock() = state;
        self.0.plan_exit.store(false, Ordering::SeqCst);
        self.0.accepting.store(false, Ordering::SeqCst);
        *self.0.request.lock() = None;
        self.save();
    }
    pub fn abandon(&self) {
        if let Some(run) = self.0.run
            && let Some(registry) = self.0.app.try_state::<Arc<crate::agents::AgentRegistry>>()
        {
            registry.native_state(
                &self.0.app,
                run,
                self.0.session,
                cide_ipc::RunState::Interrupted,
            );
        }
        self.cancel();
    }
    pub fn transition(&self, phase: Phase) {
        if !self.valid() {
            return;
        }
        let state = {
            let mut state = self.0.state.lock();
            state.phase = phase;
            state.clone()
        };
        if let Some(run) = self.0.run
            && let Some(registry) = self.0.app.try_state::<Arc<crate::agents::AgentRegistry>>()
        {
            registry.note_planning(run, self.0.session, &state);
        }
        self.save();
    }
    pub fn bind(&self, conversation: HarnessSession) {
        *self.0.conversation.lock() = Some(conversation);
        self.save();
    }
    fn save(&self) {
        if self.0.run.is_some() {
            return;
        }
        let Some(conversation) = self.0.conversation.lock().clone() else {
            return;
        };
        let Some(runtime) = self.0.app.try_state::<Runtime>() else {
            return;
        };
        let record = SavedWorker {
            conversation,
            state: self.0.state.lock().clone(),
            execution: self.0.execution.clone(),
        };
        let mut saved = runtime.saved.lock();
        let records = saved.get_or_insert_with(|| {
            std::fs::read(path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default()
        });
        records.retain(|old| old.conversation != record.conversation);
        records.push(record);
        if records.len() > 512 {
            records.drain(..records.len() - 512);
        }
        if let Ok(bytes) = serde_json::to_vec(records)
            && let Err(error) = cide_core::persist::write_atomic(&path(), &bytes)
        {
            tracing::warn!(%error, "could not persist worker planning state");
        }
    }
    pub fn state(&self, state: cide_ipc::SessionState) {
        if !self.valid() {
            return;
        }
        if let Some(run) = self.0.run
            && let Some(registry) = self.0.app.try_state::<Arc<crate::agents::AgentRegistry>>()
        {
            let next = match state {
                cide_ipc::SessionState::AwaitingPermission => {
                    cide_ipc::RunState::AwaitingPermission
                }
                cide_ipc::SessionState::AwaitingInput => cide_ipc::RunState::AwaitingPermission,
                cide_ipc::SessionState::Idle => cide_ipc::RunState::Idle,
                _ => cide_ipc::RunState::Running,
            };
            registry.native_state(&self.0.app, run, self.0.session, next);
        }
    }
    pub fn fail(&self, reason: &str) {
        tracing::error!(session=%self.0.session, %reason, "native planning failed");
        if let Some(workspace) = self
            .0
            .app
            .try_state::<crate::workspace_state::WorkspaceState>()
        {
            let project = workspace.with(|ws| {
                ws.projects
                    .iter()
                    .find(|(_, project)| {
                        project
                            .tabs
                            .iter()
                            .flat_map(|tab| tab.tree.panes.values())
                            .chain(project.detached.values())
                            .any(|pane| pane.session == Some(self.0.session))
                    })
                    .map(|(id, _)| *id)
            });
            if let Some(project) = project {
                // This existing project warning bus is also used by the native permission preflight.
                crate::emit::pool_notice(
                    &self.0.app,
                    project,
                    format!("Native plan approval failed: {reason}"),
                );
            }
        }
        if let Some(run) = self.0.run
            && let Some(registry) = self.0.app.try_state::<Arc<crate::agents::AgentRegistry>>()
        {
            registry.native_state(
                &self.0.app,
                run,
                self.0.session,
                cide_ipc::RunState::Failed {
                    reason: reason.into(),
                },
            );
        }
        self.cancel();
        if let Some(sessions) = self.0.app.try_state::<crate::state::SessionRegistry>()
            && let Some(pty) = sessions.get(self.0.session)
        {
            pty.kill();
        }
    }
    pub fn observe(&self, value: &serde_json::Value) {
        if !self.pending() || !value["parent_tool_use_id"].is_null() {
            return;
        }
        if value["type"] == "system" && value["subtype"] == "session_start" {
            self.0.ready.store(true, Ordering::SeqCst);
            if let Some(id) = value["session_id"]
                .as_str()
                .or_else(|| value["data"]["session_id"].as_str())
            {
                let cwd = value["cwd"]
                    .as_str()
                    .or_else(|| value["data"]["cwd"].as_str())
                    .unwrap_or(".");
                if self.conversation().as_deref().is_some_and(|old| old != id) {
                    self.abandon();
                    return;
                }
                self.bind(HarnessSession {
                    harness: self.0.harness,
                    id: id.into(),
                    cwd: cwd.into(),
                });
            }
        }
        let tool = value["tool_name"]
            .as_str()
            .or_else(|| value["request"]["tool_name"].as_str());
        if matches!(tool, Some("ExitPlanMode" | "exit_plan_mode")) {
            self.0.plan_exit.store(true, Ordering::SeqCst);
            *self.0.request.lock() = value["request"]["tool_use_id"]
                .as_str()
                .or_else(|| value["tool_use_id"].as_str())
                .map(str::to_string);
        }
        let result = self
            .0
            .request
            .lock()
            .as_deref()
            .and_then(|call| cide_agents::native_plan::qwen_plan_result(value, call));
        if self.phase() == Phase::Approving {
            match result {
                Some(true) => self.transition(Phase::Implementing),
                Some(false) => self.fail("Qwen rejected its native plan transition"),
                None => {}
            }
        }
    }
    /// Qwen's native /plan <work> command changes mode before it submits the work,
    /// and retains prePlanMode for the native RestorePrevious approval action.
    pub fn start_qwen(&self, pty: &Arc<PtySession>, prompt: String) {
        let handle = self.clone();
        let pty = Arc::clone(pty);
        let generation = handle.0.state.lock().generation;
        let _ = std::thread::Builder::new()
            .name("cide-qwen-plan-entry".into())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(45);
                while Instant::now() < deadline
                    && !pty.has_exited()
                    && handle.valid()
                    && handle.0.state.lock().generation == generation
                {
                    if handle.0.ready.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if !handle.valid() || handle.0.state.lock().generation != generation {
                    return;
                }
                if !handle.0.ready.load(Ordering::SeqCst) {
                    handle.fail("Qwen did not announce a native interactive session");
                    return;
                }
                crate::agents::type_submitted_line(
                    &handle.0.app,
                    handle.0.session,
                    &pty,
                    format!("/plan {prompt}\r").into_bytes(),
                );
            });
    }
    pub fn watch_terminal(&self, pty: &Arc<PtySession>) {
        let handle = self.clone();
        let child = Arc::downgrade(pty);
        let ended = self.clone();
        pty.on_exit(move |_| ended.cancel());
        if !matches!(self.0.harness, Harness::Claude | Harness::Qwen) || !self.pending() {
            return;
        }
        let generation = handle.0.state.lock().generation;
        let _ = std::thread::Builder::new()
            .name("cide-native-plan".into())
            .spawn(move || {
                let mut deadline = None;
                while handle.pending() && handle.0.state.lock().generation == generation {
                    let Some(pty) = child.upgrade().filter(|pty| !pty.has_exited()) else {
                        return;
                    };
                    if !handle.valid() {
                        deadline = None;
                        std::thread::sleep(Duration::from_millis(150));
                        continue;
                    }
                    if handle.0.plan_exit.load(Ordering::SeqCst) {
                        let deadline = *deadline
                            .get_or_insert_with(|| Instant::now() + Duration::from_secs(30));
                        if Instant::now() > deadline {
                            handle.fail(
                                "the native plan-exit dialog or execution mode is unsupported",
                            );
                            return;
                        }
                        if crate::emit::last_session_states().get(&handle.0.session)
                            != Some(&cide_ipc::SessionState::AwaitingPermission)
                        {
                            std::thread::sleep(Duration::from_millis(150));
                            continue;
                        }
                        let screen = pty.capture_screen();
                        if let Some(prompt) =
                            cide_agents::native_plan::terminal_prompt(&screen, handle.0.harness)
                            && let Some(selected) = prompt.selected
                            && let Some(target) = cide_agents::native_plan::terminal_choice(
                                &prompt,
                                handle.execution(),
                            )
                        {
                            let current = cide_agents::native_plan::terminal_prompt(
                                &pty.capture_screen(),
                                handle.0.harness,
                            );
                            if current.as_ref().map(|p| &p.digest) != Some(&prompt.digest)
                                || !handle.valid()
                            {
                                continue;
                            }
                            if handle.0.accepting.swap(true, Ordering::SeqCst) {
                                return;
                            }
                            handle.transition(Phase::Approving);
                            for key in cide_agents::native_plan::choice_keys(
                                selected,
                                target,
                                screen.info.app_cursor,
                            ) {
                                if !handle.valid()
                                    || pty.has_exited()
                                    || handle.0.state.lock().generation != generation
                                {
                                    return;
                                }
                                if cide_agents::native_plan::terminal_prompt(
                                    &pty.capture_screen(),
                                    handle.0.harness,
                                )
                                .as_ref()
                                .map(|p| &p.digest)
                                    != Some(&prompt.digest)
                                {
                                    handle.fail(
                                        "the native plan confirmation changed before acceptance",
                                    );
                                    return;
                                }
                                pty.write(key);
                                std::thread::sleep(Duration::from_millis(60));
                            }
                            let acknowledgement = Instant::now() + Duration::from_secs(30);
                            while handle.pending()
                                && !pty.has_exited()
                                && Instant::now() < acknowledgement
                                && handle.0.state.lock().generation == generation
                            {
                                std::thread::sleep(Duration::from_millis(100));
                            }
                            if handle.pending() && handle.valid() {
                                handle
                                    .fail("the harness did not acknowledge native plan acceptance");
                            }
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
            });
    }
}

pub(crate) fn observe_hook(app: &AppHandle, frame: &cide_claude::HookFrame) {
    if let Some(session) = frame.owner().and_then(|id| id.parse().ok())
        && let Some(handle) = get(app, session)
    {
        if let Some(id) = frame.payload["session_id"].as_str() {
            let changed = handle
                .0
                .conversation
                .lock()
                .as_ref()
                .is_some_and(|conversation| conversation.id != id);
            if changed {
                if frame.event == "SessionStart" {
                    handle.abandon();
                }
                return;
            }
        }
        handle.observe(&frame.payload);
        if frame.event == "PostToolUse"
            && matches!(
                frame.payload["tool_name"].as_str(),
                Some("ExitPlanMode" | "exit_plan_mode")
            )
            && handle.phase() == Phase::Approving
        {
            handle.transition(Phase::Implementing);
        }
        if frame.event == "SessionStart"
            && let (Some(id), Some(cwd)) = (
                frame.payload["session_id"].as_str(),
                frame.payload["cwd"].as_str(),
            )
        {
            handle.bind(HarnessSession {
                harness: handle.0.harness,
                id: id.into(),
                cwd: cwd.into(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_claude_launch_preserves_the_role_tools_model_and_prompt() {
        let args = [
            "--agent",
            "developer",
            "--model",
            "chosen",
            "--allowedTools=Read,Grep",
            "--append-system-prompt",
            "original role",
            "--dangerously-skip-permissions",
        ]
        .map(str::to_string);
        let plan = claude_args(&args, "bypassPermissions");
        for expected in [
            ["--agent", "developer"],
            ["--model", "chosen"],
            ["--append-system-prompt", "original role"],
            ["--permission-mode", "plan"],
        ] {
            assert!(plan.windows(2).any(|args| args == expected));
        }
        assert!(plan.iter().any(|arg| arg == "Read,Grep"));
        assert!(plan.iter().any(|arg| arg == "mcp__cide__cide_task_get"));
        assert!(!plan.iter().any(|arg| arg == "mcp__cide__cide_task_update"));
        assert!(
            !plan
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
        assert!(
            plan.iter()
                .any(|arg| arg == "--allow-dangerously-skip-permissions")
        );
        assert!(
            !claude_args(&args, "auto")
                .iter()
                .any(|arg| arg == "--allow-dangerously-skip-permissions")
        );
    }
    #[test]
    fn native_claude_refuses_an_unobservable_plan_instead_of_waiting_forever() {
        assert!(!claude_observable(&[]));
        let settings = serde_json::json!({"hooks":{"PreToolUse":[{"hooks":[{"command":"/app/cide-hook PreToolUse"}]}],"PostToolUse":[{"hooks":[{"command":"/app/cide-hook PostToolUse"}]}]}}).to_string();
        assert!(claude_observable(&["--settings".into(), settings.clone()]));
        assert!(!claude_observable(&[
            "--settings".into(),
            settings,
            "--bare".into()
        ]));
    }
    #[test]
    fn only_the_delegated_launch_gets_a_blocking_native_question_hook() {
        let original = serde_json::json!({"hooks":{"PreToolUse":[{"matcher":"","hooks":[{"type":"command","command":"/app/cide-hook PreToolUse"}]}],"PostToolUse":[{"hooks":[{"command":"/app/cide-hook PostToolUse"}]}]}});
        let args = vec!["--settings".into(), original.to_string()];
        let planned = claude_args(&args, "auto");
        let index = planned.iter().position(|a| a == "--settings").unwrap();
        let settings: serde_json::Value = serde_json::from_str(&planned[index + 1]).unwrap();
        assert_eq!(
            settings["hooks"]["PreToolUse"][0],
            original["hooks"]["PreToolUse"][0]
        );
        assert_eq!(
            settings["hooks"]["PreToolUse"][1]["matcher"],
            "^AskUserQuestion$"
        );
        assert_eq!(
            settings["hooks"]["PreToolUse"][1]["hooks"][0]["command"],
            "/app/cide-hook native-question"
        );
        assert_eq!(
            settings["hooks"]["PreToolUse"][1]["hooks"][0]["onFailure"],
            "block"
        );
        assert_eq!(
            settings["hooks"]["PreToolUse"][1]["hooks"][0]["timeout"],
            86400
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&args[1]).unwrap(),
            original
        );
    }
}
