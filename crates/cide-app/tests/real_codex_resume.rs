//! Free installed-CLI proof: resume two generated transcripts A → B → A without submitting
//! a model prompt. The same process-owned rollout/activation reader feeds the session worker.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use cide_core::{codex_active, user_inputs::TranscriptInputs};
use cide_pty::{OutputObserver, PtySession, SpawnSpec};
use serde_json::json;

#[test]
#[ignore = "requires installed Codex; isolated home/provider, no model calls"]
fn a_real_codex_resume_changes_owned_activation_before_any_model_turn() {
    let dir =
        std::env::temp_dir().join(format!("cide-codex-resume-{}", cide_ipc::SessionId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let result = std::panic::catch_unwind(|| run(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn seed(dir: &Path, id: &str, prompt: &str) {
    let day = dir.join("home/sessions/2026/10/06");
    std::fs::create_dir_all(&day).unwrap();
    let records = [
        json!({"type":"session_meta","payload":{"id":id,"timestamp":"2026-10-06T00:00:00Z",
            "cwd":dir,"originator":"codex_cli_rs","cli_version":"0.160.0","source":"cli",
            "model_provider":"resume-test","base_instructions":{"text":"A local transcript fixture."}}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user",
            "content":[{"type":"input_text","text":prompt}]}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage",
            "id":format!("message-{id}"),"content":[{"type":"text","text":prompt}]}}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer",
            "content":[{"type":"output_text","text":"Local recorded answer."}]}}),
    ];
    let text = records
        .into_iter()
        .map(|mut record| {
            record["timestamp"] = json!("2026-10-06T00:00:00.000Z");
            format!("{record}\n")
        })
        .collect::<String>();
    std::fs::write(
        day.join(format!("rollout-2026-10-06T00-00-00-{id}.jsonl")),
        text,
    )
    .unwrap();
}

fn run(dir: &Path) {
    let a = cide_ipc::SessionId::new().to_string();
    let b = cide_ipc::SessionId::new().to_string();
    seed(dir, &a, "First session prompt");
    seed(dir, &b, "Second session prompt");
    let (tx, rx) = mpsc::channel();
    let mut spec = SpawnSpec::new("codex", dir)
        .arg("resume")
        .arg(&a)
        .arg("--no-alt-screen")
        .env("CODEX_HOME", dir.join("home").to_string_lossy())
        .env("CIDE_RESUME_TEST_KEY", "unused-test-key");
    for setting in [
        "check_for_update_on_startup=false",
        "notice.hide_rate_limit_model_nudge=true",
        "model_provider=\"resume-test\"",
        "model=\"gpt-5\"",
        "model_providers.resume-test.name=\"Resume test\"",
        "model_providers.resume-test.base_url=\"http://127.0.0.1:9/v1\"",
        "model_providers.resume-test.env_key=\"CIDE_RESUME_TEST_KEY\"",
        "model_providers.resume-test.wire_api=\"responses\"",
        "model_providers.resume-test.request_max_retries=0",
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
    let pid = pty.child_pid().unwrap();
    let mut cache: HashMap<PathBuf, TranscriptInputs> = HashMap::new();
    let mut bytes = Vec::new();
    for (target, prompt) in [
        (&a, "First session prompt"),
        (&b, "Second session prompt"),
        (&a, "First session prompt"),
    ] {
        if !bytes.is_empty() {
            pty.write(format!("\x1b[200~/resume {target}\x1b[201~").into_bytes());
            std::thread::sleep(Duration::from_millis(200));
            pty.write(b"\r".to_vec());
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(
                !pty.has_exited() && Instant::now() < deadline,
                "resume did not activate {target}: {}",
                String::from_utf8_lossy(&bytes)
            );
            if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(50)) {
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    pty.write(b"\x1b[1;1R".to_vec());
                }
                if chunk.windows(5).any(|w| w == b"\x1b]11;") {
                    pty.write(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec());
                }
                bytes.extend(chunk);
            }
            let activations = codex_active::owned_rollouts(pid)
                .into_iter()
                .filter_map(|path| {
                    let inputs = cache.entry(path.clone()).or_default();
                    inputs.refresh(&path).ok()?;
                    Some((path, inputs.activated_at()?))
                })
                .collect::<Vec<_>>();
            if let Some((path, _)) = codex_active::latest(activations)
                && codex_active::thread_of(&path).as_deref() == Some(target)
                && cide_pty::codex_clear::composer(&pty.capture_screen()).is_some()
            {
                assert_eq!(cache[&path].page(None).inputs.last().unwrap().text, prompt);
                break;
            }
        }
    }
}
