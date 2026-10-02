//! One private OpenCode server per interactive console. The TUI and cide observe the same session.
use cide_agents::harness::{
    opencode::{Generation, OPENCODE_CLI},
    opencode_console::Api,
};
use cide_ipc::{Harness, HarnessSession, SessionId, SessionState, Settings};
use cide_pty::{PtySession, SpawnSpec};
use parking_lot::Mutex;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

pub(crate) struct Console {
    server: Arc<PtySession>,
    api: Api,
    conversation: Mutex<HarnessSession>,
    stopped: AtomicBool,
    instructions: Option<String>,
    instruction_file: Option<PathBuf>,
    events: bool,
}

pub(crate) struct Start {
    pub settings: Settings,
    pub spec: SpawnSpec,
    pub routing: SessionId,
    pub resume: Option<HarnessSession>,
    pub fork: bool,
    pub prompt: Option<String>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub unattended: bool,
}

impl Console {
    pub fn start(start: Start) -> Result<(SpawnSpec, Arc<Self>), String> {
        let Start {
            settings,
            mut spec,
            routing,
            resume,
            fork,
            prompt,
            instructions,
            model,
            unattended,
        } = start;
        let cli = &settings.opencode.cli;
        cide_core::opencode_cli::resolve(&cli.binary)?;
        let flags = OPENCODE_CLI.probe_with_cli(cli);
        let socket = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let port = socket.local_addr().map_err(|e| e.to_string())?.port();
        drop(socket);
        let password = SessionId::new().to_string();
        let api = Api::new(
            format!("http://127.0.0.1:{port}"),
            password.clone(),
            flags.generation,
        )?;
        let mut launch = cide_core::opencode_cli::plan(cli);
        let user_model = take_option(&mut launch.args, &["--model", "-m"]);
        let agent = take_option(&mut launch.args, &["--agent"]);
        let title = take_option(&mut launch.args, &["--title"]);
        let model = model.or(user_model);
        let unattended = unattended || launch.args.iter().any(|arg| arg == "--auto");
        launch.args.retain(|arg| arg != "--auto");
        let hook = crate::agents::cide_hook_binary();
        let mut document: serde_json::Value = OPENCODE_CLI
            .tab_config(
                &settings.llm,
                cli.inject.mcp_config.then_some(hook.as_deref()).flatten(),
                flags.generation,
            )
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({}));
        let instructions =
            instructions.filter(|_| cli.inject.instructions && cli.inject.mcp_config);
        let instruction_file = if flags.generation == Generation::V1 {
            instructions
                .as_ref()
                .map(|text| {
                    let path = cide_core::persist::config_dir()
                        .join("console-instructions")
                        .join(format!("{routing}.md"));
                    cide_core::persist::write_atomic_with_mode(&path, text.as_bytes(), 0o600)
                        .map_err(|e| e.to_string())?;
                    document["instructions"] = json!([path]);
                    Ok::<_, String>(path)
                })
                .transpose()?
        } else {
            None
        };
        let mut server_spec = SpawnSpec::new(cli.binary.trim(), spec.cwd.clone())
            .apply(cide_core::child_env::terminal_child_env(
                &settings.claude,
                env!("CARGO_PKG_VERSION"),
                launch.env.clone(),
            ))
            .apply(
                cide_core::proxy::ProxyEnv::for_target(
                    &settings.proxy,
                    settings.proxy.scope.claude,
                )
                .changes()
                .to_vec(),
            );
        server_spec.args = launch.args.clone();
        server_spec.args.extend([
            "serve".into(),
            "--hostname".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
        ]);
        // The MCP bridge is a child of the server, so it needs the console's routing identity.
        for (name, value) in &spec.env {
            if name.starts_with("CIDE_") {
                server_spec.env.push((name.clone(), value.clone()));
            }
        }
        server_spec = server_spec
            .env("CIDE_SESSION", routing.to_string())
            .env("OPENCODE_SERVER_PASSWORD", password.clone())
            .env("OPENCODE_SERVER_USERNAME", "opencode")
            .env("OPENCODE_CONFIG_CONTENT", document.to_string());
        let server = match PtySession::spawn(server_spec) {
            Ok(server) => server,
            Err(error) => {
                if let Some(path) = &instruction_file {
                    let _ = std::fs::remove_file(path);
                }
                return Err(format!("OpenCode server failed to start: {error}"));
            }
        };
        let mut console = Self {
            server,
            api,
            conversation: Mutex::new(HarnessSession {
                harness: Harness::Opencode,
                id: String::new(),
                cwd: spec.cwd.clone(),
            }),
            stopped: AtomicBool::new(false),
            instructions,
            instruction_file,
            events: cli.inject.events,
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        while !console.api.healthy() {
            if console.server.has_exited() || Instant::now() > deadline {
                return Err("OpenCode’s private server did not become ready. Check Settings → Harness → Launch.".into());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        let id = match resume.filter(|_| cli.inject.resume) {
            Some(resume) => {
                console.api.exists(&resume.id)?;
                if fork {
                    if cli.inject.fork {
                        match console.api.fork(&resume.id) {
                            Ok(id) => id,
                            Err(error) if error.contains("empty_session") => console.api.create(
                                &spec.cwd,
                                title.as_deref(),
                                model.as_deref(),
                                agent.as_deref(),
                                unattended,
                            )?,
                            Err(error) => return Err(error),
                        }
                    } else {
                        console.api.create(
                            &spec.cwd,
                            title.as_deref(),
                            model.as_deref(),
                            agent.as_deref(),
                            unattended,
                        )?
                    }
                } else {
                    resume.id
                }
            }
            None => console.api.create(
                &spec.cwd,
                title.as_deref(),
                model.as_deref(),
                agent.as_deref(),
                unattended,
            )?,
        };
        console.conversation.get_mut().id = id.clone();
        if let Some(instructions) = &console.instructions {
            console.api.instructions(&id, instructions)?;
        }
        let attachment =
            OPENCODE_CLI.attach_spec_with(&console.conversation(), &console.api_url(port), flags);
        spec.program = cli.binary.trim().into();
        spec.args = launch.args;
        spec.args.extend(attachment.args);
        if unattended && flags.tui_skip_permissions {
            spec.args.push("--auto".into());
        }
        if flags.generation == Generation::V1 {
            if let Some(model) = &model {
                spec.args.extend(["--model".into(), model.clone()]);
            }
            if let Some(agent) = &agent {
                spec.args.extend(["--agent".into(), agent.clone()]);
            }
        }
        spec = spec
            .apply(launch.env)
            .env("OPENCODE_SERVER_PASSWORD", password)
            .env("OPENCODE_SERVER_USERNAME", "opencode");
        // Submit after the TUI attaches. Passing --prompt also makes the opening turn visible in the TUI.
        if let Some(prompt) = prompt.filter(|p| !p.trim().is_empty()) {
            spec.args.extend(["--prompt".into(), prompt]);
        }
        Ok((spec, Arc::new(console)))
    }
    fn api_url(&self, port: u16) -> String {
        format!("http://127.0.0.1:{port}")
    }
    pub fn conversation(&self) -> HarnessSession {
        self.conversation.lock().clone()
    }
    fn select_conversation(&self, app: &AppHandle, routing: SessionId, id: &str) -> bool {
        if cide_agents::harness::opencode_console::validate_id(id).is_err()
            || self.conversation().id == id
        {
            return false;
        }
        let Ok(info) = self
            .api
            .request(reqwest::Method::GET, &format!("/session/{id}"), None)
        else {
            return false;
        };
        if info.get("parentID").is_some() || !same_directory(&info, &self.conversation().cwd) {
            return false;
        }
        let conversation = {
            let mut conversation = self.conversation.lock();
            conversation.id = id.into();
            conversation.clone()
        };
        if let Some(text) = &self.instructions {
            let _ = self.api.instructions(id, text);
        }
        if let Some(registry) = app.try_state::<crate::state::SessionRegistry>() {
            registry.note_native_conversation(routing, conversation.clone());
        }
        if let Some(state) = app.try_state::<crate::workspace_state::WorkspaceState>() {
            let _ = state.update(|ws| {
                cide_core::workspace::note_native_conversation(ws, routing, &conversation);
                Ok(())
            });
        }
        true
    }
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let tree = self
            .server
            .child_pid()
            .map(cide_core::process_tree::capture)
            .unwrap_or_default();
        self.server.kill();
        tree.finish(
            Duration::from_millis(500),
            "an OpenCode console server".into(),
        );
    }
    pub fn observe(self: &Arc<Self>, app: AppHandle, routing: SessionId, tui: &Arc<PtySession>) {
        let stopped = Arc::clone(self);
        tui.on_exit(move |_| stopped.stop());
        let weak_tui = Arc::downgrade(tui);
        self.server.on_exit(move |_| {
            if let Some(tui) = weak_tui.upgrade() {
                tui.kill();
            }
        });
        let this = Arc::clone(self);
        let event_app = app.clone();
        if self.events {
            let _ = std::thread::Builder::new()
                .name("cide-opencode-events".into())
                .spawn(move || {
                    while !this.stopped.load(Ordering::SeqCst) {
                        let _ = this.api.events(|event| {
                            if this.stopped.load(Ordering::SeqCst) {
                                return false;
                            }
                            let kind = event["type"].as_str().unwrap_or("");
                            let payload = event
                                .get("data")
                                .or_else(|| event.get("properties"))
                                .unwrap_or(&event);
                            if kind == "session.created" {
                                let info = payload.get("info").unwrap_or(payload);
                                if let Some(id) = info
                                    .get("sessionID")
                                    .or_else(|| info.get("id"))
                                    .and_then(|v| v.as_str())
                                {
                                    this.select_conversation(&event_app, routing, id);
                                }
                            }
                            if kind == "file.edited"
                                && let Some(path) = payload.get("file").and_then(|v| v.as_str())
                            {
                                crate::emit::session_tool(
                                    &event_app,
                                    &routing.to_string(),
                                    vec![path.into()],
                                );
                            }
                            true
                        });
                        std::thread::sleep(Duration::from_millis(250));
                    }
                });
        }
        let this = Arc::clone(self);
        let _ = std::thread::Builder::new().name("cide-opencode-state".into()).spawn(move || {
            let mut worked = false;
            let mut failures = 0;
            let mut last_status = serde_json::Value::Null;
            let mut tracked = String::new();
            while !this.stopped.load(Ordering::SeqCst) {
                let mut conversation = this.conversation();
                if this.events {
                    let route = if this.api.generation == Generation::V2 { "/session/active" } else { "/session/status" };
                    if let Ok(active) = this.api.request(reqwest::Method::GET, route, None)
                        && let Some(sessions) = active.as_object() {
                        for (id, status) in sessions {
                            if id != &conversation.id
                                && status["type"].as_str().is_some_and(|s| matches!(s, "running" | "busy" | "retry"))
                                && this.select_conversation(&app, routing, id) {
                                conversation = this.conversation();
                                break;
                            }
                        }
                    }
                    if tracked != conversation.id { tracked = conversation.id.clone(); worked = false; }
                    match this.api.state(&conversation.id, worked) {
                        Ok(state) => {
                            worked |= state == SessionState::Busy || state == SessionState::AwaitingPermission;
                            failures = 0;
                            if let Ok(info) = this.api.request(reqwest::Method::GET, &format!("/session/{}", conversation.id), None) {
                                let payload = status_payload(&info);
                                if payload != last_status { crate::emit::session_status(&app, &routing.to_string(), payload.clone()); last_status = payload; }
                            }
                            if let Some(hooks) = app.try_state::<crate::hooks::HookServer>() { hooks.external_state(&app, routing, state); }
                        }
                        Err(error) => {
                            if failures == 0 { tracing::warn!(%routing, %error, "OpenCode console state connection failed"); }
                            failures += 1;
                            if failures >= 3 { this.stop(); break; }
                        }
                    }
                } else if let Some(hooks) = app.try_state::<crate::hooks::HookServer>() {
                    hooks.external_state(&app, routing, SessionState::Idle);
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.stop();
        if let Some(path) = &self.instruction_file {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Prompt-only one-shots run without tools, with the same project launch settings.
pub(crate) fn headless(
    settings: Settings,
    cwd: PathBuf,
    request: cide_ipc::HeadlessRequest,
) -> Result<cide_ipc::HeadlessResult, cide_ipc::HeadlessError> {
    use cide_core::child_env::{FilterError, run_filter_with};
    use cide_ipc::{HeadlessError, HeadlessResult};
    if request.max_budget_usd.is_some() {
        return Err(HeadlessError::Malformed {
            detail: "OpenCode does not support a hard spending limit".into(),
            head: String::new(),
        });
    }
    let cli = settings.opencode.cli;
    cide_core::opencode_cli::resolve(&cli.binary)
        .map_err(|detail| HeadlessError::NotInstalled { detail })?;
    let flags = OPENCODE_CLI.probe_with_cli(&cli);
    let launch = cide_core::opencode_cli::plan(&cli);
    let mut command = std::process::Command::new(cli.binary.trim());
    command.current_dir(cwd).args(launch.args).arg("run");
    if flags.generation == Generation::V2 {
        command.arg("--standalone");
    }
    command.args(["--format", "json", "--agent", "cide-one-shot"]);
    if let Some(model) = request.model {
        command.args(["--model", &model]);
    }
    let mut document: serde_json::Value = OPENCODE_CLI
        .tab_config(&settings.llm, None, flags.generation)
        .and_then(|config| serde_json::from_str(&config).ok())
        .unwrap_or_else(|| json!({}));
    if flags.generation == Generation::V2 {
        document["agents"] = json!({ "cide-one-shot": { "system": "Answer the supplied prompt. Do not use tools.", "permissions": [{ "action": "*", "resource": "*", "effect": "deny" }] } });
    } else {
        document["agent"] = json!({ "cide-one-shot": { "mode": "primary", "prompt": "Answer the supplied prompt. Do not use tools.", "permission": { "*": "deny" } } });
    }
    let mut env = launch.env;
    env.extend(
        cide_core::proxy::ProxyEnv::for_target(&settings.proxy, settings.proxy.scope.claude)
            .changes()
            .to_vec(),
    );
    env.push(("OPENCODE_CONFIG_CONTENT".into(), Some(document.to_string())));
    let mut prompt = request.prompt;
    let structured_requested = request.json_schema.is_some();
    if let Some(schema) = request.json_schema {
        prompt.push_str(&format!(
            "\nReturn only JSON matching this schema: {schema}"
        ));
    }
    let started = Instant::now();
    for (key, value) in env {
        if let Some(value) = value {
            command.env(key, value);
        } else {
            command.env_remove(key);
        }
    }
    let output = run_filter_with(
        command,
        Some(prompt.as_bytes()),
        Duration::from_secs(180),
        &[],
    )
    .map_err(|error| match error {
        FilterError::Timeout => HeadlessError::TimedOut { seconds: 180 },
        other => HeadlessError::NotInstalled {
            detail: format!("OpenCode: {other:?}"),
        },
    })?;
    if !output.ok {
        return Err(HeadlessError::Exited {
            code: None,
            stderr: output.stderr.chars().take(2000).collect(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut text = String::new();
    let mut session = None;
    let mut cost = None;
    let mut turns = 0;
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(id) = event["sessionID"].as_str() {
            session = Some(id.into());
        }
        match event["type"].as_str() {
            Some("text") => {
                if let Some(part) = event["part"]["text"].as_str() {
                    text.push_str(part);
                }
            }
            Some("step_finish") => {
                turns += 1;
                if let Some(value) = event["part"]["cost"].as_f64() {
                    cost = Some(cost.unwrap_or(0.0) + value);
                }
            }
            Some("error") => {
                return Err(HeadlessError::Exited {
                    code: None,
                    stderr: event["error"].to_string(),
                });
            }
            _ => {}
        }
    }
    if text.is_empty() {
        return Err(HeadlessError::Malformed {
            detail: "OpenCode returned no text".into(),
            head: stdout.chars().take(1000).collect(),
        });
    }
    let structured = structured_requested
        .then(|| serde_json::from_str(text.trim()).ok())
        .flatten();
    Ok(HeadlessResult {
        text,
        structured,
        is_error: false,
        subtype: "success".into(),
        session,
        cost_usd: cost,
        duration_ms: Some(started.elapsed().as_millis() as u64),
        num_turns: Some(turns),
    })
}

fn same_directory(info: &serde_json::Value, cwd: &std::path::Path) -> bool {
    info["location"]["directory"]
        .as_str()
        .or_else(|| info["directory"].as_str())
        .is_some_and(|path| {
            cide_core::harness_settings::root_key(std::path::Path::new(path))
                == cide_core::harness_settings::root_key(cwd)
        })
}
fn status_payload(info: &serde_json::Value) -> serde_json::Value {
    let tokens = &info["tokens"];
    let mut payload = json!({ "harness": "opencode", "cost": { "total_cost_usd": info["cost"] }, "model": { "id": info["model"]["id"] } });
    if let (Some(input), Some(output)) = (tokens["input"].as_u64(), tokens["output"].as_u64()) {
        payload["context_window"] =
            json!({ "total_input_tokens": input, "total_output_tokens": output });
    }
    payload
}
fn take_option(args: &mut Vec<String>, names: &[&str]) -> Option<String> {
    let mut result = None;
    let mut kept = Vec::new();
    let mut tokens = std::mem::take(args).into_iter();
    while let Some(token) = tokens.next() {
        let (key, value) = token
            .split_once('=')
            .map_or((token.as_str(), None), |(k, v)| (k, Some(v)));
        if names.contains(&key) {
            result = value.map(str::to_owned).or_else(|| tokens.next());
        } else {
            kept.push(token);
        }
    }
    *args = kept;
    result
}
