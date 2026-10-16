use super::*;
use std::io::{BufRead, BufReader, Read, Write};

const THREAD: &str = "00000000-0000-4000-8000-000000000001";

fn read_json(socket: &mut tungstenite::WebSocket<TcpStream>) -> Value {
    loop {
        if let Message::Text(text) = socket.read().unwrap() {
            return serde_json::from_str(&text).unwrap();
        }
    }
}
fn send_json(socket: &mut tungstenite::WebSocket<TcpStream>, frame: Value) {
    socket
        .send(Message::Text(frame.to_string().into()))
        .unwrap();
}

#[test]
fn forwarding_preserves_native_approvals_and_consumes_only_peer_acknowledgements() {
    let upstream = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let upstream_url = format!("ws://{}", upstream.local_addr().unwrap());
    let proxy = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let proxy_addr = proxy.local_addr().unwrap();
    proxy.set_nonblocking(true).unwrap();
    let protocol = Arc::new(Mutex::new(Selection::default()));
    let state = protocol.clone();
    let (submit, receive) = channel::unbounded_channel();
    let (stop, shutdown) = watch::channel(false);
    let (peer_sent, peer_seen) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(forward(
                proxy,
                ForwardContext {
                    server_url: upstream_url,
                    token: "test-secret".into(),
                    selected: protocol,
                    observe: Arc::new(|_| {}),
                    policy: json!({}),
                },
                receive,
                shutdown,
            ))
    });
    let server = std::thread::spawn(move || {
        let mut server = tungstenite::accept(upstream.accept().unwrap().0).unwrap();
        let initialize = read_json(&mut server);
        assert_eq!(initialize["method"], "initialize");
        send_json(&mut server, json!({"id":0,"result":{}}));
        assert_eq!(read_json(&mut server)["method"], "initialized");
        let start = read_json(&mut server);
        assert_eq!(start["method"], "thread/start");
        send_json(
            &mut server,
            json!({"id":1,"result":{"thread":{"id":THREAD,"status":{"type":"idle"}}}}),
        );
        let peer = read_json(&mut server);
        assert_eq!(peer["method"], "turn/start");
        assert_eq!(peer["params"]["input"][0]["text"], "Peer Привет 世界");
        peer_sent.send(peer.clone()).unwrap();
        send_json(
            &mut server,
            json!({"id":peer["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}),
        );
        send_json(
            &mut server,
            json!({"id":200,"method":"item/commandExecution/requestApproval","params":{"threadId":THREAD,"turnId":"turn-1","command":"true"}}),
        );
        let answer = read_json(&mut server);
        assert_eq!(
            answer,
            json!({"id":200,"result":{"decision":"decline"}}),
            "the bridge must not answer an approval itself"
        );
        send_json(
            &mut server,
            json!({"method":"serverRequest/resolved","params":{"threadId":THREAD,"requestId":200}}),
        );
        send_json(
            &mut server,
            json!({"method":"turn/completed","params":{"threadId":THREAD}}),
        );
        let rejected = read_json(&mut server);
        assert_eq!(rejected["params"]["input"][0]["text"], "rejected");
        send_json(
            &mut server,
            json!({"id":rejected["id"],"error":{"code":-32600,"message":"fixture rejection"}}),
        );
        send_json(
            &mut server,
            json!({"method":"thread/status/changed","params":{"threadId":THREAD,"status":{"type":"idle"}}}),
        );
        let timed_out = read_json(&mut server);
        assert_eq!(timed_out["params"]["input"][0]["text"], "timeout");
        std::thread::sleep(ACK_TIMEOUT + Duration::from_millis(150));
        send_json(
            &mut server,
            json!({"id":timed_out["id"],"result":{"turn":{"id":"late","status":"completed"}}}),
        );
        send_json(
            &mut server,
            json!({"method":"turn/completed","params":{"threadId":THREAD}}),
        );
        let lost = read_json(&mut server);
        assert_eq!(lost["params"]["input"][0]["text"], "disconnect");
        // Drop after the write and before its acknowledgement. No retry is permitted.
    });

    let unauthorized = tungstenite::client(
        format!("ws://{proxy_addr}"),
        TcpStream::connect(proxy_addr).unwrap(),
    );
    assert!(unauthorized.is_err());
    let stream = TcpStream::connect(proxy_addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (mut terminal, _) = tungstenite::client(
        authenticated(&format!("ws://{proxy_addr}"), "test-secret").unwrap(),
        stream,
    )
    .unwrap();
    send_json(
        &mut terminal,
        json!({"id":0,"method":"initialize","params":{"clientInfo":{"name":"test","version":"1"}}}),
    );
    assert_eq!(read_json(&mut terminal)["id"], 0);
    send_json(&mut terminal, json!({"method":"initialized"}));
    send_json(&mut terminal, json!({"id":1,"method":"thread/start"}));
    assert_eq!(read_json(&mut terminal)["id"], 1);
    let receiver = state.lock().receiver().unwrap();

    // Cancellation is checked in the forwarding loop, immediately before a native write.
    let (reply, ack) = mpsc::channel();
    submit
        .send(Submission {
            receiver: receiver.clone(),
            body: "cancelled".into(),
            valid: Box::new(|| Err("cancelled".into())),
            expired: Arc::new(AtomicBool::new(false)),
            reply,
        })
        .unwrap();
    assert!(
        !ack.recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err()
            .0
    );
    let (reply, ack) = mpsc::channel();
    submit
        .send(Submission {
            receiver,
            body: "Peer Привет 世界".into(),
            valid: Box::new(|| Ok(())),
            expired: Arc::new(AtomicBool::new(false)),
            reply,
        })
        .unwrap();
    let peer = peer_seen.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(peer["id"].as_str().unwrap().starts_with("cide-peer-"));
    assert_eq!(ack.recv_timeout(Duration::from_secs(2)).unwrap(), Ok(true));
    let approval = read_json(&mut terminal);
    assert_eq!(
        approval["method"], "item/commandExecution/requestApproval",
        "internal turn replies must not leak into the native client's response table"
    );
    assert!(state.lock().receiver().is_none());
    send_json(
        &mut terminal,
        json!({"id":200,"result":{"decision":"decline"}}),
    );
    assert_eq!(read_json(&mut terminal)["method"], "serverRequest/resolved");
    assert_eq!(read_json(&mut terminal)["method"], "turn/completed");
    assert!(state.lock().receiver().is_some());
    for (body, uncertain, event) in [
        ("rejected", false, Some("thread/status/changed")),
        ("timeout", true, Some("turn/completed")),
        ("disconnect", true, None),
    ] {
        let receiver = state.lock().receiver().unwrap();
        let (reply, ack) = mpsc::channel();
        submit
            .send(Submission {
                receiver,
                body: body.into(),
                valid: Box::new(|| Ok(())),
                expired: Arc::new(AtomicBool::new(false)),
                reply,
            })
            .unwrap();
        let result = ack
            .recv_timeout(ACK_TIMEOUT + Duration::from_secs(2))
            .unwrap();
        assert_eq!(result.unwrap_err().0, uncertain);
        if let Some(event) = event {
            assert_eq!(
                read_json(&mut terminal)["method"],
                event,
                "late or rejected internal acknowledgements must not reach the terminal"
            );
            assert!(state.lock().receiver().is_some());
        } else {
            assert!(terminal.read().is_err());
        }
    }
    stop.send_replace(true);
    drop(terminal);
    assert!(worker.join().unwrap().is_ok());
    server.join().unwrap();
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("cide-codex-native-audit-{}", SessionId::new()));
        std::fs::create_dir_all(path.join("state")).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Provider {
    port: u16,
    requests: Arc<Mutex<Vec<Value>>>,
    hold: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Provider {
    fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let hold = Arc::new(AtomicBool::new(false));
        let held = hold.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                drop(reader);
                if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                    // Native title/suggestion helpers can also use the fake provider. Count
                    // the actual peer turns, excluding those independent structured tasks.
                    let peer = value["text"]["format"]["type"] != "json_schema"
                        && value["input"].as_array().is_some_and(|input| {
                            input.iter().any(|item| {
                                item["role"] == "user"
                                    && item["content"].as_array().is_some_and(|content| {
                                        content.iter().any(|part| {
                                            part["text"].as_str().is_some_and(|text| {
                                                text.starts_with("Chat from Peer")
                                            })
                                        })
                                    })
                            })
                        });
                    if peer {
                        captured.lock().push(value);
                    }
                }
                while held.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let item = json!({"type":"message","id":"msg-audit","role":"assistant","content":[{"type":"output_text","text":"Native peer reply received."}]});
                let events = [
                    json!({"type":"response.created","response":{"id":"resp-audit","status":"in_progress","output":[]}}),
                    json!({"type":"response.output_item.done","output_index":0,"item":item}),
                    json!({"type":"response.completed","response":{"id":"resp-audit","status":"completed","output":[item],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}),
                ];
                let data = events
                    .into_iter()
                    .map(|event| format!("data: {event}\n\n"))
                    .collect::<String>();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",
                    data.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            port,
            requests,
            hold,
            stopped,
            thread: Some(thread),
        }
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.hold.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

struct Native {
    console: Arc<Console>,
    tui: Arc<PtySession>,
    output: mpsc::Receiver<Vec<u8>>,
}
impl Native {
    fn start(dir: &Temp, provider: &Provider, resume: Option<&str>) -> Self {
        let mut spec =
            SpawnSpec::new("codex", &dir.0).apply(cide_core::child_env::terminal_child_env(
                &Default::default(),
                env!("CARGO_PKG_VERSION"),
                Vec::new(),
            ));
        spec.geometry = cide_pty::Geometry::new(120, 38, 8, 16);
        spec = spec
            .env("CODEX_HOME", dir.0.join("state").to_string_lossy())
            .env("CIDE_PEER_AUDIT_KEY", "unused-local-test-key")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .env_remove("OPENAI_BASE_URL");
        if resume.is_some() {
            spec.args.push("resume".into());
            spec.args.push("--include-non-interactive".into());
        }
        spec.args.extend([
            "-C".into(),
            dir.0.to_string_lossy().into_owned(),
            "-s".into(),
            "read-only".into(),
            "-a".into(),
            "on-request".into(),
        ]);
        for config in [
            "check_for_update_on_startup=false".into(),
            "model_provider=\"peer-audit\"".into(),
            "model=\"gpt-5\"".into(),
            "model_providers.peer-audit.name=\"Peer audit\"".into(),
            format!(
                "model_providers.peer-audit.base_url=\"http://127.0.0.1:{}/v1\"",
                provider.port
            ),
            "model_providers.peer-audit.env_key=\"CIDE_PEER_AUDIT_KEY\"".into(),
            "model_providers.peer-audit.wire_api=\"responses\"".into(),
            "model_providers.peer-audit.request_max_retries=0".into(),
            "model_providers.peer-audit.stream_max_retries=0".into(),
            "developer_instructions=\"Read-only native peer audit fixture.\"".into(),
        ] {
            spec.args.extend(["-c".into(), config]);
        }
        if let Some(thread) = resume {
            spec.args.push(thread.into());
        }
        let (output, receive) = mpsc::channel();
        spec.output_observer = Some(cide_pty::OutputObserver::new(move |bytes| {
            let _ = output.send(bytes.to_vec());
        }));
        let (spec, console) = Console::start(spec, Arc::new(|_| {})).unwrap();
        let tui = PtySession::spawn(spec).unwrap();
        console.observe(&tui);
        Self {
            console,
            tui,
            output: receive,
        }
    }
    fn pump(&self) {
        while let Ok(bytes) = self.output.try_recv() {
            if bytes.windows(4).any(|window| window == b"\x1b[6n") {
                self.tui.write(b"\x1b[1;1R".to_vec());
            }
        }
    }
    fn screen(&self) -> String {
        self.tui
            .capture_screen()
            .lines
            .iter()
            .map(|line| {
                line.runs
                    .iter()
                    .map(|run| run.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn wait(&self, label: &str, predicate: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut trust_sent = false;
        loop {
            self.pump();
            if predicate() {
                return;
            }
            let screen = self.screen();
            // This is only the audit's newly created directory and isolated CODEX_HOME.
            // Production never answers a native trust or permission prompt.
            if !trust_sent && screen.contains("Trust and continue") {
                std::thread::sleep(Duration::from_millis(200));
                self.tui.write(vec![b'\r']);
                trust_sent = true;
            }
            assert!(
                !self.tui.has_exited() && Instant::now() < deadline,
                "{label}: {:?}\n{screen}\nexit: {:?}; server: {:?}",
                self.console.protocol.lock(),
                self.tui.exit_status(),
                self.console.server.capture_screen()
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.console.stop();
        self.tui.kill();
    }
}

#[test]
#[ignore = "installed Codex, local fake provider and isolated state only; no model quota"]
fn real_codex_native_delivery_preserves_drafts_and_restores_the_thread() {
    let dir = Temp::new();
    let provider = Provider::new();
    let native = Native::start(&dir, &provider, None);
    native.wait("fresh terminal receiver", || {
        native.console.receiver().unwrap().is_some()
    });
    let original = native.console.receiver().unwrap().unwrap();
    let draft = "LOCAL_DRAFT_MUST_SURVIVE";
    native.tui.write(draft.as_bytes().to_vec());
    native.wait("manual draft", || native.screen().contains(draft));
    let body = format!(
        "Chat from Peer (opencode): {} Привет 世界 😀",
        "finding ".repeat(1500)
    );
    provider.hold.store(true, Ordering::SeqCst);
    assert_eq!(
        native
            .console
            .peer_prompt(original.clone(), body.clone(), Box::new(|| Ok(()))),
        Ok(true)
    );
    native.wait("local provider request", || {
        !provider.requests.lock().is_empty()
    });
    assert!(native.console.receiver().unwrap().is_none());
    assert_eq!(
        native
            .console
            .peer_prompt(original, "must wait".into(), Box::new(|| Ok(()))),
        Ok(false)
    );
    assert_eq!(provider.requests.lock().len(), 1);
    let request = provider.requests.lock()[0].clone();
    assert!(
        request
            .to_string()
            .contains("Read-only native peer audit fixture.")
    );
    assert!(request["input"].as_array().unwrap().iter().any(|item| {
        item["role"] == "user"
            && item["content"]
                .as_array()
                .is_some_and(|content| content.iter().any(|part| part["text"] == body))
    }));
    provider.hold.store(false, Ordering::SeqCst);
    native.wait("completed native turn", || {
        native.console.receiver().unwrap().is_some()
    });
    native.wait("preserved draft", || {
        cide_pty::codex_clear::composer(&native.tui.capture_screen())
            .is_some_and(|text| text.contains(draft))
    });
    let thread = native.console.receiver().unwrap().unwrap().thread;
    assert!(
        cide_core::codex_cli::rollout_of(&dir.0.join("state"), &thread).is_some(),
        "native peer conversations must remain discoverable by Cide's restore path"
    );
    drop(native);
    let restored = Native::start(&dir, &provider, Some(&thread));
    restored.wait("restored native thread", || {
        restored
            .console
            .receiver()
            .is_ok_and(|receiver| receiver.is_some())
    });
    assert_eq!(restored.console.receiver().unwrap().unwrap().thread, thread);
    assert_eq!(
        provider.requests.lock().len(),
        1,
        "restore must not replay a message"
    );
    let receiver = restored.console.receiver().unwrap().unwrap();
    assert_eq!(
        restored.console.peer_prompt(
            receiver,
            "Chat from Peer: second message".into(),
            Box::new(|| Ok(()))
        ),
        Ok(true)
    );
    restored.wait("second native turn", || {
        provider.requests.lock().len() == 2 && restored.console.receiver().unwrap().is_some()
    });
    let retired = restored.console.receiver().unwrap().unwrap();
    restored.tui.write(b"/new".to_vec());
    restored.wait("new command", || restored.screen().contains("/new"));
    restored.tui.write(vec![b'\r']);
    restored.wait("replacement native conversation", || {
        restored
            .console
            .receiver()
            .unwrap()
            .is_some_and(|receiver| receiver.thread != retired.thread)
    });
    assert_eq!(
        restored.console.peer_prompt(
            retired,
            "must not reach retired conversation".into(),
            Box::new(|| Ok(()))
        ),
        Ok(false)
    );
    assert_eq!(provider.requests.lock().len(), 2);
    let server = restored.console.server.clone();
    let credentials = restored.console._credentials.0.clone();
    drop(restored);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !server.has_exited() || credentials.exists() {
        assert!(
            Instant::now() < deadline,
            "closing the native pane must stop its server and remove credentials"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
