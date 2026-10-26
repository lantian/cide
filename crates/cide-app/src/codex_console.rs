//! Paired Codex consoles use the real TUI over an owned local app-server.
//!
//! A small forwarding connection lets Cide submit turns without touching the composer.
//! Peer turns share the native terminal's own connection, and all server requests reach
//! it unchanged. Bootstrap and background clients cannot claim its conversation. Only
//! our own turn acknowledgements are consumed here. Native lifecycle events enter the
//! existing ordered hook applier because
//! remote Codex does not run the TUI's invocation-local hook-trust bypass.
use cide_agents::harness::codex_console::{self as protocol, Hook, Protocol, Receiver};
use cide_ipc::SessionId;
use cide_pty::{PtySession, SpawnSpec};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc as channel, watch};
use tokio_tungstenite::{
    accept_hdr_async_with_config, connect_async_with_config,
    tungstenite::{
        self, Message, client::IntoClientRequest, http::StatusCode, protocol::WebSocketConfig,
    },
};

const TOKEN_ENV: &str = "CIDE_CODEX_CONSOLE_TOKEN";
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
// A resumed Codex thread can arrive as one unfragmented JSON frame. The library's
// 16 MiB frame default disconnected real consoles during resume (16,796,165 bytes
// in the reported case). Keep a finite bound equal to its default message limit.
const MAX_CODEX_MESSAGE: usize = 64 << 20;

fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_frame_size(Some(MAX_CODEX_MESSAGE))
        .max_message_size(Some(MAX_CODEX_MESSAGE))
}
type Valid = Box<dyn Fn() -> Result<(), String> + Send>;
type Reply = Result<bool, (bool, String)>;
type Observe = Arc<dyn Fn(Vec<Hook>) + Send + Sync>;

struct Submission {
    receiver: Receiver,
    body: String,
    valid: Valid,
    expired: Arc<AtomicBool>,
    reply: mpsc::Sender<Reply>,
}

struct Pending {
    deadline: Instant,
    reply: mpsc::Sender<Reply>,
}

#[derive(Default, Debug)]
struct Selection {
    connection: Option<u64>,
    protocol: Arc<Mutex<Protocol>>,
    disconnected: bool,
}
impl Selection {
    fn receiver(&self) -> Option<Receiver> {
        self.connection
            .and_then(|_| self.protocol.lock().receiver())
    }
}

#[derive(Clone)]
struct ForwardContext {
    server_url: String,
    token: String,
    selected: Arc<Mutex<Selection>>,
    observe: Observe,
    policy: Value,
    decision: Option<crate::decision::Handle>,
    planning: Option<crate::native_plan::Handle>,
}

struct Credentials(PathBuf);
impl Credentials {
    fn new(token: &str) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("cide-codex-{}", SessionId::new()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).map_err(|e| e.to_string())?;
        let credentials = Self(dir);
        cide_core::persist::write_atomic_with_mode(
            &credentials.0.join("token"),
            token.as_bytes(),
            0o600,
        )
        .map_err(|e| e.to_string())?;
        Ok(credentials)
    }
}
impl Drop for Credentials {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) struct Console {
    server: Arc<PtySession>,
    protocol: Arc<Mutex<Selection>>,
    submissions: channel::UnboundedSender<Submission>,
    shutdown: watch::Sender<bool>,
    stopped: AtomicBool,
    _credentials: Credentials,
}

impl Console {
    #[cfg(test)]
    pub fn start(spec: SpawnSpec, observe: Observe) -> Result<(SpawnSpec, Arc<Self>), String> {
        Self::start_with_decision(spec, observe, None)
    }

    #[cfg(test)]
    pub fn start_with_decision(
        spec: SpawnSpec,
        observe: Observe,
        decision: Option<crate::decision::Handle>,
    ) -> Result<(SpawnSpec, Arc<Self>), String> {
        Self::start_planning(spec, observe, decision, None)
    }

    pub(crate) fn start_planning(
        mut spec: SpawnSpec,
        observe: Observe,
        decision: Option<crate::decision::Handle>,
        planning: Option<crate::native_plan::Handle>,
    ) -> Result<(SpawnSpec, Arc<Self>), String> {
        if let Some(h) = &decision {
            h.available(true, "Native Codex app-server adapter");
        }
        supports_remote(&spec)?;
        spec.args = protocol::remote_args(&spec.args);
        let token = format!("{}{}", SessionId::new(), SessionId::new());
        let credentials = Credentials::new(&token)?;
        let proxy = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let proxy_url = format!(
            "ws://127.0.0.1:{}",
            proxy.local_addr().map_err(|e| e.to_string())?.port()
        );
        proxy.set_nonblocking(true).map_err(|e| e.to_string())?;
        let reservation = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let server_addr = reservation.local_addr().map_err(|e| e.to_string())?;
        let server_url = format!("ws://{server_addr}");
        drop(reservation);

        let mut server_spec = SpawnSpec::new(&spec.program, &spec.cwd);
        server_spec.env = spec.env.clone();
        server_spec.env_remove = spec.env_remove.clone();
        server_spec.args = vec![
            "app-server".into(),
            "--listen".into(),
            server_url.clone(),
            "--ws-auth".into(),
            "capability-token".into(),
            "--ws-token-file".into(),
            credentials.0.join("token").to_string_lossy().into_owned(),
        ];
        server_spec
            .args
            .extend(protocol::server_overrides(&spec.args));
        let server = PtySession::spawn(server_spec)
            .map_err(|e| format!("Codex app-server failed to start: {e}"))?;
        let (submissions, receive) = channel::unbounded_channel();
        let (shutdown, shutdown_rx) = watch::channel(false);
        let protocol = Arc::new(Mutex::new(Selection::default()));
        let console = Arc::new(Self {
            server,
            protocol: protocol.clone(),
            submissions,
            shutdown,
            stopped: AtomicBool::new(false),
            _credentials: credentials,
        });
        let mut policy = Value::Null;
        let mut policy_error = None;

        // Authenticate a disposable connection before starting the terminal. A CLI without
        // this protocol must fail setup explicitly, never quietly fall back to pasting.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let connected = TcpStream::connect_timeout(&server_addr, Duration::from_millis(200))
                .ok()
                .is_some_and(|stream| {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    let Ok(request) = authenticated(&server_url, &token) else {
                        return false;
                    };
                    let Ok((mut client, _)) = tungstenite::client(request, stream) else {
                        return false;
                    };
                    match read_policy(&mut client, &spec.cwd, planning.is_some()) {
                        Ok(config) => {
                            if let Some((_, scratch)) = spec.env.iter().rev().find(|(name, _)| {
                                name == cide_core::peer_chat::reviewer::SCRATCH_ENV
                            }) && let Err(error) =
                                cide_core::peer_chat::reviewer::validate_codex_config(
                                    &config,
                                    std::path::Path::new(scratch),
                                )
                            {
                                policy_error = Some(error);
                                return false;
                            }
                            policy = protocol::thread_policy(&config, &spec.args);
                            if spec.env.iter().any(|(name, _)| {
                                name == cide_core::peer_chat::reviewer::SCRATCH_ENV
                            }) {
                                // A resumed thread can carry an older disabled search
                                // setting; apply the reviewer's current launch choice.
                                policy["config"]["web_search"] = config["web_search"].clone();
                            }
                            true
                        }
                        Err(error) => {
                            if error.starts_with("Native Plan mode unavailable") {
                                policy_error = Some(error);
                            }
                            false
                        }
                    }
                });
            if let Some(error) = policy_error.take() {
                return Err(error);
            }
            if connected {
                break;
            }
            if console.server.has_exited() || Instant::now() >= deadline {
                return Err("Codex’s private app-server did not become ready. Update Codex to a version supporting authenticated app-server and --remote, and check its launch settings.".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // Remote resume refuses permission overrides in the terminal's argv. Apply the
        // already-resolved launch policy to native thread requests instead. The server
        // retains all its original overrides, including named profile definitions.
        spec.args = cide_core::codex_cli::strip_permission_args(&spec.args);
        let server = console.server.clone();
        std::thread::Builder::new()
            .name("cide-codex-console".into())
            .spawn(move || {
                let outcome = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())
                    .and_then(|runtime| {
                        runtime.block_on(forward(
                            proxy,
                            ForwardContext {
                                server_url,
                                token,
                                selected: protocol,
                                observe,
                                policy,
                                decision,
                                planning,
                            },
                            receive,
                            shutdown_rx,
                        ))
                    });
                if let Err(error) = outcome {
                    tracing::warn!(%error, "Codex console connection ended");
                }
                // The terminal's socket closes with the forwarding loop. Also stop the server
                // if that loop exits before the terminal has connected or after a protocol error.
                let tree = server
                    .child_pid()
                    .map(cide_core::process_tree::capture)
                    .unwrap_or_default();
                server.kill();
                tree.finish(Duration::from_millis(500), "a Codex console server".into());
            })
            .map_err(|e| e.to_string())?;

        // Keep the native argv, including resume/fork, its permission profile and model.
        // The endpoint is Cide-owned; user --remote options remain refused by codex_cli.
        let at = spec
            .args
            .iter()
            .position(|a| a == "--")
            .unwrap_or(spec.args.len());
        spec.args.splice(
            at..at,
            [
                "--remote".into(),
                proxy_url,
                "--remote-auth-token-env".into(),
                TOKEN_ENV.into(),
            ],
        );
        spec.env.push((TOKEN_ENV.into(), token_for(&console)?));
        Ok((spec, console))
    }

    pub fn receiver(&self) -> Result<Option<Receiver>, String> {
        if self.stopped.load(Ordering::SeqCst) || self.server.has_exited() {
            return Err("The Codex console server stopped".into());
        }
        let selected = self.protocol.lock();
        if selected.disconnected {
            return Err(
                "The native Codex connection closed. Restore the receiving pane before resuming."
                    .into(),
            );
        }
        Ok(selected.receiver())
    }

    pub fn turn(&self, conversation: &str) -> Option<String> {
        let selected = self.protocol.lock();
        let protocol = selected.protocol.lock();
        (protocol.current_thread() == Some(conversation))
            .then(|| protocol.current_turn().map(str::to_owned))
            .flatten()
    }

    pub fn peer_prompt(&self, receiver: Receiver, body: String, valid: Valid) -> Reply {
        let (reply, receive) = mpsc::channel();
        let expired = Arc::new(AtomicBool::new(false));
        self.submissions
            .send(Submission {
                receiver,
                body,
                valid,
                expired: expired.clone(),
                reply,
            })
            .map_err(|_| {
                (
                    false,
                    "The Codex console connection closed before submission".into(),
                )
            })?;
        match receive.recv_timeout(ACK_TIMEOUT + Duration::from_secs(1)) {
            Ok(result) => result,
            Err(_) => {
                expired.store(true, Ordering::SeqCst);
                Err((true, "Codex submission was not confirmed. Inspect the receiving conversation before resending.".into()))
            }
        }
    }

    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shutdown.send_replace(true);
        let tree = self
            .server
            .child_pid()
            .map(cide_core::process_tree::capture)
            .unwrap_or_default();
        self.server.kill();
        tree.finish(Duration::from_millis(500), "a Codex console server".into());
    }

    pub fn observe(self: &Arc<Self>, tui: &Arc<PtySession>) {
        let console = self.clone();
        tui.on_exit(move |_| console.stop());
        let weak = Arc::downgrade(tui);
        self.server.on_exit(move |_| {
            if let Some(tui) = weak.upgrade() {
                tui.kill();
            }
        });
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        self.stop();
    }
}

fn token_for(console: &Console) -> Result<String, String> {
    std::fs::read_to_string(console._credentials.0.join("token")).map_err(|e| e.to_string())
}

fn authenticated(url: &str, token: &str) -> Result<tungstenite::http::Request<()>, String> {
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {token}")
            .parse()
            .map_err(|e| format!("Invalid local credential: {e}"))?,
    );
    Ok(request)
}

fn append_research(frame: &mut Value, text: &str) -> bool {
    let Some(input) = frame["params"]["input"].as_array_mut() else {
        return false;
    };
    input.push(json!({
        "type": "text",
        "text": format!("Cide decision assistance (advisory context): {text}\nFollow the user's request and normal permission rules; skip this suggestion if irrelevant."),
        "text_elements": []
    }));
    true
}

fn supports_remote(spec: &SpawnSpec) -> Result<(), String> {
    let mut command = std::process::Command::new(&spec.program);
    command
        .arg("--help")
        .current_dir(&spec.cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for name in &spec.env_remove {
        command.env_remove(name);
    }
    command.envs(spec.env.iter().cloned());
    let mut child = cide_core::child_env::on_spawn_thread(move || {
        cide_core::child_env::arm(&mut command);
        command.spawn()
    })
    .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(
                "Codex capability check timed out. Check its configured binary or wrapper.".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let help = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !help.contains("--remote-auth-token-env") {
        return Err("Two-agent chat requires Codex with --remote and authenticated app-server support. Update Codex or configure a wrapper that forwards those commands.".into());
    }
    Ok(())
}

async fn forward(
    listener: TcpListener,
    context: ForwardContext,
    mut submissions: channel::UnboundedReceiver<Submission>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::from_std(listener).map_err(|e| e.to_string())?;
    let mut connections = HashMap::new();
    let mut workers = tokio::task::JoinSet::new();
    let mut next = 0u64;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                while workers.join_next().await.is_some() {}
                return Ok(());
            }
            Some(finished) = workers.join_next() => {
                let (id, outcome) = finished.map_err(|e| e.to_string())?;
                connections.remove(&id);
                if let Err(error) = outcome { tracing::debug!(%error, "Codex client connection ended"); }
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|e| e.to_string())?;
                next += 1;
                let id = next;
                let (send, receive) = channel::unbounded_channel();
                connections.insert(id, send);
                let context = context.clone();
                let selected = context.selected.clone();
                let shutdown = shutdown.clone();
                workers.spawn(async move {
                    let result = forward_connection(stream, id, context, receive, shutdown).await;
                    let mut selected = selected.lock();
                    if selected.connection == Some(id) {
                        selected.connection = None;
                        selected.disconnected = true;
                    }
                    (id, result)
                });
            }
            Some(submission) = submissions.recv() => {
                // Native resume first opens a short-lived metadata connection, then its
                // interactive client. Route only to the connection selecting the visible
                // persistent thread, never to a bootstrap or ephemeral helper connection.
                if context.selected.lock().connection != Some(submission.receiver.connection) {
                    let _ = submission.reply.send(Ok(false));
                    continue;
                }
                if let Some(connection) = connections.get(&submission.receiver.connection) {
                    if let Err(error) = connection.send(submission) {
                        let _ = error.0.reply.send(Err((false, "Codex disconnected before submission".into())));
                    }
                } else {
                    let _ = submission.reply.send(Err((false, "Codex disconnected before submission".into())));
                }
            }
        }
    }
}

async fn forward_connection(
    stream: tokio::net::TcpStream,
    connection: u64,
    context: ForwardContext,
    mut submissions: channel::UnboundedReceiver<Submission>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let ForwardContext {
        server_url,
        token,
        selected,
        observe,
        policy,
        decision,
        planning,
    } = context;
    let credential = format!("Bearer {token}");
    let handshake = accept_hdr_async_with_config(
        stream,
        move |request: &tungstenite::handshake::server::Request, response| {
            if request
                .headers()
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                != Some(credential.as_str())
                || request.headers().contains_key("Origin")
            {
                let mut error = tungstenite::handshake::server::ErrorResponse::new(None);
                *error.status_mut() = StatusCode::UNAUTHORIZED;
                return Err(error);
            }
            Ok(response)
        },
        Some(socket_config()),
    );
    if *shutdown.borrow() {
        return Ok(());
    }
    let mut terminal = tokio::select! {
        _ = shutdown.changed() => return Ok(()),
        accepted = tokio::time::timeout(Duration::from_secs(2), handshake) =>
            accepted.map_err(|_| "Codex local authentication timed out".to_string())?.map_err(|e|e.to_string())?,
    };
    let request = authenticated(&server_url, &token)?;
    let (mut server, _) = tokio::select! {
        _ = shutdown.changed() => return Ok(()),
        connected = connect_async_with_config(request, Some(socket_config()), false) => connected.map_err(|e| e.to_string())?,
    };
    let protocol = Arc::new(Mutex::new(Protocol::new(connection)));
    let mut pending: HashMap<String, Pending> = HashMap::new();
    let (decisions, mut actions) = channel::channel::<crate::decision::Action>(8);
    let mut decision_pending: HashMap<String, (u64, Option<Value>, Option<String>)> =
        HashMap::new();
    let mut research_pending: HashMap<String, u64> = HashMap::new();
    let mut active_thread = String::new();
    let mut active_turn = String::new();
    let mut native_plan = cide_agents::native_plan::Codex::default();
    let mut plan_ack: Option<String> = None;
    let mut plan_starts = std::collections::HashSet::new();
    let mut questions: HashMap<String, (Value, crate::native_questions::Question)> = HashMap::new();
    let question_scope = SessionId::new().to_string();
    let mut answered_questions = std::collections::HashSet::new();
    let mut question_interrupts = std::collections::HashSet::new();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let outcome: Result<(), String> = async {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => return Ok(()),
                frame = terminal.next() => {
                    let Some(frame) = frame else { return Ok(()) };
                    let mut frame = frame.map_err(|e| e.to_string())?;
                    if let Message::Text(text) = &frame {
                        let mut parsed: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
                        if planning.is_some() && parsed["method"] == "initialize" {
                            if !parsed["params"]["capabilities"].is_object() { parsed["params"]["capabilities"] = json!({}); }
                            parsed["params"]["capabilities"]["experimentalApi"] = json!(true);
                            frame = Message::Text(parsed.to_string().into());
                        }
                        let selects = protocol::apply_policy(&mut parsed, &policy);
                        if parsed.get("method").is_none() && answered_questions.contains(&parsed["id"].to_string()) { continue; }
                        if parsed.get("method").is_none()
                            && let Some((_, question)) = questions.remove(&parsed["id"].to_string()) {
                            answered_questions.insert(parsed["id"].to_string());
                            question.retire("Answered in the native terminal.");
                        }
                        if selects || parsed["method"] == "turn/interrupt" { questions.clear(); }
                        if selects && parsed["params"]["ephemeral"] != true
                            && let Some(plan) = &planning
                            && protocol.lock().current_thread().is_some_and(|thread| Some(thread) != parsed["params"]["threadId"].as_str()) {
                            plan.abandon();
                        }
                        let input_event = {
                            let current = selected.lock();
                            decision_input(&protocol.lock(), current.connection == Some(connection), selects, &parsed)
                        };
                        if let Some(h) = &decision && let Some(event) = input_event {
                            let text = event.text.clone();
                            h.event(event);
                            if parsed["method"] == "turn/start" {
                                h.requested_model(parsed["params"]["model"].as_str());
                                let research=h.research(&text).await;
                                let model=h.model(&text).await;
                                if let Some(action)=research {
                                    if h.can_apply(action.id, action.generation) && append_research(&mut parsed, &action.text) {
                                        research_pending.insert(parsed["id"].to_string(),action.id);
                                    } else {
                                        h.status(action.id,"stale","Session or settings changed before delivery");
                                    }
                                }
                                // A rejected model retries the user's original model selection
                                // with the same research context, which is acknowledged only then.
                                let original=parsed.clone();
                                if let Some(action)=model && h.can_apply(action.id, action.generation) && let Some(profile)=action.profile {
                                    parsed["params"]["model"]=json!(profile.model);
                                    if let Some(effort)=profile.effort {parsed["params"]["effort"]=json!(effort);}
                                    decision_pending.insert(parsed["id"].to_string(), (action.id,Some(original),Some(profile.model.clone())));
                                }
                            }
                            frame = Message::Text(parsed.to_string().into());
                        }
                        if selects {
                            frame = Message::Text(parsed.to_string().into());
                        }
                        if let Some(plan) = &planning && plan.pending() && plan.valid() {
                            let owned = protocol.lock().owns_thread(&parsed);
                            if owned && parsed["method"] == "turn/start" {
                                if cide_agents::native_plan::codex_implementation_request(&parsed) {
                                    plan.transition(cide_agents::native_plan::Phase::Implementing);
                                } else {
                                    let model = protocol.lock().current_model().map(str::to_string)
                                        .ok_or_else(|| "Codex did not identify the model for native planning".to_string())?;
                                    if native_plan.prepare(&mut parsed, &model) {
                                        plan_starts.insert(parsed["id"].to_string());
                                        frame = Message::Text(parsed.to_string().into());
                                    }
                                }
                            }
                            if parsed["method"] == "turn/interrupt" || (selects && protocol.lock().receiver().is_some()) {
                                native_plan = Default::default();
                            }
                        }
                        // Same lock order as receiver(): selection, then protocol.
                        let mut current = selected.lock();
                        protocol.lock().from_terminal(&parsed);
                        if selects {
                            current.connection = Some(connection);
                            current.protocol = protocol.clone();
                            current.disconnected = false;
                        }
                    }
                    write_frame(&mut server, frame).await?;
                }
                frame = server.next() => {
                    let Some(frame) = frame else { return Err("Codex app-server disconnected".into()) };
                    let frame = frame.map_err(|e| e.to_string())?;
                    let mut acceptance = None;
                    if let Message::Text(text) = &frame {
                        let parsed: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
                        if parsed.get("method").is_none() && parsed["id"].as_str().is_some_and(|id| question_interrupts.remove(id)) { continue; }
                        if parsed["method"] == "item/tool/requestUserInput"
                            && selected.lock().connection == Some(connection)
                            && protocol.lock().owns_thread(&parsed)
                            && let Some(plan) = &planning
                            && plan.pending() && plan.valid() {
                            let batch = cide_agents::native_questions::questions(&parsed["params"]["questions"], cide_agents::native_questions::Dialect::Codex)?;
                            let key = parsed["id"].to_string();
                            if let std::collections::hash_map::Entry::Vacant(entry) = questions.entry(key.clone()) {
                                let question = plan.ask(parsed["params"]["threadId"].as_str().ok_or("Missing question thread")?, &cide_agents::native_questions::codex_key(&parsed, &question_scope), batch)?;
                                entry.insert((parsed["id"].clone(), question));
                            }
                        }
                        if parsed["method"] == "serverRequest/resolved"
                            && protocol.lock().owns_thread(&parsed)
                            && let Some((_, question)) = questions.remove(&parsed["params"]["requestId"].to_string()) {
                            question.finish();
                            observe(vec![Hook { kind:"PostToolUse", payload:json!({"session_id":question.source.conversation,"tool_name":"request_user_input"}) }]);
                        }
                        if parsed["method"] == "turn/completed" && protocol.lock().owns_thread(&parsed) { questions.clear(); }
                        if parsed.get("method").is_none() && plan_starts.remove(&parsed["id"].to_string()) && parsed.get("error").is_some() {
                            if let Some(plan) = &planning { plan.fail("Codex rejected the native Plan-mode turn"); }
                            return Err("Codex rejected native Plan mode".into());
                        }
                        // Some Codex versions announce the implementation turn before
                        // replying to our turn/start request. Its start is authoritative
                        // even if that acknowledgement is delayed or omitted.
                        if plan_ack.is_some()
                            && parsed["method"] == "turn/started"
                            && protocol.lock().owns_thread(&parsed)
                            && let Some(plan) = &planning {
                            plan.transition(cide_agents::native_plan::Phase::Implementing);
                        }
                        if plan_ack.as_deref().is_some_and(|id| parsed["id"].as_str() == Some(id)) && parsed.get("method").is_none() {
                            plan_ack = None;
                            protocol.lock().from_server(&parsed);
                            if let Some(plan) = &planning {
                                if parsed.get("error").is_some() {
                                    plan.fail("Codex rejected its native plan acceptance");
                                    return Err("Codex rejected native plan acceptance".into());
                                }
                                plan.transition(cide_agents::native_plan::Phase::Implementing);
                            }
                            continue;
                        }
                        acceptance = if let Some(plan) = &planning
                            && plan.pending() && plan.valid()
                            && selected.lock().connection == Some(connection)
                            && protocol.lock().owns_thread(&parsed) {
                            let id = format!("cide-plan-{}", SessionId::new());
                            native_plan.observe(&parsed, &id).inspect(|_| {
                                plan_ack = Some(id);
                                plan.transition(cide_agents::native_plan::Phase::Approving);
                            })
                        } else { None };
                        if let Some(h)=&decision {
                            if parsed.get("method").is_none() && let Some((record,original,model))=decision_pending.remove(&parsed["id"].to_string()) {
                                h.status(record,if parsed.get("error").is_some(){"rejected"}else{"applied"},"Native harness acknowledgement");
                                if parsed.get("error").is_some() && let Some(original)=original {
                                    write_frame(&mut server,Message::Text(original.to_string().into())).await?;
                                    continue;
                                }
                                if parsed.get("error").is_none() && let Some(model)=model {h.selected_model(&model);}
                                if parsed["id"].as_str().is_some_and(|id|id.starts_with("cide-decision-")){continue;}
                            }
                            if parsed.get("method").is_none() && let Some(record)=research_pending.remove(&parsed["id"].to_string()) {
                                h.status(record,if parsed.get("error").is_some(){"rejected"}else{"applied"},"Native Codex user-turn acknowledgement");
                            }
                            let owns = {
                                let current = selected.lock();
                                current.connection == Some(connection) && protocol.lock().owns_thread(&parsed)
                            };
                            if owns {
                                if parsed["method"]=="turn/started" {
                                    active_thread=parsed["params"]["threadId"].as_str().unwrap_or_default().into();
                                    active_turn=parsed["params"]["turn"]["id"].as_str().unwrap_or_default().into();
                                }
                                let event = decision_output(&protocol.lock(), &parsed);
                                if let Some(event)=event {
                                    let tool=event.kind=="tool";let complete=event.kind=="complete";
                                    if h.event(event) && (tool||complete) {
                                        let h=h.clone();let send=decisions.clone();
                                        tokio::spawn(async move {let action=if complete{h.completion().await}else{h.after_tool().await};if let Some(a)=action{let _=send.try_send(a);}});
                                    }
                                }
                            }
                        }
                        if parsed.get("method").is_none() && let Some(id) = parsed["id"].as_str().filter(|id| id.starts_with("cide-peer-")) {
                            if let Some(request) = pending.remove(id) {
                                let result = protocol::acknowledgement(&parsed).map(|_| true);
                                let _ = request.reply.send(result);
                            }
                            continue;
                        }
                        let current = selected.lock();
                        let hooks = native_plan_hooks(&mut protocol.lock(), &parsed, acceptance.is_some());
                        let owns = current.connection == Some(connection);
                        drop(current);
                        if owns && !hooks.is_empty() { observe(hooks); }
                    }
                    write_frame(&mut terminal, frame).await?;
                    if let Some(request) = acceptance
                        && planning.as_ref().is_some_and(|plan| plan.valid()) {
                            protocol.lock().from_terminal(&request);
                            write_frame(&mut server, Message::Text(request.to_string().into())).await?;
                    }
                }
                Some(action) = actions.recv() => {
                    let Some(h)=&decision else{continue};
                    if !h.can_apply(action.id, action.generation) || selected.lock().connection!=Some(connection) {h.status(action.id,"stale","Session or settings changed before delivery");continue;}
                    let id=format!("cide-decision-{}",SessionId::new());
                    let frame=if action.continuation {
                        let receiver=protocol.lock().receiver();
                        let Some(receiver)=receiver else {h.status(action.id,"stale","The terminal is busy or awaiting input");continue;};
                        if !protocol.lock().claim(&receiver){continue;}
                        protocol::turn_request(&id,&receiver,&action.text)
                    } else {
                        if active_thread.is_empty()||active_turn.is_empty(){continue;}
                        json!({"id":id,"method":"turn/steer","params":{"threadId":active_thread,"expectedTurnId":active_turn,"input":[{"type":"text","text":action.text}]}})
                    };
                    decision_pending.insert(json!(id).to_string(),(action.id,None,None));
                    h.status(action.id,"delivered","Sent through the native Codex connection");
                    write_frame(&mut server,Message::Text(frame.to_string().into())).await?;
                }
                Some(submission) = submissions.recv() => {
                    if submission.expired.load(Ordering::SeqCst) { continue; }
                    if let Err(error) = (submission.valid)() {
                        let _ = submission.reply.send(Err((false, error)));
                        continue;
                    }
                    let claimed = {
                        let current = selected.lock();
                        current.connection == Some(connection) && protocol.lock().claim(&submission.receiver)
                    };
                    if !claimed {
                        let _ = submission.reply.send(Ok(false));
                        continue;
                    }
                    if let Some(h)=&decision {h.event(cide_core::decision::Event{kind:"prompt".into(),text:submission.body.clone(),..Default::default()});}
                    let id = format!("cide-peer-{}", SessionId::new());
                    let frame = protocol::turn_request(&id, &submission.receiver, &submission.body);
                    pending.insert(id, Pending { deadline: Instant::now() + ACK_TIMEOUT, reply: submission.reply });
                    write_frame(&mut server, Message::Text(frame.to_string().into())).await?;
                }
                _ = tick.tick() => {
                    questions.retain(|_, (_, question)| question.active());
                    for (id, question) in questions.values() {
                        if selected.lock().connection != Some(connection) { break; }
                        let Some(response) = question.response() else { continue; };
                        if !question.claim() { continue; }
                        if let Some(reply) = cide_agents::native_questions::codex_reply(id, &question.questions, &response) {
                            if let Some(h) = &decision && let Some(event) = decision_input(&protocol.lock(), true, false, &reply) { h.event(event); }
                            protocol.lock().from_terminal(&reply);
                            answered_questions.insert(id.to_string());
                            write_frame(&mut server, Message::Text(reply.to_string().into())).await?;
                        } else {
                            let id = format!("cide-question-cancel-{}", SessionId::new());
                            question_interrupts.insert(id.clone());
                            let frame = json!({"id":id,"method":"turn/interrupt","params":{"threadId":question.source.conversation}});
                            write_frame(&mut server, Message::Text(frame.to_string().into())).await?;
                            question.retire("The user cancelled the native question.");
                            question.cancel_turn();
                        }
                    }
                    let now = Instant::now();
                    pending.retain(|_, request| {
                        if now < request.deadline { return true; }
                        let _ = request.reply.send(Err((true, "Codex did not acknowledge the peer message. Inspect the receiving conversation before resending.".into())));
                        false
                    });
                }
            }
        }
    }.await;
    for (_, request) in pending {
        let _ = request.reply.send(Err((true, "Codex disconnected after submission; acceptance is uncertain. Inspect the conversation before resending.".into())));
    }
    if outcome.is_err()
        && let Some(plan) = &planning
        && plan.pending()
        && plan.valid()
    {
        plan.fail("the native Codex connection was lost; plan acceptance was not replayed");
    }
    outcome
}

fn native_plan_hooks(protocol: &mut Protocol, frame: &Value, accepting: bool) -> Vec<Hook> {
    let mut hooks = protocol.from_server(frame);
    // Acceptance is sent before the queued hook applier necessarily catches up. Never
    // let the planning Stop release the slot after implementation has advanced the phase.
    if accepting {
        hooks.retain(|hook| hook.kind != "Stop");
    }
    hooks
}

fn decision_input(
    protocol: &Protocol,
    selected: bool,
    selects: bool,
    frame: &Value,
) -> Option<cide_core::decision::Event> {
    let mut event = cide_core::decision::Event::default();
    if selects {
        event.kind = "stop".into();
    } else if !selected {
        return None;
    } else if protocol.resolves_last_question(frame) {
        event.kind = "resumed".into();
    } else if protocol.owns_thread(frame) {
        match frame["method"].as_str()? {
            "turn/start" | "turn/steer" => {
                event.kind = "prompt".into();
                event.text = frame["params"]["input"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
            }
            "turn/interrupt" | "thread/unsubscribe" => event.kind = "stop".into(),
            _ => return None,
        }
    } else {
        return None;
    }
    Some(event)
}

fn decision_output(protocol: &Protocol, frame: &Value) -> Option<cide_core::decision::Event> {
    if !protocol.owns_thread(frame) {
        return None;
    }
    if protocol.resolves_last_question(frame) {
        return Some(cide_core::decision::Event {
            kind: "resumed".into(),
            ..Default::default()
        });
    }
    // Native tool execution also uses server requests. It must remain able to call
    // cide_context while running; only human questions suspend decision inference.
    if frame.get("id").is_some() && protocol::requests_user_input(frame) {
        return Some(cide_core::decision::Event {
            kind: "waiting".into(),
            ..Default::default()
        });
    }
    crate::decision::codex_event(frame)
}

fn read_policy(
    client: &mut tungstenite::WebSocket<TcpStream>,
    cwd: &std::path::Path,
    planning: bool,
) -> Result<Value, String> {
    fn request(
        client: &mut tungstenite::WebSocket<TcpStream>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        client
            .send(Message::Text(
                json!({"id": id, "method": method, "params": params})
                    .to_string()
                    .into(),
            ))
            .map_err(|e| e.to_string())?;
        loop {
            if Instant::now() >= deadline {
                return Err("Codex configuration request timed out".into());
            }
            let frame = client.read().map_err(|e| e.to_string())?;
            if let Message::Text(text) = frame {
                let frame: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                if frame["id"] == id {
                    if let Some(error) = frame.get("error") {
                        return Err(error.to_string());
                    }
                    return Ok(frame["result"].clone());
                }
            }
        }
    }
    request(
        client,
        0,
        "initialize",
        if planning {
            json!({"clientInfo":{"name":"cide_native_console","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})
        } else {
            json!({"clientInfo":{"name":"cide_native_console","version":env!("CARGO_PKG_VERSION")}})
        },
    )?;
    client
        .send(Message::Text(
            json!({"method":"initialized"}).to_string().into(),
        ))
        .map_err(|e| e.to_string())?;
    let config = request(
        client,
        1,
        "config/read",
        json!({"includeLayers":false,"cwd":cwd}),
    )?["config"]
        .clone();
    if !config.is_object() {
        return Err("Codex did not return its effective configuration".into());
    }
    if planning {
        let modes = request(client, 2, "collaborationMode/list", json!({}))
            .map_err(|error| format!("Native Plan mode unavailable: {error}"))?;
        if !modes["data"]
            .as_array()
            .is_some_and(|modes| modes.iter().any(|mode| mode["mode"] == "plan"))
        {
            return Err(
                "Native Plan mode unavailable: Codex exposes no Plan collaboration preset".into(),
            );
        }
    }
    let _ = client.close(None);
    Ok(config)
}

async fn write_frame<S>(socket: &mut S, frame: Message) -> Result<(), String>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    tokio::time::timeout(ACK_TIMEOUT, socket.send(frame))
        .await
        .map_err(|_| "Codex console write timed out".to_string())?
        .map_err(|e| e.to_string())
}

pub(crate) fn observer(app: AppHandle, routing: SessionId) -> Observe {
    let previous = Arc::new(Mutex::new(None::<String>));
    Arc::new(move |hooks| {
        for mut hook in hooks {
            if hook.kind == "SessionStart" {
                let id = hook.payload["session_id"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let changed = previous
                    .lock()
                    .replace(id.clone())
                    .is_some_and(|old| old != id);
                if changed {
                    crate::peer_chat::conversation_changed(&app, routing);
                }
                hook.payload["cide_activation_ms"] = json!(cide_core::persist::now_ms());
            }
            if hook.kind == "CideUserPrompt" {
                crate::peer_chat::note_codex_prompt(&app, routing);
                hook.kind = "UserPromptSubmit";
            }
            if hook.kind == "CideAwaitingInput" {
                hook.kind = "PreToolUse";
                hook.payload["tool_name"] = json!("request_user_input");
            }
            if let Some(hooks) = app.try_state::<crate::hooks::HookServer>() {
                hooks.codex_native_frame(routing, hook.kind, hook.payload);
            }
        }
    })
}

#[cfg(test)]
#[path = "codex_console_tests.rs"]
mod tests;
