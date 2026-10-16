//! Installed-CLI proof. The only stand-ins are the model and the desktop event consumer;
//! cide-hook, OpenCode, the five schemas, tool handlers, broker and authentication are real.
use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Rejections(Arc<AtomicUsize>);
impl tracing::Subscriber for Rejections {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::WARN
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().fields().field("presented").is_some() {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

struct Files(Arc<AtomicUsize>);
impl tools::FileOpener for Files {
    fn open(
        &self,
        path: &str,
        start: Option<u32>,
        end: Option<u32>,
    ) -> std::result::Result<(), String> {
        assert!(std::path::Path::new(path).is_file());
        assert_eq!((start, end), (Some(1), Some(2)));
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires installed OpenCode, built cide-hook, PTYs and local sockets; uses a fake local model"]
async fn real_opencode_uses_existing_ide_tools_and_authenticated_discovery() {
    use std::os::unix::fs::DirBuilderExt;
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let hook = workspace
        .join("target/debug/cide-hook")
        .canonicalize()
        .expect("run cargo build --locked -p cide-hook first");
    let root =
        Temp(std::env::temp_dir().join(format!("cide-opencode-ide-{}", uuid::Uuid::new_v4())));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root.0)
        .unwrap();
    let rejected = Arc::new(AtomicUsize::new(0));
    let _trace = tracing::subscriber::set_default(Rejections(rejected.clone()));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let server = Arc::new(IdeServer::attach(listener, "isolated-audit-token".into()).unwrap());
    let mut events = server.events();
    let files = Arc::new(AtomicUsize::new(0));
    server.set_file_opener(Arc::new(Files(files.clone())));
    let broker = server.broker();
    let desktop = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if let ServerEvent::DiffRequested(request) = event {
                assert_eq!(request.pane.as_deref(), Some("audit-pane"));
                broker.resolve(
                    &request.id,
                    protocol::DiffOutcome::Saved {
                        contents: "approved audit text".into(),
                    },
                );
            }
        }
    });
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let socket = root.0.join("mcp.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let rpc = tokio::spawn({
        let server = server.clone();
        let calls = calls.clone();
        async move {
            while let Ok((stream, _)) = listener.accept().await {
                let session = Arc::new(server.tool_session("audit-pane".into()).unwrap());
                let calls = calls.clone();
                tokio::spawn(async move {
                    let (read, write) = stream.into_split();
                    let mut lines = BufReader::new(read).lines();
                    let hello: Value =
                        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(hello["hello"], "cide-mcp");
                    assert_eq!(hello["session"], "ide-audit-session");
                    let write = Arc::new(tokio::sync::Mutex::new(write));
                    let cancelled = Arc::new(AtomicBool::new(false));
                    while let Ok(Some(line)) = lines.next_line().await {
                        let value: Value = serde_json::from_str(&line).unwrap();
                        if value.get("id").is_none() {
                            continue;
                        }
                        let (session, calls, write, cancelled) = (
                            session.clone(),
                            calls.clone(),
                            write.clone(),
                            cancelled.clone(),
                        );
                        tokio::spawn(async move {
                            let result = match value["method"].as_str().unwrap() {
                                "initialize" => initialize_result(&value["params"]),
                                "tools/list" => json!({"tools":tools::descriptors()}),
                                "tools/call" => {
                                    let name = value["params"]["name"].as_str().unwrap();
                                    calls.lock().push(name.into());
                                    session
                                        .call(name, &value["params"]["arguments"], &cancelled)
                                        .await
                                        .to_json()
                                }
                                _ => json!({}),
                            };
                            let line = format!(
                                "{}\n",
                                json!({"jsonrpc":"2.0","id":value["id"],"result":result})
                            );
                            let _ = write.lock().await.write_all(line.as_bytes()).await;
                        });
                    }
                    cancelled.store(true, Ordering::Release);
                    session.close();
                });
            }
        }
    });
    let native_connected = Arc::new(AtomicBool::new(false));
    let watch = tokio::spawn({
        let inner = server.inner.clone();
        let native_connected = native_connected.clone();
        async move {
            loop {
                if inner.connection_count() > 0 {
                    native_connected.store(true, Ordering::Release);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });
    let control = root.0.join("audit.json");
    std::fs::write(
        &control,
        json!({
            "root":root.0,"hook":hook,"socket":socket,"port":server.port(),
            "token":"isolated-audit-token","pid":std::process::id(),
        })
        .to_string(),
    )
    .unwrap();
    let output = tokio::process::Command::new("python3")
        .arg(workspace.join("scripts/audit-opencode-ide.py"))
        .arg(&control)
        .output()
        .await
        .unwrap();
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    rpc.abort();
    let _ = rpc.await;
    watch.abort();
    let _ = watch.await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in protocol::tool::ALL {
        assert!(
            calls.lock().iter().any(|called| called == name),
            "not called: {name}"
        );
    }
    assert!(files.load(Ordering::Relaxed) >= 1);
    assert!(
        native_connected.load(Ordering::Acquire),
        "native lockfile discovery never authenticated"
    );
    assert_eq!(
        rejected.load(Ordering::Relaxed),
        0,
        "OpenCode retried without authorization"
    );
    Arc::try_unwrap(server)
        .ok()
        .expect("audit released its server")
        .shutdown()
        .await;
    desktop.await.unwrap();
}
