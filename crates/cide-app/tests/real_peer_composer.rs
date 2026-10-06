//! Quota-free peer transport audit against the installed Codex TUI. State is isolated,
//! and its only provider points at an unused loopback port, never a paid model endpoint.
use cide_core::peer_composer;
use cide_ipc::{ConsoleHarness, screen::ScreenCapture};
use cide_pty::{Geometry, OutputObserver, PtySession, SpawnSpec};
use std::{
    path::Path,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires installed Codex; isolated state and unused local provider, no quota"]
fn real_codex_receives_a_complete_chunked_peer_reply() {
    let dir =
        std::env::temp_dir().join(format!("cide-peer-composer-{}", cide_ipc::SessionId::new()));
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let result = std::panic::catch_unwind(|| run(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn run(dir: &Path) {
    let (tx, rx) = mpsc::channel();
    let mut spec = SpawnSpec::new("codex", dir)
        .env("CODEX_HOME", dir.join("home").to_string_lossy())
        .env("CIDE_PEER_AUDIT_KEY", "unused-test-key");
    spec.geometry = Geometry::new(100, 30, 8, 16);
    for setting in [
        "check_for_update_on_startup=false",
        "notice.hide_rate_limit_model_nudge=true",
        "model_provider=\"peer-audit\"",
        "model=\"gpt-5\"",
        "model_providers.peer-audit.name=\"Peer audit\"",
        "model_providers.peer-audit.base_url=\"http://127.0.0.1:9/v1\"",
        "model_providers.peer-audit.env_key=\"CIDE_PEER_AUDIT_KEY\"",
        "model_providers.peer-audit.wire_api=\"responses\"",
        "model_providers.peer-audit.request_max_retries=0",
    ] {
        spec = spec.arg("-c").arg(setting);
    }
    spec = spec.arg("-c").arg(format!(
        "projects.\"{}\".trust_level=\"trusted\"",
        dir.display()
    ));
    spec.output_observer = Some(OutputObserver::new(move |bytes| {
        let _ = tx.send(bytes.to_vec());
    }));
    struct Child(Arc<PtySession>);
    impl Drop for Child {
        fn drop(&mut self) {
            self.0.kill();
        }
    }
    let child = Child(PtySession::spawn(spec).unwrap());
    let pty = &child.0;
    let pump = || {
        while let Ok(bytes) = rx.try_recv() {
            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                pty.write(b"\x1b[1;1R".to_vec());
            }
        }
    };
    let wait = |predicate: &dyn Fn(&ScreenCapture) -> bool, label: &str, settle: Duration| {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut stable = None;
        loop {
            pump();
            let capture = pty.capture_screen();
            if predicate(&capture) {
                let since = stable.get_or_insert_with(Instant::now);
                if since.elapsed() >= settle {
                    break;
                }
            } else {
                stable = None;
            }
            assert!(
                !pty.has_exited() && Instant::now() < deadline,
                "{label}: {capture:?}"
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    wait(
        &|capture| {
            peer_composer::composer(ConsoleHarness::Codex, capture).is_some_and(|(text, col)| {
                col == 2 && peer_composer::empty(ConsoleHarness::Codex, &text)
            })
        },
        "empty composer",
        Duration::from_millis(100),
    );
    let body = peer_composer::normalize(&format!(
        "Chat from Peer (opencode): {} Final independent finding Привет 世界 😀 verified.",
        (0..24).map(|n| format!("Finding-{n}: the source is readable and this unique section needs a measured benchmark before adoption. ")).collect::<String>()
    )).unwrap();
    let follow_up = "Chat from Peer (opencode): A second independent review arrived while your first turn was still waiting for the provider. Keep both findings intact and respond when ready.";
    for body in [body.as_str(), follow_up] {
        wait(
            &|capture| {
                peer_composer::composer(ConsoleHarness::Codex, capture).is_some_and(
                    |(text, col)| col == 2 && peer_composer::empty(ConsoleHarness::Codex, &text),
                )
            },
            "ready for the next peer reply",
            Duration::from_millis(100),
        );
        let marker = "[cide:00000000-0000-0000-0000-000000000000]";
        for (input, expected, witness) in peer_composer::marked_chunks(body, marker) {
            pty.write(input);
            wait(
                &|capture| {
                    peer_composer::matches_capture(
                        ConsoleHarness::Codex,
                        capture,
                        &expected,
                        witness,
                    )
                },
                "marked chunk",
                Duration::from_millis(100),
            );
        }
        pty.write(vec![127; marker.len()]);
        wait(
            &|capture| peer_composer::matches_capture(ConsoleHarness::Codex, capture, body, 70),
            "complete body",
            Duration::from_millis(500),
        );
        pty.write(vec![b'\r']);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            pump();
            if submitted(&dir.join("home/history.jsonl"), body) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Codex did not record the complete submitted peer reply: {:?}",
                pty.capture_screen(),
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn submitted(history: &Path, body: &str) -> bool {
    std::fs::read_to_string(history)
        .unwrap_or_default()
        .lines()
        .any(|line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return false;
            };
            v["text"] == body
        })
}
