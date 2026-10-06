//! Free, opt-in proof of Codex's actual composer and `/clear` reset bytes.
//! The isolated provider points at an unused loopback port; no model turn is submitted.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use cide_pty::{OutputObserver, PtySession, SpawnSpec, codex_clear};
use parking_lot::Mutex;

#[test]
#[ignore = "requires the real Codex binary; isolated, no model calls"]
fn a_real_codex_clear_is_detected_without_submitting_a_prompt() {
    let dir = std::env::temp_dir().join(format!("cide-codex-clear-{}", cide_ipc::SessionId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let result = std::panic::catch_unwind(|| run(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn run(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let observer = Arc::new(Mutex::new(codex_clear::ClearObserver::default()));
    let callback = Arc::clone(&observer);
    let (tx, rx) = mpsc::channel();
    let mut spec = SpawnSpec::new("codex", dir)
        .env("CODEX_HOME", dir.join("home").to_string_lossy())
        .env("OPENAI_API_KEY", "unused-test-key")
        .env("CIDE_CLEAR_TEST_KEY", "unused-test-key");
    for setting in [
        "check_for_update_on_startup=false",
        "notice.hide_rate_limit_model_nudge=true",
        "model_provider=\"clear-test\"",
        "model=\"gpt-5\"",
        "model_providers.clear-test.name=\"Clear test\"",
        "model_providers.clear-test.base_url=\"http://127.0.0.1:9/v1\"",
        "model_providers.clear-test.env_key=\"CIDE_CLEAR_TEST_KEY\"",
        "model_providers.clear-test.wire_api=\"responses\"",
        "model_providers.clear-test.request_max_retries=0",
    ] {
        spec = spec.arg("-c").arg(setting);
    }
    spec = spec.arg("-c").arg(format!(
        "projects.\"{}\".trust_level=\"trusted\"",
        dir.display()
    ));
    spec.output_observer = Some(OutputObserver::new(move |bytes| {
        let cleared = callback.lock().output(bytes);
        let _ = tx.send((bytes.to_vec(), cleared));
    }));
    let pty = PtySession::spawn(spec).unwrap();
    // A guard kills this particular child even if an assertion fails.
    struct Child(Arc<PtySession>);
    impl Drop for Child {
        fn drop(&mut self) {
            self.0.kill();
        }
    }
    let child = Child(pty);
    let pty = &child.0;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut submitted = false;
    let mut typed = false;
    let mut output = Vec::new();
    loop {
        assert!(
            !pty.has_exited(),
            "Codex exited before clear; output: {}",
            String::from_utf8_lossy(&output)
        );
        assert!(
            Instant::now() < deadline,
            "Codex never confirmed clear; output: {}",
            String::from_utf8_lossy(&output)
        );
        if let Ok((bytes, cleared)) = rx.recv_timeout(Duration::from_millis(50)) {
            output.extend_from_slice(&bytes);
            // Codex asks for the cursor before drawing; no xterm is attached in this fixture.
            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                pty.write(b"\x1b[1;1R".to_vec());
            }
            if cleared {
                assert!(submitted, "startup redraws cannot clear a session");
                break;
            }
        }
        let capture = pty.capture_screen();
        let ready = capture
            .lines
            .iter()
            .flat_map(|l| l.runs.iter())
            .any(|r| r.text.contains("gpt-5"));
        if !submitted
            && ready
            && let Some(visible) = codex_clear::composer(&capture)
        {
            if !typed && visible.is_empty() {
                let command = b"\x1b[200~/clear\x1b[201~";
                observer.lock().input(command, Some(&visible));
                pty.write(command.to_vec());
                typed = true;
            } else if typed && visible == "/clear" {
                observer.lock().input(b"\r", Some(&visible));
                pty.write(b"\r".to_vec());
                submitted = true;
            }
        }
    }
}
