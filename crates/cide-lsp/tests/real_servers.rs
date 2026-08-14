//! The tests that decide whether this crate actually works.
//!
//! Every one is `#[ignore]`d, which in this workspace means **it spawns a real binary** — the same
//! convention `cide-claude`'s and `cide-ide-mcp`'s live tests follow. They need `rust-analyzer` or
//! `gopls` installed, they take tens of seconds, and CI does not run them.
//!
//! That is the honest state of it: no automated gate covers the step where the design is either
//! real or not. Run them by hand after touching anything in this crate:
//!
//! ```sh
//! cargo test -p cide-lsp -- --ignored --nocapture
//! ```
//!
//! Each one reaps its child on **every** path — a `LspHandle` dropped at any exit runs the
//! shutdown ladder — because a test that leaks a rust-analyzer leaves 1–4 GB resident on the
//! machine of whoever ran it.

use std::time::{Duration, Instant};

use cide_lsp::{LspEvent, LspHandle, Server};

/// Wait until `f` is satisfied by the events seen so far, or give up.
///
/// Polls rather than blocks, and returns everything seen — so a failure can print what *did*
/// arrive instead of only that something did not.
fn wait_for(
    handle: &LspHandle,
    timeout: Duration,
    mut f: impl FnMut(&[LspEvent]) -> bool,
) -> (bool, Vec<LspEvent>) {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        seen.extend(handle.drain());
        if f(&seen) {
            return (true, seen);
        }
        // A source that has gone `Unavailable` is never coming back — the supervisor has given
        // up. Waiting out the remaining timeout would turn a decided answer into three minutes of
        // sleeping, and then report it as "never reached Ready" rather than as the reason it
        // actually failed. Learned the hard way: an uninstalled rust-analyzer component cost 360
        // seconds and hid its own diagnosis.
        if seen.iter().any(|e| {
            matches!(
                e,
                LspEvent::Status(cide_ipc::SourceStatus::Unavailable { .. })
            )
        }) {
            return (false, seen);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    (false, seen)
}

/// The `Unavailable` reason, if the supervisor gave up. What a failing assertion should print.
fn gave_up(events: &[LspEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        LspEvent::Status(cide_ipc::SourceStatus::Unavailable { reason }) => Some(reason.clone()),
        _ => None,
    })
}

fn ready(events: &[LspEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, LspEvent::Status(cide_ipc::SourceStatus::Ready)))
}

#[test]
#[ignore = "spawns the real rust-analyzer"]
fn a_real_rust_analyzer_indexes_this_workspace_and_reaches_ready() {
    // Pointed at cide itself, which is a genuine multi-crate Cargo workspace — the shape that
    // exercises `workspaceFolders` and the `$/progress` sequence rather than a toy fixture.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();

    let handle = LspHandle::start(Server::RustAnalyzer, vec![root]).expect("start rust-analyzer");

    // 180s: rust-analyzer on a workspace this size is 30–120s to first useful output on a warm
    // cache and slower on a cold one. A shorter timeout makes this test flaky rather than fast.
    let (reached, seen) = wait_for(&handle, Duration::from_secs(180), ready);
    assert!(
        reached,
        "rust-analyzer never reached Ready.{}",
        match gave_up(&seen) {
            // The supervisor's own sentence, which is the actionable half. On a machine where
            // `~/.cargo/bin/rust-analyzer` is a rustup shim with no component behind it, this
            // reads "…could not start. error: Unknown binary… Run `rustup component add
            // rust-analyzer`." — which is the answer, not a symptom.
            Some(reason) => format!(" It gave up: {reason}"),
            None => format!(
                " Seen: {:#?}",
                seen.iter()
                    .filter(|e| matches!(e, LspEvent::Status(_)))
                    .collect::<Vec<_>>()
            ),
        }
    );

    /*
     * `Ready` is reached on progress *ending*, not on a diagnostic arriving. This workspace is
     * clean and — with no document opened — rust-analyzer publishes **nothing at all** for it, so
     * a rule keyed on diagnostics would leave it Scanning for ever.
     *
     * The assertion below is the one that matters, and the weaker version it replaced is the
     * reason: asserting only "Scanning appears before Ready" is trivially true, because
     * `Session::new` emits Scanning the moment it writes `initialize`. It passed while the
     * implementation flapped **eight times** in the first 0.9 seconds — rust-analyzer's indexing
     * is a *sequence* of progress tokens, and the gaps between them looked like "nothing in
     * flight". Every one of those flaps was a clean bill of health over an unindexed workspace.
     *
     * So: no Ready may ever be followed by a Scanning. `READY_SETTLE` is what makes that true,
     * and a regression in it fails here rather than in front of a user.
     */
    let statuses: Vec<&cide_ipc::SourceStatus> = seen
        .iter()
        .filter_map(|e| match e {
            LspEvent::Status(s) => Some(s),
            _ => None,
        })
        .collect();
    let flaps = statuses
        .windows(2)
        .filter(|w| {
            matches!(w[0], cide_ipc::SourceStatus::Ready)
                && matches!(w[1], cide_ipc::SourceStatus::Scanning { .. })
        })
        .count();
    assert_eq!(
        flaps, 0,
        "the source went Ready and then back to Scanning {flaps} time(s) — every one of those \
         announced a clean workspace while the analyser was still indexing it: {statuses:#?}"
    );
    assert!(
        matches!(
            statuses.first(),
            Some(cide_ipc::SourceStatus::Scanning { .. })
        ),
        "the first thing a server says must be that it is starting, not that it is ready"
    );
}

#[test]
#[ignore = "spawns the real rust-analyzer"]
fn a_real_rust_analyzer_reports_a_type_error_we_introduce() {
    // The end-to-end claim, in one gesture: a broken file in a real Cargo project produces a real
    // diagnostic at the right line.
    let dir = std::env::temp_dir().join(format!("cide-lsp-ra-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("mkdir");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn f() -> u32 { \"not a u32\" }\n",
    )
    .expect("write");

    let handle = LspHandle::start(Server::RustAnalyzer, vec![dir.clone()]).expect("start");
    let (found, seen) = wait_for(&handle, Duration::from_secs(180), |events| {
        events.iter().any(|e| {
            matches!(e, LspEvent::Published { items, .. } if items.iter().any(|d| d.message.contains("mismatched types")))
        })
    });

    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        found,
        "no mismatched-types diagnostic arrived.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" rust-analyzer gave up: {reason}"),
            None => format!(" Seen: {seen:#?}"),
        }
    );
}

#[test]
#[ignore = "spawns the real gopls"]
fn a_real_gopls_reports_on_a_module() {
    let dir = std::env::temp_dir().join(format!("cide-lsp-gopls-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("go.mod"), "module probe\n\ngo 1.21\n").expect("write");
    // An undefined reference, which gopls reports without needing the module downloaded.
    std::fs::write(
        dir.join("main.go"),
        "package main\n\nfunc main() {\n\tundefinedCall()\n}\n",
    )
    .expect("write");

    let handle = LspHandle::start(Server::Gopls, vec![dir.clone()]).expect("start gopls");
    let (found, seen) = wait_for(&handle, Duration::from_secs(120), |events| {
        events
            .iter()
            .any(|e| matches!(e, LspEvent::Published { items, .. } if !items.is_empty()))
    });

    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        found,
        "gopls reported nothing.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" It gave up: {reason}"),
            None => format!(" Seen: {seen:#?}"),
        }
    );
}

#[test]
#[ignore = "spawns the real rust-analyzer"]
fn dropping_the_handle_stops_the_server() {
    // The ladder, observed from outside: after a drop, the process is gone. A leaked
    // rust-analyzer is 1–4 GB the user never asked for, and `Drop` is the only thing between
    // them and it.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let handle = LspHandle::start(Server::RustAnalyzer, vec![root]).expect("start");
    // Let it get as far as spawning before pulling the rug out — stopping during the handshake is
    // the interesting case, because that is when the shutdown request has nowhere to go.
    std::thread::sleep(Duration::from_millis(500));
    let before = std::time::Instant::now();
    drop(handle);
    // `Drop` joins the supervisor, which runs the ladder. If it hung, this is where it would.
    assert!(
        before.elapsed() < Duration::from_secs(10),
        "the shutdown ladder did not finish promptly"
    );
}

#[test]
#[ignore = "spawns the real rust-analyzer"]
fn an_on_disk_edit_alone_never_refreshes_diagnostics() {
    // The test that justifies `ui/src/editor/docSync.ts` existing at all.
    //
    // Two facts, and the second is the one that matters:
    //
    // * flycheck runs once when the workspace finishes loading, so a project opens with a real
    //   diagnostic list **without anyone sending `didOpen`** — which is exactly why the missing
    //   document sync looked like a working feature;
    // * repairing the file on disk and telling the server nothing changes nothing, for as long as
    //   you care to wait. We declare no `didChangeWatchedFiles`, and rust-analyzer's fallback
    //   watcher does not re-run flycheck by itself.
    //
    // So the panel would freeze at the state the project opened in while the user edits
    // underneath it. `did_save` is what breaks the freeze, and the frontend is the only thing
    // that can send it.
    //
    // The clearing half is given 120 s and asserted **negative**. If a future rust-analyzer does
    // start watching the disk this test fails, and that failure is the point: it would mean the
    // save notification is no longer the only thing keeping the panel alive.
    let dir = std::env::temp_dir().join(format!("cide-lsp-disk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("mkdir");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    std::fs::write(dir.join("src/lib.rs"), "pub fn f() -> u32 { \"nope\" }\n").expect("write");

    let handle = LspHandle::start(Server::RustAnalyzer, vec![dir.clone()]).expect("start");
    let broken = |events: &[LspEvent]| {
        events.iter().any(|e| {
            matches!(e, LspEvent::Published { items, .. } if items.iter().any(|d| d.message.contains("mismatched types")))
        })
    };
    let (saw_error, seen) = wait_for(&handle, Duration::from_secs(180), broken);

    // Repair it on disk and send the server nothing at all.
    std::fs::write(dir.join("src/lib.rs"), "pub fn f() -> u32 { 7 }\n").expect("write");
    let (cleared, _) = wait_for(&handle, Duration::from_secs(120), |events| {
        events.iter().rev().take(6).any(|e| {
            matches!(e, LspEvent::Published { items, .. } if !items.iter().any(|d| d.message.contains("mismatched types")))
        })
    });

    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        saw_error,
        "flycheck never reported the error on load.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" rust-analyzer gave up: {reason}"),
            None => format!(" Seen: {seen:#?}"),
        }
    );
    assert!(
        !cleared,
        "an on-disk fix cleared the diagnostic with no `didSave` — rust-analyzer now watches the \
         disk itself, and `docSync.ts` is no longer the only thing keeping the panel current"
    );
}

#[test]
#[ignore = "spawns the real rust-analyzer"]
fn a_real_rust_analyzer_resolves_a_definition() {
    // The end-to-end proof for the request path: everything else about `textDocument/definition`
    // is unit-tested against a fake, and a fake cannot show that the id correlation survives a
    // real server's interleaved `$/progress`, `workspace/configuration` and `publishDiagnostics`
    // traffic — which is the whole risk of intercepting replies in the pump.
    let dir = std::env::temp_dir().join(format!("cide-lsp-def-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("mkdir");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    // `target()` is declared on line 1 and used on line 4. Asking about the call site must come
    // back with line 1 — a 0-based reply that was never converted would say 0 or 2.
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn target() -> u32 { 7 }\n\npub fn caller() -> u32 {\n    target()\n}\n",
    )
    .expect("write");

    let handle = LspHandle::start(Server::RustAnalyzer, vec![dir.clone()]).expect("start");
    let (up, seen) = wait_for(&handle, Duration::from_secs(180), ready);
    assert!(
        up,
        "rust-analyzer never reached Ready.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" It gave up: {reason}"),
            None => String::new(),
        }
    );

    // The file has to be open before the server will answer about it — the same prerequisite
    // `docSync.ts` exists to satisfy in the app.
    let uri = cide_lsp::convert::path_to_uri(&dir.join("src/lib.rs"));
    let (session, _) = cide_lsp::Session::new(std::slice::from_ref(&dir), "rust-analyzer");
    let text = std::fs::read_to_string(dir.join("src/lib.rs")).expect("read");
    if let cide_lsp::Effect::Send(message) = session.did_open(uri.clone(), "rust", 1, text) {
        handle.send(message);
    }

    let requester = handle.requester();
    // 0-based on the wire: line 3 is `    target()`, character 4 is the `t`.
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 3, "character": 4 },
    });

    // Retried, because "indexed enough to publish diagnostics" and "indexed enough to resolve a
    // reference" are different readiness questions and rust-analyzer reaches them at different
    // times. A single ask here would be flaky for a reason that has nothing to do with the code
    // under test.
    let mut answer = None;
    for _ in 0..20 {
        match requester.request(
            "textDocument/definition",
            params.clone(),
            Duration::from_secs(10),
        ) {
            Ok(value) => {
                if let Some(found) = cide_lsp::convert::location(&value) {
                    answer = Some(found);
                    break;
                }
            }
            Err(error) => panic!("the request failed: {error}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    }

    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);

    let (path, line, column) = answer.expect("rust-analyzer never resolved `target`");
    assert!(path.ends_with("src/lib.rs"), "resolved into {path:?}");
    // 1-based, converted. `0` here would mean the conversion was skipped; `2` would mean it was
    // applied twice.
    assert_eq!(line, 1, "the declaration is on line 1");
    assert!(column >= 1, "columns are 1-based");
}

#[test]
#[ignore = "spawns the real rust-analyzer"]
fn a_real_rust_analyzer_finds_the_usages_of_a_declaration() {
    // The sibling of the definition test above, and it proves the half a fake cannot: that
    // `textDocument/references` is answered at all (the capability is negotiated, not assumed),
    // that `includeDeclaration: false` really does leave the declaration out, and that a list
    // reply survives the same interleaved `$/progress` traffic the definition reply does.
    //
    // The fixture is deliberately the same shape as the definition test's — `target()` declared
    // on line 1, called on line 4 — so the two read as one story: ask at the *call site* and you
    // get the declaration; ask at the *declaration* and you get the call site.
    let dir = std::env::temp_dir().join(format!("cide-lsp-refs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("mkdir");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn target() -> u32 { 7 }\n\npub fn caller() -> u32 {\n    target()\n}\n",
    )
    .expect("write");

    let handle = LspHandle::start(Server::RustAnalyzer, vec![dir.clone()]).expect("start");
    let (up, seen) = wait_for(&handle, Duration::from_secs(180), ready);
    assert!(
        up,
        "rust-analyzer never reached Ready.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" It gave up: {reason}"),
            None => String::new(),
        }
    );

    // The handshake has completed by now, so the capability must have been read off it. `None`
    // here would mean `Session` stopped storing `result.capabilities` and nothing else noticed.
    assert_eq!(
        handle.supports_references(),
        Some(true),
        "rust-analyzer advertises referencesProvider; a None means the capability was never read"
    );

    let uri = cide_lsp::convert::path_to_uri(&dir.join("src/lib.rs"));
    let (session, _) = cide_lsp::Session::new(std::slice::from_ref(&dir), "rust-analyzer");
    let text = std::fs::read_to_string(dir.join("src/lib.rs")).expect("read");
    if let cide_lsp::Effect::Send(message) = session.did_open(uri.clone(), "rust", 1, text) {
        handle.send(message);
    }

    let requester = handle.requester();
    // 0-based on the wire: line 0 is `pub fn target() -> u32 { 7 }`, character 7 is the `t` of
    // the *declaration*. This is the gesture the whole feature is about.
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": 7 },
        "context": { "includeDeclaration": false },
    });

    // Retried for the reason the definition test states: "indexed enough to publish diagnostics"
    // and "indexed enough to resolve a reference" are different readiness questions.
    let mut answer: Option<Vec<cide_lsp::convert::Loc>> = None;
    for _ in 0..20 {
        match requester.request(
            "textDocument/references",
            params.clone(),
            Duration::from_secs(10),
        ) {
            Ok(value) => {
                if let Some(rows) = cide_lsp::convert::locations(&value)
                    && !rows.is_empty()
                {
                    answer = Some(rows);
                    break;
                }
            }
            Err(error) => panic!("the request failed: {error}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    }

    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);

    let rows = answer.expect("rust-analyzer never listed a usage of `target`");
    // The call site, 1-based. `3` would mean the conversion was skipped.
    assert!(
        rows.iter().any(|row| row.line == 4),
        "the call on line 4 is missing: {rows:?}"
    );
    // And the declaration is *not* in the list. That is what `includeDeclaration: false` buys,
    // and a list whose first row is "where you already are" is the one row that is certainly
    // useless.
    assert!(
        !rows.iter().any(|row| row.line == 1),
        "the declaration came back despite includeDeclaration: false: {rows:?}"
    );
}
