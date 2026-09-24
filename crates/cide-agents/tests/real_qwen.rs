//! The facts about Qwen Code that only the real CLI can confirm. (M43)
//!
//! `#[ignore]`d, like every test in this workspace that spawns a real harness: they need `qwen`
//! on `PATH`, a configured model endpoint, and they spend the user's own quota — one turn and
//! two resumes. Run deliberately: `cargo test -p cide-agents --test real_qwen -- --ignored`.
//!
//! What they pin is the half of the harness the morning's measurements could not reach because
//! the model endpoint was down: that a completed turn writes a `result` event to the FIFO and
//! the process stays interactive after it, and that the chat file is resumable from the
//! directory it was started in and from nowhere else.

use std::io::Read as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cide_agents::{Harness as _, LoadedAgent, QwenHarness, RunPlan};
use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, TaskId, Theme};
use cide_pty::PtySession;

fn role() -> LoadedAgent {
    LoadedAgent {
        def: AgentDef {
            id: AgentId("probe".into()),
            label: "Probe".into(),
            scope: cide_ipc::agents::AgentScope::Project,
            harness: cide_ipc::Harness::Qwen,
            description: "Answers one word.".into(),
            system_prompt: "You answer with exactly one word and nothing else.".into(),
            model: None,
            color: None,
            unavailable: None,
            max_concurrent: 1,
            worktree: false,
        },
        origin: PathBuf::from("/nowhere/.cide/agents/probe.md"),
        shadows: None,
        tools: Vec::new(),
        permission_mode: None,
        effort: None,
        extras: Vec::new(),
    }
}

fn mkfifo(path: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt as _;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: a NUL-terminated path and a mode; `mkfifo` reads both and returns.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
}

/// A completed turn ends in a `result` event on the FIFO, the process stays at its prompt
/// afterwards, and the conversation resumes from its own directory only.
#[test]
#[ignore = "spawns the real qwen and spends the user's quota"]
fn a_real_turn_ends_in_a_result_event_and_the_child_stays_interactive() {
    let dir = std::env::temp_dir().join(format!("cide-real-qwen-{}", SessionId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("events.fifo");
    mkfifo(&fifo);

    let agent = role();
    let session = SessionId::new();
    let plan = RunPlan {
        run: RunId::new(),
        session,
        agent: &agent,
        cwd: dir.clone(),
        project: ProjectId::new(),
        task: Some(TaskId("t-1".into())),
        task_title: Some("probe".into()),
        change: None,
        spec_cli: None,
        spec_apply: None,
        prompt: "Reply with exactly the single word: pong".into(),
        hook_bin: None,
        hook_sock: None,
        agent_sock: None,
        events_path: Some(fifo.clone()),
        theme: Theme::Dark,
        proxy: cide_core::proxy::ProxyEnv::default(),
        env: Vec::new(),
        geometry: Geometry::default(),
        claude: cide_ipc::ClaudeSettings::default(),
        llm: cide_ipc::LlmSettings::default(),
        choice: None,
        harness: agent.def.harness,
        unattended: cide_agents::config::Unattended::Bypass,
        tracker_paragraphs: true,
    };
    let spawn = QwenHarness.spawn_spec(&plan).expect("spawnable");
    let pty = PtySession::spawn(spawn.spec).expect("spawn qwen");

    // Read the FIFO the way `cide_app::event_tap` does: non-blocking, read-only.
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut result: Option<serde_json::Value> = None;
    while Instant::now() < deadline && result.is_none() {
        match file.read(&mut chunk) {
            Ok(0) => std::thread::sleep(Duration::from_millis(100)),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                while let Some(at) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=at).collect();
                    if let Ok(event) = serde_json::from_slice::<serde_json::Value>(&line)
                        && event["type"] == "result"
                    {
                        result = Some(event);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("reading the FIFO: {error}"),
        }
        assert!(!pty.has_exited(), "the child exited before the turn ended");
    }
    let result = result.expect("a `result` event ends a completed turn");
    assert_eq!(result["is_error"], false, "{result}");
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !pty.has_exited(),
        "`-i` keeps the process at its prompt after the turn"
    );
    pty.kill();
    let gone = Instant::now() + Duration::from_secs(10);
    while !pty.has_exited() && Instant::now() < gone {
        std::thread::sleep(Duration::from_millis(50));
    }

    // Filed where `lifecycle::transcript_of` says, and resumable from there only.
    let encoded: String = dir
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let chat = PathBuf::from(std::env::var("HOME").unwrap())
        .join(".qwen/projects")
        .join(&encoded)
        .join("chats")
        .join(format!("{session}.jsonl"));
    assert!(chat.is_file(), "the chat file is at {}", chat.display());

    let same = std::process::Command::new("qwen")
        .args(["-p", "What single word did you reply with?", "--resume"])
        .arg(session.to_string())
        .args(["-o", "json", "--approval-mode", "yolo"])
        .current_dir(&dir)
        .output()
        .expect("run qwen");
    assert!(
        same.status.success(),
        "resume from the same cwd: {}",
        String::from_utf8_lossy(&same.stderr)
    );
    let elsewhere =
        std::env::temp_dir().join(format!("cide-real-qwen-elsewhere-{}", SessionId::new()));
    std::fs::create_dir_all(&elsewhere).unwrap();
    let other = std::process::Command::new("qwen")
        .args(["-p", "What single word did you reply with?", "--resume"])
        .arg(session.to_string())
        .args([
            "-o",
            "json",
            "--approval-mode",
            "yolo",
            "--max-wall-time",
            "60",
        ])
        .current_dir(&elsewhere)
        .output()
        .expect("run qwen");
    assert!(
        !other.status.success(),
        "a conversation is filed per directory and must not resume from another: {}",
        String::from_utf8_lossy(&other.stdout)
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&elsewhere);
}
