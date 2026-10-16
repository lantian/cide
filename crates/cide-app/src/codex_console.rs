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
    accept_hdr_async, connect_async,
    tungstenite::{self, Message, client::IntoClientRequest, http::StatusCode},
};

const TOKEN_ENV: &str = "CIDE_CODEX_CONSOLE_TOKEN";
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
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
    pub fn start(mut spec: SpawnSpec, observe: Observe) -> Result<(SpawnSpec, Arc<Self>), String> {
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
                    match read_policy(&mut client, &spec.cwd) {
                        Ok(config) => {
                            policy = protocol::thread_policy(&config, &spec.args);
                            true
                        }
                        Err(_) => false,
                    }
                });
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
                            server_url,
                            token,
                            protocol,
                            receive,
                            shutdown_rx,
                            observe,
                            policy,
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
    server_url: String,
    token: String,
    selected: Arc<Mutex<Selection>>,
    mut submissions: channel::UnboundedReceiver<Submission>,
    mut shutdown: watch::Receiver<bool>,
    observe: Observe,
    policy: Value,
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
                let server_url = server_url.clone();
                let token = token.clone();
                let selected = selected.clone();
                let shutdown = shutdown.clone();
                let observe = observe.clone();
                let policy = policy.clone();
                workers.spawn(async move {
                    let result = forward_connection(stream, id, server_url, token, selected.clone(), receive, shutdown, observe, policy).await;
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
                if selected.lock().connection != Some(submission.receiver.connection) {
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
    server_url: String,
    token: String,
    selected: Arc<Mutex<Selection>>,
    mut submissions: channel::UnboundedReceiver<Submission>,
    mut shutdown: watch::Receiver<bool>,
    observe: Observe,
    policy: Value,
) -> Result<(), String> {
    let credential = format!("Bearer {token}");
    let handshake = accept_hdr_async(
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
        connected = connect_async(request) => connected.map_err(|e| e.to_string())?,
    };
    let protocol = Arc::new(Mutex::new(Protocol::new(connection)));
    let mut pending: HashMap<String, Pending> = HashMap::new();
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
                        let selects = protocol::apply_policy(&mut parsed, &policy);
                        if selects {
                            frame = Message::Text(parsed.to_string().into());
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
                    if let Message::Text(text) = &frame {
                        let parsed: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
                        if parsed.get("method").is_none() && let Some(id) = parsed["id"].as_str().filter(|id| id.starts_with("cide-peer-")) {
                            if let Some(request) = pending.remove(id) {
                                let result = protocol::acknowledgement(&parsed).map(|_| true);
                                let _ = request.reply.send(result);
                            }
                            continue;
                        }
                        let current = selected.lock();
                        let hooks = protocol.lock().from_server(&parsed);
                        let owns = current.connection == Some(connection);
                        drop(current);
                        if owns && !hooks.is_empty() { observe(hooks); }
                    }
                    write_frame(&mut terminal, frame).await?;
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
                    let id = format!("cide-peer-{}", SessionId::new());
                    let frame = protocol::turn_request(&id, &submission.receiver, &submission.body);
                    pending.insert(id, Pending { deadline: Instant::now() + ACK_TIMEOUT, reply: submission.reply });
                    write_frame(&mut server, Message::Text(frame.to_string().into())).await?;
                }
                _ = tick.tick() => {
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
    outcome
}

fn read_policy(
    client: &mut tungstenite::WebSocket<TcpStream>,
    cwd: &std::path::Path,
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
        json!({"clientInfo":{"name":"cide_native_console","version":env!("CARGO_PKG_VERSION")}}),
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
