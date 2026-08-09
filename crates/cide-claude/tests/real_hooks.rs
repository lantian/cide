//! Does the settings payload we generate actually make the real CLI call our hooks?
//!
//! Every other test in this crate checks that we build the JSON we meant to build. None of
//! them can tell whether the CLI agrees, and that is the part with no specification: the hook
//! config shape is undocumented, unversioned, and was read out of a binary that updates
//! itself. A rename of one key would leave every unit test green and every hook silent.
//!
//! `#[ignore]`d because it spawns the real `claude`, which needs the binary on PATH, network
//! access and the user's authentication.
//!
//! Run with: `cargo test -p cide-claude -- --ignored`

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cide_claude::{HookEvent, StatusLine, inline_settings};

fn on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

/// The `cide-hook` built alongside this test.
fn hook_binary() -> Option<PathBuf> {
    // `current_exe` is the test binary in target/<profile>/deps/, so the sibling binaries are
    // two levels up.
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let hook = dir.join("cide-hook");
    hook.is_file().then_some(hook)
}

#[test]
#[ignore = "spawns the real claude binary: needs it on PATH and the user's authentication. \
            Run with: cargo test -p cide-claude -- --ignored"]
fn the_real_cli_calls_the_hooks_our_settings_register() {
    let Some(claude) = on_path("claude") else {
        eprintln!("SKIP: no `claude` on PATH");
        return;
    };
    let Some(hook) = hook_binary() else {
        eprintln!("SKIP: cide-hook is not built; run `cargo build -p cide-hook` first");
        return;
    };

    let dir = std::env::temp_dir().join(format!("cide-m7-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let sock = dir.join("hooks.sock");
    let _ = std::fs::remove_file(&sock);

    let listener = UnixListener::bind(&sock).expect("bind the hook socket");
    let (tx, rx) = mpsc::channel::<(String, String)>();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    let event = v["event"].as_str().unwrap_or_default().to_owned();
                    let session = v["payload"]["session_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    let _ = tx.send((event, session));
                }
            });
        }
    });

    // The id we will demand the CLI reports back. This is the whole point of passing
    // `--session-id`: without it the CLI picks its own uuid, every hook frame names a session
    // the app never minted, and the state machine silently never runs.
    let session_id = uuid::Uuid::new_v4().to_string();
    let settings = inline_settings(&hook.to_string_lossy(), &StatusLine::Ours);

    let output = std::process::Command::new(&claude)
        .arg("-p")
        .arg("say exactly: done")
        .arg("--session-id")
        .arg(&session_id)
        .arg("--settings")
        .arg(serde_json::to_string(&settings).expect("settings serialise"))
        .current_dir(&dir)
        .env("CIDE_HOOK_SOCK", &sock)
        .env("TERM", "xterm-256color")
        // A developer whose shell disables IDE auto-connect would otherwise get a run that
        // proves nothing while reporting success.
        .env_remove("CLAUDE_CODE_AUTO_CONNECT_IDE")
        .output();

    let Ok(output) = output else {
        eprintln!("SKIP: could not spawn claude");
        return;
    };
    if !output.status.success() {
        // A rate limit or an auth prompt is not evidence about our settings payload, and
        // failing on it would get this test disabled within a week.
        eprintln!(
            "SKIP: claude exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(300)
                .collect::<String>()
        );
        return;
    }

    let mut seen: Vec<(String, String)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(frame) => seen.push(frame),
            Err(_) => break,
        }
    }

    let _ = std::fs::remove_file(&sock);
    let mut events: Vec<&str> = seen.iter().map(|(e, _)| e.as_str()).collect();
    events.sort();
    events.dedup();
    eprintln!("hook events observed: {events:?}");

    assert!(
        !seen.is_empty(),
        "the real CLI called no hooks at all — the settings payload registered nothing, \
         which every unit test in this crate would still report as correct"
    );

    // The two that must fire for any turn whatsoever. Tool hooks depend on what the model
    // chose to do, so asserting on them would make this test depend on a model's behaviour.
    for required in [HookEvent::SessionStart, HookEvent::Stop] {
        assert!(
            events.contains(&required.as_str()),
            "{required} never fired; observed {events:?}"
        );
    }

    let reported: Vec<&String> = seen
        .iter()
        .map(|(_, s)| s)
        .filter(|s| !s.is_empty())
        .collect();
    assert!(
        reported.iter().all(|s| **s == session_id),
        "the CLI reported a session id other than the one we passed to --session-id.\n\
         passed:   {session_id}\n\
         reported: {reported:?}\n\
         Every hook frame would be dropped as belonging to a session this app does not own."
    );
}

#[test]
#[ignore = "spawns the real claude binary"]
fn a_chained_statusline_is_not_lost() {
    // Adopting cide must not cost someone the status line they already had. The chained
    // command's stdout has to reach the CLI verbatim.
    let Some(hook) = hook_binary() else {
        eprintln!("SKIP: cide-hook is not built");
        return;
    };

    let settings = inline_settings(
        &hook.to_string_lossy(),
        &StatusLine::Chained("printf MINE".into()),
    );
    let command = settings["statusLine"]["command"]
        .as_str()
        .expect("a chained statusline is configured");
    assert!(command.ends_with("printf MINE"), "got {command}");

    // Run it the way the CLI would: payload on stdin, read stdout.
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(br#"{"session_id":"x"}"#)
        .expect("writes");
    let out = child.wait_with_output().expect("completes");

    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "MINE",
        "the user's own statusline output was not passed through verbatim"
    );
}
