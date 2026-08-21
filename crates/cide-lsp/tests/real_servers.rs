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

    let handle = LspHandle::start(Server::RUST_ANALYZER, vec![root]).expect("start rust-analyzer");

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

    let handle = LspHandle::start(Server::RUST_ANALYZER, vec![dir.clone()]).expect("start");
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

    let handle = LspHandle::start(Server::GOPLS, vec![dir.clone()]).expect("start gopls");
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
    let handle = LspHandle::start(Server::RUST_ANALYZER, vec![root]).expect("start");
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

    let handle = LspHandle::start(Server::RUST_ANALYZER, vec![dir.clone()]).expect("start");
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

    let handle = LspHandle::start(Server::RUST_ANALYZER, vec![dir.clone()]).expect("start");
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
    let (session, _) = cide_lsp::Session::new(std::slice::from_ref(&dir), Server::RUST_ANALYZER);
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

    let handle = LspHandle::start(Server::RUST_ANALYZER, vec![dir.clone()]).expect("start");
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
    let (session, _) = cide_lsp::Session::new(std::slice::from_ref(&dir), Server::RUST_ANALYZER);
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

/* -------------------------------------------------------------------------- M18 --------- */

/// A Cargo project with one deliberately broken file, at a unique temp path.
///
/// Factored out because the three M18 tests below all need the same fixture and the same "wait for
/// flycheck to report the error" preamble, and three copies of it is three chances for one of them
/// to drift into asserting something slightly different from what its neighbour asserts.
fn broken_rust_crate(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cide-lsp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("mkdir");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    std::fs::write(dir.join("src/lib.rs"), "pub fn f() -> u32 { \"nope\" }\n").expect("write");
    dir
}

/// Does any event in the tail carry a `mismatched types`?
fn saw_mismatch(events: &[LspEvent]) -> bool {
    events.iter().any(|e| {
        matches!(e, LspEvent::Published { items, .. } if items.iter().any(|d| d.message.contains("mismatched types")))
    })
}

#[test]
#[ignore = "spawns the real rust-analyzer"]
fn a_flycheck_kick_refreshes_diagnostics_an_on_disk_edit_alone_would_not() {
    /*
     * The exact inverse of `an_on_disk_edit_alone_never_refreshes_diagnostics`, and the proof
     * that M18's fix for the Rust half of the report is real rather than plausible.
     *
     * That test repairs the file on disk, sends nothing, waits two minutes and asserts the
     * diagnostic is **still there**. It stays exactly as it is: it is the guard on the assumption,
     * and it stays true because it still sends nothing.
     *
     * This one does the same thing and then sends `rust-analyzer/runFlycheck` — the notification
     * `ProjectDiagnostics::files_changed` parks on the `Kick` coalescer for the pump to send once
     * a burst of watcher events settles. If rust-analyzer ever stops implementing that extension,
     * or changes its params, this test fails and the panel's "Re-run analysis" button silently
     * becomes decoration. There is no other gate on it: a notification produces no reply, so a
     * server that has never heard of the method drops it without a word — which is what makes
     * sending it safe and also what makes it invisible when it breaks.
     */
    let dir = broken_rust_crate("flycheck");
    let handle = LspHandle::start(Server::RUST_ANALYZER, vec![dir.clone()]).expect("start");
    let (saw_error, seen) = wait_for(&handle, Duration::from_secs(180), saw_mismatch);
    assert!(
        saw_error,
        "flycheck never reported the error on load, so this test cannot say anything about \
         clearing it.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" rust-analyzer gave up: {reason}"),
            None => format!(" Seen: {seen:#?}"),
        }
    );

    std::fs::write(dir.join("src/lib.rs"), "pub fn f() -> u32 { 7 }\n").expect("write");
    // Drain whatever is queued so the assertion below reads only what arrives *after* the kick.
    // Without this the pre-repair publish is still sitting in the channel and would be mistaken
    // for the answer.
    let _ = handle.drain();

    let (session, _) = cide_lsp::Session::new(std::slice::from_ref(&dir), Server::RUST_ANALYZER);
    let cide_lsp::Effect::Send(kick) = session.run_flycheck() else {
        panic!("run_flycheck must be a Send effect")
    };
    handle.send(kick);

    // A `cargo check` of a one-file crate, plus rust-analyzer noticing the VFS change. Generous,
    // because a cold cargo on a loaded machine is not fast and a flake here would be read as "the
    // kick does not work".
    let (cleared, after) = wait_for(&handle, Duration::from_secs(120), |events| {
        events
            .iter()
            .any(|e| matches!(e, LspEvent::Published { items, .. } if !items.iter().any(|d| d.message.contains("mismatched types"))))
    });

    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        cleared,
        "`rust-analyzer/runFlycheck` did not re-run the check after an on-disk repair. The \
         Problems panel's Re-run button and every agent edit depend on this notification; if the \
         extension has been renamed or its params have changed, a notification fails silently and \
         nothing else in this repository would notice. Seen after the kick: {after:#?}"
    );
}

#[test]
#[ignore = "spawns the real rust-analyzer"]
fn rust_analyzer_answers_go_to_implementation() {
    /*
     * The Rust half of the ITEM 3 guarantee — and the reason Ctrl+B was **not** changed to try
     * implementation first.
     *
     * rust-analyzer answers `textDocument/implementation` on a trait's method with the concrete
     * `impl` blocks, which is the feature. It also answers on an ordinary struct or trait *name*
     * with its impls — so an implementation-first Ctrl+click would stop opening declarations for
     * every plain type in a Rust workspace. This test pins the first behaviour; the separate
     * command id is what protects the second.
     */
    let dir = std::env::temp_dir().join(format!("cide-lsp-impl-rs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("mkdir");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    // `trait Greet { fn hello(&self); }` on line 1, and the impl's `hello` on line 6.
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub trait Greet {\n    fn hello(&self);\n}\n\npub struct En;\n\nimpl Greet for En {\n    fn hello(&self) {}\n}\n",
    )
    .expect("write");

    let handle = LspHandle::start(Server::RUST_ANALYZER, vec![dir.clone()]).expect("start");
    let (_, seen) = wait_for(&handle, Duration::from_secs(180), ready);
    let uri = cide_lsp::convert::path_to_uri(&dir.join("src/lib.rs"));
    let (session, _) = cide_lsp::Session::new(std::slice::from_ref(&dir), Server::RUST_ANALYZER);
    let text = std::fs::read_to_string(dir.join("src/lib.rs")).expect("read");
    if let cide_lsp::Effect::Send(message) = session.did_open(uri.clone(), "rust", 1, text) {
        handle.send(message);
    }

    // The handshake is done, so the capability has been read. `None` would mean `Caps::store_from`
    // stopped being called and the refusal branch in `ProjectDiagnostics::implementations` would
    // be reading a permanently-unknown atomic.
    assert_eq!(
        handle.supports_implementation(),
        Some(true),
        "rust-analyzer advertises implementationProvider; a None means the capability was never \
         read off the handshake.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" It gave up: {reason}"),
            None => String::new(),
        }
    );

    // 0-based: line 1 is `    fn hello(&self);`, character 7 is the `h` of the trait's method.
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 1, "character": 7 },
    });
    let requester = handle.requester();
    let mut answer: Option<Vec<cide_lsp::convert::Loc>> = None;
    for _ in 0..20 {
        match requester.request(
            "textDocument/implementation",
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

    let rows = answer.expect("rust-analyzer never answered with an implementation of `hello`");
    // 1-based. Line 8 is the impl's `fn hello(&self) {}`. Line 2 would be the trait's own
    // declaration, i.e. the answer `textDocument/definition` already gives.
    assert!(
        rows.iter().any(|row| row.line == 8),
        "the concrete `hello` on line 8 is missing — this is the whole feature: {rows:?}"
    );
}

#[test]
#[ignore = "spawns the real gopls"]
fn gopls_goes_to_the_implementation_rather_than_the_interface() {
    /*
     * The report, reproduced and then answered.
     *
     * `Speak(s Speaker)` calls `s.Say()` through an interface. `textDocument/definition` on that
     * call resolves to the *interface's* `Say` — which is gopls being correct, and is exactly what
     * the user saw and did not want. `textDocument/implementation` on the same position resolves
     * to `En.Say`, the concrete method.
     *
     * Both halves are asserted, in one test, deliberately: the point is not that implementation
     * works, it is that the two requests give **different** answers at the same caret. A test that
     * only asked the second could pass on a build where definition had quietly started answering
     * the same thing, and the separate command would then be pointless.
     */
    let dir = std::env::temp_dir().join(format!("cide-lsp-impl-go-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("go.mod"), "module probe\n\ngo 1.21\n").expect("write");
    /*
     * Line-numbered, 1-based, because both assertions below are about which line came back:
     *   1 package probe
     *   2
     *   3 type Speaker interface {
     *   4     Say() string
     *   5 }
     *   6
     *   7 type En struct{}
     *   8
     *   9 func (En) Say() string { return "hi" }
     *  10
     *  11 func Speak(s Speaker) string { return s.Say() }
     */
    std::fs::write(
        dir.join("probe.go"),
        "package probe\n\ntype Speaker interface {\n\tSay() string\n}\n\ntype En struct{}\n\nfunc (En) Say() string { return \"hi\" }\n\nfunc Speak(s Speaker) string { return s.Say() }\n",
    )
    .expect("write");

    let handle = LspHandle::start(Server::GOPLS, vec![dir.clone()]).expect("start");
    let (_, seen) = wait_for(&handle, Duration::from_secs(120), ready);
    let uri = cide_lsp::convert::path_to_uri(&dir.join("probe.go"));
    let (session, _) = cide_lsp::Session::new(std::slice::from_ref(&dir), Server::GOPLS);
    let text = std::fs::read_to_string(dir.join("probe.go")).expect("read");
    if let cide_lsp::Effect::Send(message) = session.did_open(uri.clone(), "go", 1, text) {
        handle.send(message);
    }

    assert_eq!(
        handle.supports_implementation(),
        Some(true),
        "gopls advertises implementationProvider.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" It gave up: {reason}"),
            None => String::new(),
        }
    );

    /*
     * 0-based: line 10 is `func Speak(s Speaker) string { return s.Say() }`, and character 40 is
     * the `S` of `Say`.
     *
     * Counted rather than guessed, and the difference is a real failure this test already caught
     * once: character 38 is the receiver `s`, and gopls answers
     * `textDocument/implementation` there with a JSON-RPC **error** — *"s is a var, not a type"*
     * — rather than with `null`. That behaviour is why `ProjectDiagnostics::implementations`
     * folds `RequestError::Failed` into `NotFound`; see the comment there.
     */
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 10, "character": 40 },
    });
    let requester = handle.requester();

    let ask = |method: &str| -> Vec<cide_lsp::convert::Loc> {
        for _ in 0..20 {
            match requester.request(method, params.clone(), Duration::from_secs(10)) {
                Ok(value) => {
                    if let Some(rows) = cide_lsp::convert::locations(&value)
                        && !rows.is_empty()
                    {
                        return rows;
                    }
                }
                Err(error) => panic!("{method} failed: {error}"),
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        Vec::new()
    };

    let definition = ask("textDocument/definition");
    let implementation = ask("textDocument/implementation");

    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);

    // The report, reproduced: definition lands on the interface's method, line 4.
    assert!(
        definition.iter().any(|row| row.line == 4),
        "gopls no longer resolves the interface call to the interface's own method — the premise \
         of this whole change. Got: {definition:?}"
    );
    // And the answer: implementation lands on the concrete one, line 9.
    assert!(
        implementation.iter().any(|row| row.line == 9),
        "`textDocument/implementation` did not reach `En.Say` on line 9: {implementation:?}"
    );
    assert!(
        !implementation.iter().any(|row| row.line == 4),
        "implementation came back with the interface declaration, which is the answer the user \
         already had and did not want: {implementation:?}"
    );
}

#[test]
#[ignore = "spawns the real gopls"]
fn gopls_re_diagnoses_a_file_it_was_told_changed_on_disk() {
    /*
     * The Go half of the stale-diagnostics report.
     *
     * gopls has **no file watcher of its own**: it registers watch patterns through
     * `client/registerCapability` and then waits for `workspace/didChangeWatchedFiles`. cide
     * answered that registration with `null` and sent the notification nowhere, so a file an agent
     * rewrote — one the user never opened, so no `didChange` either — kept its diagnostics with
     * their original line numbers for the rest of the session.
     *
     * The file here is deliberately **never opened**: no `didOpen`, no `didChange`. If this test
     * passes only because of an editor notification, it is not testing the path that was broken.
     */
    let dir = std::env::temp_dir().join(format!("cide-lsp-watch-go-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("go.mod"), "module probe\n\ngo 1.21\n").expect("write");
    // `undefinedName` is an unresolved identifier, which gopls reports without needing to build.
    std::fs::write(
        dir.join("probe.go"),
        "package probe\n\nfunc F() int { return undefinedName }\n",
    )
    .expect("write");

    let handle = LspHandle::start(Server::GOPLS, vec![dir.clone()]).expect("start");
    let broken = |events: &[LspEvent]| {
        events.iter().any(|e| {
            matches!(e, LspEvent::Published { items, .. } if items.iter().any(|d| d.message.contains("undefinedName")))
        })
    };
    let (saw_error, seen) = wait_for(&handle, Duration::from_secs(120), broken);
    assert!(
        saw_error,
        "gopls never reported the undefined name, so this test cannot say anything about \
         clearing it.{}",
        match gave_up(&seen) {
            Some(reason) => format!(" It gave up: {reason}"),
            None => format!(" Seen: {seen:#?}"),
        }
    );

    std::fs::write(
        dir.join("probe.go"),
        "package probe\n\nfunc F() int { return 7 }\n",
    )
    .expect("write");
    let _ = handle.drain();

    let (session, _) = cide_lsp::Session::new(std::slice::from_ref(&dir), Server::GOPLS);
    // `2` is LSP's `FileChangeType::Changed`. This is byte-for-byte what
    // `ProjectDiagnostics::files_changed` sends from the watcher thread.
    let cide_lsp::Effect::Send(notice) = session.did_change_watched_files(vec![(
        cide_lsp::convert::path_to_uri(&dir.join("probe.go")),
        2,
    )]) else {
        panic!("did_change_watched_files must be a Send effect")
    };
    handle.send(notice);

    let (cleared, after) = wait_for(&handle, Duration::from_secs(60), |events| {
        events
            .iter()
            .any(|e| matches!(e, LspEvent::Published { items, .. } if !items.iter().any(|d| d.message.contains("undefinedName"))))
    });

    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        cleared,
        "gopls did not re-diagnose a file it was told had changed on disk. Every agent edit to a \
         Go file the user has not opened depends on this notification. Seen after it: {after:#?}"
    );
}
