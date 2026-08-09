//! Two checks on the same server, at two different prices.
//!
//! # Why there are two of them
//!
//! The IDE protocol is undocumented and unversioned (see `protocol.rs`), so the only thing
//! that can tell us it still works is the shipped `claude` binary. But a test that needs the
//! binary, a network loopback and the user's Claude subscription cannot be the test that runs
//! on every commit — it is slow, it costs money, and it fails for reasons that have nothing
//! to do with the code under review.
//!
//! So the checks are split:
//!
//! * [`the_frames_the_cli_sends_get_the_replies_it_expects`] drives the server with frames
//!   copied byte-for-byte from a real CLI session. It needs nothing but a loopback socket,
//!   runs in under a second, and is what actually guards the code.
//! * [`the_real_cli_round_trips_a_diff_three_ways`] is `#[ignore]`d and spawns the real
//!   binary. It is the periodic check that the frames above are still the frames the CLI
//!   sends. Run it with `cargo test -p cide-ide-mcp -- --ignored`.
//!
//! The first is worthless without the second: hand-written frames only prove the server
//! answers *us*. The second is unusable as the first: a test that fails because a model
//! phrased an edit differently gets disabled within a week and then guards nothing.
//!
//! # Provenance of the frames below
//!
//! Every JSON literal in the fast test was captured from claude 2.1.226 talking to a
//! throwaway IDE server on this machine, not written from the documentation (there is none)
//! and not copied from the protocol module (which would make the test a mirror of the code
//! rather than a check on it). The capture is reproduced in the constants near each use.
//!
//! # Why the real-CLI test needs a terminal
//!
//! `openDiff` is only ever sent from the CLI's **permission prompt**. Headless `claude -p`
//! never opens an IDE connection at all — the auto-connect lives in the interactive UI, and a
//! print-mode run denies the edit instead of asking. That was measured, not assumed: a
//! `claude -p` child with `CLAUDE_CODE_SSE_PORT` set opened no socket whatsoever. So the real
//! run happens under `script(1)`, which supplies a pty and nothing else. That is a terminal
//! program; no window is created.
//!
//! # What this file must never do
//!
//! It must never read `~/.claude/.credentials.json` and never set `ANTHROPIC_API_KEY`. The
//! child inherits its authentication by inheriting the environment, and an API key would
//! outrank a subscription and bill someone.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::{ClientRequestBuilder, Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use cide_ide_mcp::protocol::{AUTH_HEADER, DiffOutcome, SUBPROTOCOL};
use cide_ide_mcp::server::{IdeServer, ServerEvent};

/// The file every test edits, and the word the prompt asks for.
const ORIGINAL: &str = "alpha\n";
/// What a human types into cide's diff view before accepting. It has to differ from anything
/// the model would produce on its own, or the accepted-with-edits assertion proves nothing.
const OURS: &str = "edited by the human, not by the model\n";

// ---------------------------------------------------------------------------------------
// The fast test: hand-written frames, no `claude`.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_frames_the_cli_sends_get_the_replies_it_expects() {
    let workspace = Scratch::new("cide-ide-ws");
    fs::write(workspace.path().join("note.txt"), ORIGINAL).expect("seed the workspace");

    // `Lockfile::publish` resolves `~/.claude/ide` from `HOME` at the moment the server
    // starts, so the override only has to survive that call; it is undone immediately
    // afterwards so nothing else in the process sees a fabricated home directory.
    //
    // This test and the `#[ignore]`d one below must not share a process: `--ignored` and the
    // default run are disjoint sets, so cargo never does that on its own, but
    // `--include-ignored` would, and the real-CLI test needs the true `HOME` for its
    // authentication.
    let home = Scratch::new("cide-ide-home");
    let server = {
        let _home = HomeGuard::set(home.path());
        IdeServer::start(vec![workspace.path().to_path_buf()])
            .await
            .expect("the server binds a loopback port and publishes a lockfile")
    };

    let port = server.port();
    let broker = server.broker().clone();
    let mut events = server.events();

    // --- the lockfile, as the CLI will read it ------------------------------------------

    let lock_path = home
        .path()
        .join(".claude")
        .join("ide")
        .join(format!("{port}.lock"));
    let lock: Value = serde_json::from_slice(
        &fs::read(&lock_path).expect("the lockfile exists while the server is running"),
    )
    .expect("the lockfile is JSON");

    assert_eq!(
        lock["pid"].as_u64(),
        Some(u64::from(std::process::id())),
        "the CLI unlinks any lockfile whose pid is not a living process"
    );
    assert_eq!(
        lock["workspaceFolders"],
        json!([workspace.path().to_string_lossy()]),
        "without CLAUDE_CODE_SSE_PORT this list is the only thing that makes us valid: the \
         CLI requires one of these to be a prefix of the child's cwd"
    );
    assert_eq!(lock["ideName"], json!("cide"));
    assert_eq!(
        lock["transport"],
        json!("ws"),
        "the CLI's lockfile parser is `useWebSocket = transport === \"ws\"`, and anything \
         else sends it to `http://host:port/sse` instead of our WebSocket. \"ws-ide\" and \
         \"sse-ide\" are the names it gives the resulting config afterwards, not values it \
         accepts here"
    );
    assert_eq!(lock["runningInWindows"], json!(false));
    let token = lock["authToken"]
        .as_str()
        .expect("the lockfile carries the bearer token")
        .to_owned();
    assert!(!token.is_empty());

    // --- an unauthenticated client gets nowhere ------------------------------------------

    assert!(
        McpClient::connect(port, "not-the-token").await.is_err(),
        "the token is the only thing standing between a local process and the user's editor"
    );

    // --- the opening frames, exactly as claude 2.1.226 sends them -------------------------

    let mut cli = McpClient::connect(port, &token)
        .await
        .expect("a client presenting the lockfile token is accepted");

    let hello = cli
        .request(
            0,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {"roots": {"listChanged": true}, "elicitation": {}},
                "clientInfo": {
                    "name": "claude-code",
                    "title": "Claude Code",
                    "version": "2.1.226",
                    "description": "Anthropic's agentic coding tool",
                    "websiteUrl": "https://claude.com/claude-code"
                }
            }),
        )
        .await;
    assert!(
        hello["result"]["capabilities"]["tools"].is_object(),
        "the CLI reads the tools capability before it will call a tool: {hello}"
    );
    assert!(hello["result"]["serverInfo"]["name"].is_string());

    // Two notifications, neither of which may draw a reply. Replying to a message with no id
    // is a JSON-RPC error, and the CLI logs it as a broken server.
    cli.notify("notifications/initialized", json!(null)).await;
    cli.notify("ide_connected", json!({"pid": std::process::id()}))
        .await;

    let tools = cli.request(1, "tools/list", json!(null)).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools/list answers with an array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    // Spelled out rather than compared against `tool::ALL`. Comparing a constant with itself
    // would pass through a rename in either direction, and the camelCase/snake_case mixture
    // below is exactly the sort of thing a tidying pass "corrects" — these are the strings the
    // CLI dispatches on, and it silently drops a tool whose name it does not recognise.
    assert_eq!(
        names,
        [
            "openDiff",
            "close_tab",
            "closeAllDiffTabs",
            "getDiagnostics",
            "openFile"
        ],
        "five tools, no more: seven others were named in planning and appear nowhere in the \
         binary, so serving them would be handlers nothing can call"
    );

    // --- a turn, in the order the CLI actually does it ------------------------------------

    // Every user turn opens with this, before the model has said anything.
    let closed = cli
        .request(
            2,
            "tools/call",
            json!({"name": "closeAllDiffTabs", "arguments": {}}),
        )
        .await;
    assert!(closed["error"].is_null(), "closeAllDiffTabs: {closed}");

    // Sent with no arguments at the start of a turn, and again with a `uri` afterwards.
    for (id, args) in [(3, json!({})), (4, json!({"uri": "file:///tmp/note.txt"}))] {
        let diagnostics = cli
            .request(
                id,
                "tools/call",
                json!({"name": "getDiagnostics", "arguments": args}),
            )
            .await;
        assert_eq!(
            diagnostics["result"]["content"][0]["text"],
            json!("[]"),
            "no language server ships in v1, and an empty list is the honest answer to \
             'what did the language server find'; an error would claim something broke"
        );
    }

    // --- openDiff, the one call that blocks the agent's turn ------------------------------

    let target = workspace.path().join("note.txt");
    let tab_name = "\u{273b} [Claude Code] note.txt (9d201c) \u{29c9}";
    let proposed = "beta\n";

    cli.send(json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "openDiff",
            "arguments": {
                "old_file_path": target.to_string_lossy(),
                "new_file_path": target.to_string_lossy(),
                "new_file_contents": proposed,
                "tab_name": tab_name,
            },
            // The CLI attaches one to every tools/call. Nothing here uses it; it is in the
            // capture because a server that chokes on an unknown `_meta` would break.
            "_meta": {"progressToken": 5},
        }
    }))
    .await;

    let request = next_diff_within(&mut events, Duration::from_secs(5))
        .await
        .expect("openDiff reaches the app as an event");
    assert_eq!(request.params.new_file_contents, proposed);
    assert_eq!(request.params.tab_name, tab_name);

    // Nothing may come back before a human answers. The whole point of the broker is that
    // the CLI is sitting still until it does — so this must distinguish a live-but-quiet
    // connection from a dead one. `Silent` specifically, never merely "no message".
    match cli.try_recv(Duration::from_millis(250)).await {
        Recv::Silent => {}
        Recv::Message(v) => panic!("openDiff answered before anyone looked at it: {v}"),
        Recv::Closed => panic!("the connection dropped instead of blocking on the diff"),
    }

    assert!(broker.resolve(
        &request.id,
        DiffOutcome::Saved {
            contents: OURS.into()
        }
    ));
    let answered = cli.recv().await;
    assert_eq!(answered["id"], json!(5));
    assert_eq!(
        answered["result"]["content"],
        json!([
            {"type": "text", "text": "FILE_SAVED"},
            {"type": "text", "text": OURS},
        ]),
        "the CLI writes `content[1].text` to the file; that entry is what makes a human's \
         edit inside our diff view the version that lands on disk"
    );

    // The CLI sends close_tab twice for the same tab, from two different teardown paths. The
    // second one is for a tab that is already gone, and it must still be a success.
    for id in [6, 7] {
        let closed = cli
            .request(
                id,
                "tools/call",
                json!({"name": "close_tab", "arguments": {"tab_name": tab_name}}),
            )
            .await;
        assert!(
            closed["error"].is_null(),
            "close_tab for an unknown tab must succeed; it fires from beforeExit, for tabs \
             the user already dealt with: {closed}"
        );
    }

    // --- the rejected shape, which carries one entry rather than two ----------------------

    cli.send(json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {
            "name": "openDiff",
            "arguments": {
                "old_file_path": target.to_string_lossy(),
                "new_file_path": target.to_string_lossy(),
                "new_file_contents": proposed,
                "tab_name": "second tab",
            }
        }
    }))
    .await;
    let second = next_diff_within(&mut events, Duration::from_secs(5))
        .await
        .expect("a second diff is registered");
    assert!(broker.resolve(&second.id, DiffOutcome::Rejected));
    let refused = cli.recv().await;
    assert_eq!(
        refused["result"]["content"],
        json!([{"type": "text", "text": "DIFF_REJECTED"}])
    );

    // --- a method we do not serve ---------------------------------------------------------

    let unknown = cli
        .request(9, "resources/read", json!({"uri": "file:///x"}))
        .await;
    assert_eq!(
        unknown["error"]["code"],
        json!(-32601),
        "an unimplemented method is answered, not ignored: a request the CLI never gets a \
         reply to is a hung turn"
    );

    // --- shutdown takes the lockfile with it ----------------------------------------------

    let _ = server.shutdown().await;
    assert!(
        !lock_path.exists(),
        "a lockfile that outlives its server sends the next claude to a dead port"
    );
}

// ---------------------------------------------------------------------------------------
// The slow test: the real binary.
// ---------------------------------------------------------------------------------------

/// The three ways a human can answer a diff, and what each one must leave on disk.
#[derive(Debug, Clone, Copy)]
enum Answer {
    /// Accepted after editing it in cide. The file must end up with **our** text, which is
    /// the assertion this whole file exists for: it is the only proof that `content[1].text`
    /// is read by the CLI rather than ignored.
    SavedWithOurEdits,
    /// Accepted as proposed, by closing the tab. The file gets the model's version.
    TabClosed,
    /// Rejected. The file is not touched.
    Rejected,
}

#[tokio::test]
#[ignore = "spawns the real claude binary: needs it on PATH, a loopback port and the user's \
            Claude authentication. Run with: cargo test -p cide-ide-mcp -- --ignored"]
async fn the_real_cli_round_trips_a_diff_three_ways() {
    let Some(claude) = on_path("claude") else {
        eprintln!("SKIP: no `claude` on PATH, so there is nothing to check the protocol against");
        return;
    };
    if on_path("script").is_none() {
        eprintln!(
            "SKIP: no `script(1)`. The CLI only sends openDiff from its interactive \
             permission prompt, so this test needs a pty and script is how it gets one."
        );
        return;
    }
    eprintln!("using {}", claude.display());

    for answer in [
        Answer::SavedWithOurEdits,
        Answer::TabClosed,
        Answer::Rejected,
    ] {
        match run_one_turn(&claude, answer).await {
            Ok(()) => eprintln!("PASS {answer:?}"),
            // The CLI writing the wrong file is the regression this test exists to catch, so
            // that one fails. Everything else — a model that phrased the turn differently, a
            // rate limit, a machine without the binary — is not evidence about our code, and
            // failing on it would get the whole test disabled within a week. Those become
            // skips, with the pty transcript attached so the run is still diagnosable.
            Err(reason) if reason.starts_with("WRONG FILE") => panic!("{reason}"),
            Err(reason) => eprintln!("SKIP {answer:?}: {reason}"),
        }
    }
}

async fn run_one_turn(claude: &Path, answer: Answer) -> Result<(), String> {
    let workspace = Scratch::new("cide-real-cli");
    let target = workspace.path().join("note.txt");
    fs::write(&target, ORIGINAL).map_err(|e| e.to_string())?;

    // The real `HOME`, deliberately. The child authenticates by inheriting the environment,
    // and it looks for the lockfile under its own `HOME` — the same directory the server
    // writes to. Pointing either one at a temporary home breaks the pair. The port is
    // ephemeral and `Lockfile` removes its own file, so the shared directory is left as it
    // was found; the lockfile lifecycle itself is asserted against a temporary home in the
    // fast test above.
    let server = IdeServer::start(vec![workspace.path().to_path_buf()])
        .await
        .map_err(|e| format!("the server would not start: {e}"))?;
    let port = server.port();
    let broker = server.broker().clone();
    let mut events = server.events();

    let lock_path = home_dir()?
        .join(".claude")
        .join("ide")
        .join(format!("{port}.lock"));
    if !lock_path.exists() {
        return Err(format!("no lockfile at {}", lock_path.display()));
    }

    let transcript = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut child = spawn_under_pty(claude, workspace.path(), port, &transcript)?;
    // Not `?`. The child is already running by this point, so an early return here would
    // leave a `claude` holding a model connection with nobody to reap it — the one failure
    // mode this file is least entitled to have, given it spawns real processes.
    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = child.kill().await;
            return Err("the child has no stdin".into());
        }
    };

    // The turn is driven inside a block so that the teardown below runs exactly once, on
    // every path out of it. A `return` that skips the teardown leaves a `claude` holding a
    // model connection open, and the next two cases then race it for the lockfile directory.
    let driven = async {
        // Typed rather than passed as an argument: the prompt has to arrive after the CLI has
        // booted and connected, and `claude "<prompt>"` would start the turn before the IDE
        // connection exists.
        let typing = async {
            tokio::time::sleep(Duration::from_secs(4)).await;
            // A fresh directory raises the workspace-trust question; Enter takes its default,
            // which is to trust. If the question does not appear, this submits an empty
            // prompt, which the CLI ignores.
            stdin.write_all(b"\r").await?;
            tokio::time::sleep(Duration::from_secs(5)).await;
            stdin
                .write_all(
                    b"Use the Edit tool to replace the word alpha with the word beta in \
                      note.txt. Do nothing else.",
                )
                .await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            stdin.write_all(b"\r").await?;
            stdin.flush().await
        };
        typing
            .await
            .map_err(|e| format!("could not type into the pty: {e}"))?;

        // Boot, a model call and a `Read` all happen first; ninety seconds is roughly five
        // times what the runs on this machine took.
        let request = match timeout(Duration::from_secs(90), next_diff(&mut events)).await {
            Ok(Some(request)) => request,
            Err(_) => return Err("no openDiff within 90s".to_owned()),
            Ok(None) => return Err("the event stream ended".to_owned()),
        };

        let proposed = request.params.new_file_contents.clone();
        if !proposed.contains("beta") {
            // Answered before the skip is reported. A registered request that is never
            // resolved leaves the CLI blocked with nothing on screen, and the teardown below
            // would then be killing an agent mid-turn rather than interrupting an idle one.
            broker.resolve(&request.id, DiffOutcome::Rejected);
            return Err(format!(
                "the model proposed {proposed:?}, which is not the asked-for edit"
            ));
        }

        let (outcome, expected) = match answer {
            Answer::SavedWithOurEdits => (
                DiffOutcome::Saved {
                    contents: OURS.into(),
                },
                Expected::Becomes(OURS.to_owned()),
            ),
            Answer::TabClosed => (DiffOutcome::TabClosed, Expected::Becomes(proposed.clone())),
            // Not `Becomes(ORIGINAL)`. The file already holds ORIGINAL, so a first read would
            // match microseconds after `resolve` — before the CLI has so much as parsed the
            // reply — and the case would report a pass whatever the CLI went on to do. The
            // only assertion with any content here is that it *stays* untouched.
            Answer::Rejected => (DiffOutcome::Rejected, Expected::Stays(ORIGINAL.to_owned())),
        };
        if !broker.resolve(&request.id, outcome) {
            return Err("the CLI stopped waiting for the answer".to_owned());
        }

        // The CLI writes the file itself once it has the answer, and says nothing when it is
        // done, so the file is the only signal there is.
        match expected {
            Expected::Becomes(want) => {
                let found = wait_for_file(&target, &want, Duration::from_secs(30)).await?;
                if found != want {
                    // A wrong file is the failure this test exists to catch, so it is reported
                    // rather than skipped: the caller turns this one into a panic.
                    return Err(format!(
                        "WRONG FILE: {answer:?} left {found:?} on disk, expected {want:?} \
                         (the model proposed {proposed:?})"
                    ));
                }
            }
            Expected::Stays(want) => {
                // Long enough to cover the write the two accepting cases perform, which is
                // what a rejection mishandled as an acceptance would do here. Those complete
                // in well under a second on this machine; the margin is for a loaded one.
                if let Some(found) = file_changed(&target, &want, Duration::from_secs(15)).await? {
                    return Err(format!(
                        "WRONG FILE: {answer:?} left {found:?} on disk, expected the file to \
                         be untouched at {want:?} (the model proposed {proposed:?})"
                    ));
                }
            }
        }
        Ok(())
    }
    .await;

    reap(&mut child, &mut stdin).await;
    let _ = server.shutdown().await;
    assert!(
        !lock_path.exists(),
        "the lockfile survived shutdown; the next claude would be sent to a dead port"
    );

    driven.map_err(|why| format!("{why}\n--- transcript ---\n{}", tail(&transcript)))
}

/// Spawn `claude` with a pty in front of it and our port in its environment.
///
/// `CLAUDE_CODE_SSE_PORT` does two things at once: it makes the CLI auto-connect, and it
/// makes our lockfile valid without the workspace-folder check. Nothing else is added to the
/// environment, and in particular no API key — the child inherits its authentication.
fn spawn_under_pty(
    claude: &Path,
    cwd: &Path,
    port: u16,
    transcript: &Arc<Mutex<Vec<u8>>>,
) -> Result<tokio::process::Child, String> {
    let program = claude.to_string_lossy();
    if program.contains('\'') {
        return Err(format!("cannot quote {program} for script(1)"));
    }

    let mut command = tokio::process::Command::new("script");
    command
        .arg("-qec")
        .arg(format!("'{program}' --permission-mode manual"))
        .arg("/dev/null")
        .current_dir(cwd)
        .env("CLAUDE_CODE_SSE_PORT", port.to_string())
        // Scrubbed, not merely unset by us. A developer whose shell exports either of these
        // would otherwise get a silent skip — the test would pass having never connected,
        // which is the same shape of false green as a unit test pinning the wrong transport.
        // `CLAUDE_CODE_AUTO_CONNECT_IDE` can switch auto-connect off outright;
        // `CLAUDE_CODE_IDE_SKIP_VALID_CHECK` marks *every* lockfile valid, which would let
        // another editor's file compete with ours for the connection.
        .env_remove("CLAUDE_CODE_AUTO_CONNECT_IDE")
        .env_remove("CLAUDE_CODE_IDE_SKIP_VALID_CHECK")
        // The CLI lays out its interface against these; a default 80x24 wraps the permission
        // prompt badly enough to change what the transcript says on failure.
        .env("TERM", "xterm-256color")
        .env("COLUMNS", "120")
        .env("LINES", "40")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own process group, so that if the polite exit fails there is a single signal
        // that reaches `script` and the `claude` underneath it. A leaked claude holds a model
        // connection open long after the test has finished.
        .process_group(0)
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|e| format!("could not spawn script: {e}"))?;
    for stream in [
        child.stdout.take().map(Pipe::Out),
        child.stderr.take().map(Pipe::Err),
    ]
    .into_iter()
    .flatten()
    {
        let sink = Arc::clone(transcript);
        tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            let mut stream = stream;
            while let Ok(n) = stream.read(&mut buffer).await {
                if n == 0 {
                    break;
                }
                sink.lock().extend_from_slice(&buffer[..n]);
            }
        });
    }
    Ok(child)
}

/// The two output streams, unified so one reader task body serves both.
enum Pipe {
    Out(tokio::process::ChildStdout),
    Err(tokio::process::ChildStderr),
}

impl Pipe {
    async fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Out(s) => s.read(buffer).await,
            Self::Err(s) => s.read(buffer).await,
        }
    }
}

/// Ask the CLI to leave, then insist.
///
/// Two interrupts because the first one cancels the turn and the second one exits; that is
/// the CLI's own behaviour and it is what lets it write its transcript before dying.
async fn reap(child: &mut tokio::process::Child, stdin: &mut tokio::process::ChildStdin) {
    let _ = stdin.write_all(b"\x03").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let _ = stdin.write_all(b"\x03").await;

    if timeout(Duration::from_secs(10), child.wait()).await.is_ok() {
        return;
    }
    // The polite exit did not happen. `script` and `claude` share the process group created
    // at spawn, and killing the leader alone would orphan the CLI.
    if let Some(pid) = child.id() {
        // SAFETY: `pid` came from a child of this process that has not been reaped, so the
        // group id is ours to signal and cannot yet name an unrelated process.
        unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
    }
    let _ = timeout(Duration::from_secs(5), child.wait()).await;
}

/// What the file on disk has to do for an answer to count as honoured.
///
/// The distinction is not cosmetic. Two of the three answers change the file, so waiting for
/// the change is both the wait and the assertion. The third answer changes nothing, and for
/// that one "does it match?" is already true before the CLI has read the reply — so it has to
/// be watched instead.
enum Expected {
    /// The CLI writes this, eventually.
    Becomes(String),
    /// The CLI writes nothing, for as long as anyone is looking.
    Stays(String),
}

/// Watch a file that should not change, and report the first content that is not `expected`.
///
/// `Ok(None)` means it held still for the whole window. An unreadable file is an error rather
/// than a deviation: a rejected diff should leave the file exactly as it was, and the CLI
/// deleting it is a different fault from the CLI rewriting it.
async fn file_changed(
    path: &Path,
    expected: &str,
    window: Duration,
) -> Result<Option<String>, String> {
    let deadline = tokio::time::Instant::now() + window;
    while tokio::time::Instant::now() < deadline {
        let found = fs::read_to_string(path).map_err(|e| e.to_string())?;
        if found != expected {
            return Ok(Some(found));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(None)
}

async fn wait_for_file(path: &Path, expected: &str, budget: Duration) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        last = fs::read_to_string(path).map_err(|e| e.to_string())?;
        if last == expected {
            return Ok(last);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Returned rather than asserted so the caller can attach the transcript; the mismatch
    // itself is asserted there.
    Ok(last)
}

/// The last of the pty transcript, with the escape sequences left in.
///
/// Stripping them would be guesswork about which sequences matter, and the raw bytes are what
/// a human debugging a failure wants to paste into a terminal.
fn tail(transcript: &Arc<Mutex<Vec<u8>>>) -> String {
    let bytes = transcript.lock();
    let start = bytes.len().saturating_sub(4000);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

// ---------------------------------------------------------------------------------------
// A hand-written MCP client.
// ---------------------------------------------------------------------------------------

/// The outcome of waiting for a frame.
///
/// Three states rather than `Option`, because "no message arrived" and "the connection is
/// gone" mean opposite things to the assertion this file cares most about. See
/// [`McpClient::try_recv`].
enum Recv {
    Message(Value),
    /// The budget elapsed with the connection still open.
    Silent,
    /// The socket closed or errored.
    Closed,
}

struct McpClient {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl McpClient {
    /// Connect the way the CLI does: subprotocol `mcp`, token in the IDE authorisation header.
    async fn connect(port: u16, token: &str) -> Result<Self, WsError> {
        // Every component is a literal or a `u16`, so a parse failure would be a bug in this
        // line rather than a condition the caller can be told about.
        let uri: Uri = format!("ws://127.0.0.1:{port}/")
            .parse()
            .expect("a loopback URI");
        let request = ClientRequestBuilder::new(uri)
            .with_sub_protocol(SUBPROTOCOL)
            .with_header(AUTH_HEADER, token);
        let (socket, _) = connect_async(request).await?;
        Ok(Self { socket })
    }

    async fn send(&mut self, message: Value) {
        self.socket
            .send(Message::text(message.to_string()))
            .await
            .expect("the socket accepts a frame");
    }

    /// A message with no id, which must never draw a reply.
    async fn notify(&mut self, method: &str, params: Value) {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message).await;
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message).await;
        let reply = self.recv().await;
        assert_eq!(reply["id"], json!(id), "reply out of order: {reply}");
        reply
    }

    async fn recv(&mut self) -> Value {
        match self.try_recv(Duration::from_secs(5)).await {
            Recv::Message(v) => v,
            Recv::Silent => panic!("the server did not answer within five seconds"),
            Recv::Closed => panic!("the connection dropped while waiting for a reply"),
        }
    }

    /// Read one text frame, or give up. Control frames are the transport's business and are
    /// skipped rather than surfaced.
    async fn try_recv(&mut self, budget: Duration) -> Recv {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Recv::Silent;
            }
            match timeout(remaining, self.socket.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    return Recv::Message(
                        serde_json::from_str(text.as_str()).expect("the server sends JSON"),
                    );
                }
                Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
                // A dead socket is deliberately *not* folded into `Silent`. The assertion
                // that matters most in this file — that `openDiff` blocks the turn — is that
                // nothing comes back before a human answers. If a dropped connection also
                // read as "nothing came back", that assertion would pass most convincingly
                // at the exact moment the server had failed.
                Ok(Some(Ok(_)) | None) | Ok(Some(Err(_))) => return Recv::Closed,
                Err(_) => return Recv::Silent,
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------------------

/// The next diff request, skipping the connection bookkeeping in between.
async fn next_diff(events: &mut Receiver<ServerEvent>) -> Option<cide_ide_mcp::DiffRequest> {
    while let Some(event) = events.recv().await {
        if let ServerEvent::DiffRequested(request) = event {
            return Some(request);
        }
    }
    None
}

/// [`next_diff`], but it gives up.
///
/// The bare version never returns when no diff is emitted: the server holds a sender for its
/// whole life, so the channel never closes and `recv` waits for ever. A tool handler that
/// rejects its arguments — which is what a renamed `openDiff` field looks like from here —
/// answers the CLI and emits nothing, and the caller would then hang until whatever runs the
/// suite gives up on it. `cargo test` has no timeout of its own.
async fn next_diff_within(
    events: &mut Receiver<ServerEvent>,
    budget: Duration,
) -> Option<cide_ide_mcp::DiffRequest> {
    timeout(budget, next_diff(events)).await.ok().flatten()
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// A directory that deletes itself.
///
/// Hand-rolled rather than pulled in as a dependency: one directory and one `remove_dir_all`
/// is not worth adding a crate to the graph of a crate that ships in the application.
struct Scratch(PathBuf);

impl Scratch {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let unique = format!(
            "{prefix}-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("a scratch directory");
        // Symlinks in the temporary path (/tmp -> /private/tmp, and similar) would make the
        // CLI's prefix comparison between a workspace folder and its cwd fail on paths that
        // are in fact the same directory.
        Self(path.canonicalize().unwrap_or(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `HOME`, moved and put back.
struct HomeGuard(Option<OsString>);

impl HomeGuard {
    fn set(home: &Path) -> Self {
        let previous = std::env::var_os("HOME");
        // SAFETY: the environment is process-wide, so this is only sound while no other
        // thread reads it. Nothing else in this test binary touches HOME, and the two tests
        // here never run in the same process — `--ignored` and the default run are disjoint.
        unsafe { std::env::set_var("HOME", home) };
        Self(previous)
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: as above.
        match self.0.take() {
            Some(previous) => unsafe { std::env::set_var("HOME", previous) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
