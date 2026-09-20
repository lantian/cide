//! The public road for an attached server (M59): a `connect` definition is installed, located
//! without touching `PATH`, refused without its project marker, and driven through
//! `LspHandle::start_with` to `Ready` against a fake listener — with no pid at any point.
//!
//! Its own binary, for `registry.rs`'s reason: the registry is process-global and this installs
//! into it, so it is one test that owns the process from start to finish.

use std::io::BufReader;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use cide_ipc::SourceStatus;
use cide_ipc::lang::{LanguageServerDef, TcpEndpoint};
use cide_lsp::discover::{Found, Provenance, Target, applicable, find, install, locate};
use cide_lsp::{LspEvent, LspHandle};

/// The `godot` extension's server, as the marketplace manifest spells it, on a port of ours.
fn godot(name: &str, host: &str, port: u16) -> LanguageServerDef {
    LanguageServerDef {
        binary: name.into(),
        args: vec![],
        language_ids: vec!["gdscript".into()],
        project_markers: vec!["project.godot".into()],
        project_kind: "Godot project".into(),
        install_hint: "open this project in the Godot editor".into(),
        declares_watched_files: false,
        extra_path_hints: vec![],
        init_options: None,
        connect: Some(TcpEndpoint {
            host: host.into(),
            port,
        }),
    }
}

#[test]
fn an_attached_server_is_located_without_a_binary_and_reaches_ready() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");

    let mut defs = cide_ipc::lang::builtin_servers();
    defs.push(godot("godot", "127.0.0.1", addr.port()));
    // A builtin definition is never validated by the manifest, so `locate` is the only thing
    // between a mistyped host and the network; this row proves it asks.
    defs.push(godot("godot-elsewhere", "10.0.0.1", addr.port()));
    let handles = install(defs);
    let server = handles[handles.len() - 2];
    let elsewhere = handles[handles.len() - 1];

    let dir = std::env::temp_dir().join(format!("cide-lsp-attach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let roots = vec![dir.clone()];

    // No marker: not applicable, and the sentence says "connected" rather than "started",
    // because nothing here is ever started.
    assert!(!applicable(server, &roots));
    let refused = locate(server, &roots, Default::default()).expect_err("no project");
    assert!(
        refused.contains("No Godot project") && refused.contains("was not connected"),
        "{refused}"
    );

    std::fs::write(dir.join("project.godot"), "config_version=5\n").expect("marker");
    assert!(applicable(server, &roots));
    let candidates = locate(server, &roots, Default::default()).expect("located");
    assert_eq!(
        candidates.len(),
        1,
        "one rung: there is nothing to fall back to"
    );
    assert_eq!(candidates[0].provenance, Provenance::Attached);
    assert!(
        matches!(candidates[0].target, Target::Tcp { addr: found } if found == addr),
        "{:?}",
        candidates[0].target
    );
    let Found::Ready(target) = find(server, &roots) else {
        panic!("find refused a located server");
    };
    assert_eq!(target.to_string(), addr.to_string());

    // The loopback rule, asked again here because the manifest never saw this definition.
    let refused = locate(elsewhere, &roots, Default::default()).expect_err("off loopback");
    assert!(
        refused.contains("not a loopback address") && refused.contains("10.0.0.1"),
        "{refused}"
    );

    // A fake server: one connection, the handshake, then drain until the client hangs up.
    let fake = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut input = BufReader::new(stream.try_clone().expect("clone"));
        let mut output = stream;
        let init = cide_lsp::codec::read_message(&mut input).expect("initialize");
        assert_eq!(init["method"], "initialize");
        assert_eq!(
            init["params"]["processId"],
            serde_json::Value::Null,
            "an attached server is told cide is not its parent"
        );
        cide_lsp::codec::write_message(
            &mut output,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": init["id"],
                "result": { "capabilities": { "textDocumentSync": 1 } },
            }),
        )
        .expect("reply");
        let mut methods = Vec::new();
        while let Ok(message) = cide_lsp::codec::read_message(&mut input) {
            if let Some(method) = message.get("method").and_then(|m| m.as_str()) {
                methods.push(method.to_string());
                // Godot's road for a built-in: the documentation arrives as a notification
                // *during* the request, before the reply — which is why the waiter is registered
                // before the request is sent, and what this ordering exercises.
                if method == "textDocument/declaration" {
                    cide_lsp::codec::write_message(
                        &mut output,
                        &serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "gdscript/show_native_symbol",
                            "params": { "name": "Dictionary", "kind": 5, "native_class": "Dictionary",
                                        "detail": "<Native> class Dictionary",
                                        "documentation": "A [b]dictionary[/b]." },
                        }),
                    )
                    .expect("notification");
                    cide_lsp::codec::write_message(
                        &mut output,
                        &serde_json::json!({ "jsonrpc": "2.0", "id": message["id"], "result": [] }),
                    )
                    .expect("reply");
                }
            }
        }
        methods
    });

    let handle = LspHandle::start_with(
        server,
        roots.clone(),
        Default::default(),
        cide_lsp::config::Tuning::default(),
    )
    .expect("start");
    assert_eq!(handle.pid(), None, "nothing was spawned");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ready = false;
    'wait: while Instant::now() < deadline {
        for event in handle.drain() {
            match event {
                LspEvent::Status(SourceStatus::Ready) => {
                    ready = true;
                    break 'wait;
                }
                LspEvent::Status(SourceStatus::Unavailable { reason }) => {
                    panic!("a listening server was reported unavailable: {reason}");
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready, "the attached server never reached Ready");
    assert_eq!(handle.pid(), None, "and still nothing was spawned");

    // A notification correlated with the request that provoked it (M60): the waiter goes in
    // first, the request second, and the payload comes back through the pending map with no
    // request id to key on.
    let requester = handle.requester();
    let waiter = requester.expect_notification(cide_lsp::docs::GODOT_NATIVE_SYMBOL);
    let reply = requester
        .request(
            "textDocument/declaration",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .expect("declaration reply");
    assert_eq!(
        reply,
        serde_json::json!([]),
        "the reply itself is empty, as Godot's is"
    );
    let payload = waiter
        .recv_timeout(Duration::from_secs(2))
        .expect("the notification arrived")
        .expect("and was a payload, not a cancellation");
    assert_eq!(payload["name"], "Dictionary");
    let page = cide_lsp::docs::godot::page("godot", &payload).expect("a page");
    assert_eq!(page.markdown, "A **dictionary**.");
    assert_eq!(page.reference.as_deref(), Some("#ref/class/Dictionary"));
    // And the whole road through `docs::lookup`, which is what the app calls.
    let outcome = cide_lsp::docs::lookup(
        &requester,
        &dir.join("main.gd"),
        1,
        1,
        "Dictionary",
        Duration::from_secs(5),
    );
    let cide_lsp::docs::Outcome::Page(page) = outcome else {
        panic!("lookup did not produce a page: {outcome:?}");
    };
    assert_eq!(page.title, "Dictionary");

    drop(handle);
    let methods = fake.join().expect("fake server");
    assert!(
        methods.iter().any(|m| m == "shutdown") && !methods.iter().any(|m| m == "exit"),
        "the drop path said `shutdown` and never `exit`: {methods:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
