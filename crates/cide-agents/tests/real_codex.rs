//! The facts about the Codex CLI that only the real binary can confirm. (M44, rehosted M93)
//!
//! `#[ignore]`d, like every test in this workspace that spawns a real harness: they need
//! `codex` on `PATH` and a logged-in account. Two of the three spend **nothing** — `codex debug
//! prompt-input` renders what the model would be shown without calling one, and `codex mcp get`
//! reads configuration — and are worth running on every change to `harness/codex.rs`:
//!
//! ```sh
//! cargo test -p cide-agents --test real_codex -- --ignored --skip a_real_turn
//! ```
//!
//! The third spends two short turns and one resume of the user's own quota, and is run
//! deliberately:
//!
//! ```sh
//! cargo test -p cide-agents --test real_codex -- --ignored
//! ```
//!
//! What they pin is the half of the harness a `--help` probe could not reach: that the brief
//! decodes byte for byte out of the TOML the argv carries and lands as a developer message;
//! that the `mcp_servers.cide.*` overrides register the server *with its environment*; and —
//! the quota-spending one, since M93 — that the **interactive TUI** a run now is fires the hook
//! table cide hands it on the command line (with the trust bypass), that `SessionStart` names
//! the thread the binding captures, that a typed follow-up (a paste and Enter) is a second turn
//! in the same conversation, and that `codex resume <thread>` continues it from another
//! directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cide_agents::{
    CodexHarness, Delivery, Harness as _, LoadedAgent, Observation, RunPlan, SessionBinding,
};
use cide_core::child_env::run_filter_with;
use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, RunState, SessionId, Theme};

fn role(prompt: &str) -> LoadedAgent {
    LoadedAgent {
        def: AgentDef {
            id: AgentId("probe".into()),
            label: "Probe".into(),
            scope: cide_ipc::agents::AgentScope::Project,
            harness: cide_ipc::Harness::Codex,
            description: "Answers one word.".into(),
            system_prompt: prompt.into(),
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

fn plan<'a>(agent: &'a LoadedAgent, cwd: &Path, prompt: &str) -> RunPlan<'a> {
    RunPlan {
        run: RunId::new(),
        session: SessionId::new(),
        agent,
        cwd: cwd.to_path_buf(),
        project: ProjectId::new(),
        task: None,
        task_title: None,
        change: None,
        spec_cli: None,
        spec_apply: None,
        prompt: prompt.into(),
        // No bridge: the brief is the role's own words, and no server is registered — the
        // tests that want one set it themselves.
        hook_bin: None,
        hook_sock: None,
        agent_sock: None,
        events_path: None,
        theme: Theme::Dark,
        proxy: cide_core::proxy::ProxyEnv::default(),
        env: Vec::new(),
        geometry: Geometry::default(),
        claude: cide_ipc::ClaudeSettings::default(),
        codex: cide_ipc::CodexSettings::default(),
        llm: cide_ipc::LlmSettings::default(),
        choice: None,
        harness: agent.def.harness,
        // The project default: a bypassed sandbox, which is what a task run gets.
        unattended: cide_agents::config::Unattended::Bypass,
        tracker_paragraphs: true,
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cide-real-codex-{tag}-{}", SessionId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every `-c key=value` pair in an argv whose key starts with `prefix`, as the two tokens.
fn overrides(args: &[String], prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 && args[i - 1] == "-c" && arg.starts_with(prefix) {
            out.push("-c".to_string());
            out.push(arg.clone());
        }
    }
    out
}

/// `codex` resolved the way the harness's own `models()` resolves it.
fn codex() -> PathBuf {
    cide_core::toolchain::which("codex").expect("codex on PATH")
}

/// The brief the harness composes reaches the model as the first developer message, byte for
/// byte — the real decoder for the TOML encoding the argv carries. Spends nothing.
#[test]
#[ignore = "runs the real codex; needs the binary on PATH"]
fn the_real_codex_puts_the_brief_first_as_a_developer_message() {
    let dir = temp_dir("brief");
    let agent = role(
        "CIDE-PROBE: you answer with one word.\nQuote it like \"this\", and mind C:\\paths\\too.\n\n\tIndented, then done.",
    );
    let plan = plan(&agent, &dir, "ping");
    let spawn = CodexHarness.spawn_spec(&plan).expect("spawnable");
    let brief = overrides(&spawn.spec.args, "developer_instructions=");
    assert_eq!(brief.len(), 2, "{:?}", spawn.spec.args);

    let mut command = std::process::Command::new(codex());
    command
        .args(&brief)
        .args(["debug", "prompt-input", "ping"])
        .current_dir(&dir);
    let filtered = run_filter_with(
        command,
        None,
        Duration::from_secs(30),
        &[codex().parent().unwrap().to_path_buf()],
    )
    .expect("codex ran");
    assert!(filtered.ok, "{}", filtered.stderr);

    let rendered: serde_json::Value =
        serde_json::from_slice(&filtered.stdout).expect("prompt-input prints JSON");
    let messages = rendered.as_array().expect("a list of messages");
    let developer_texts: Vec<&str> = messages
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("developer"))
        .flat_map(|m| {
            m.get("content")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
                .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
        })
        .collect();

    let expected = "CIDE-PROBE: you answer with one word.\nQuote it like \"this\", and mind C:\\paths\\too.\n\n\tIndented, then done.";
    assert!(
        developer_texts.contains(&expected),
        "the brief did not decode as composed:\n{developer_texts:#?}"
    );
    assert_eq!(
        developer_texts.first().copied(),
        Some(expected),
        "and it is the first developer text the model sees"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The four `mcp_servers.cide.*` overrides register cide's server with the run's own
/// environment — the whitelist codex hands an MCP server would otherwise strip it. Spends
/// nothing.
#[test]
#[ignore = "runs the real codex; needs the binary on PATH"]
fn the_real_codex_registers_cides_server_with_its_environment() {
    let dir = temp_dir("mcp");
    let agent = role("You are a probe.");
    let mut plan = plan(&agent, &dir, "ping");
    plan.hook_bin = Some(PathBuf::from("/opt/cide/cide-hook"));
    plan.agent_sock = Some(PathBuf::from("/run/user/1000/cide-agents-42.sock"));
    let spawn = CodexHarness.spawn_spec(&plan).expect("spawnable");
    let server = overrides(&spawn.spec.args, "mcp_servers.cide.");
    // command, args, and three variables: `CIDE_RUN`, `CIDE_SESSION` (M93) and the socket.
    assert_eq!(server.len(), 10, "{:?}", spawn.spec.args);

    let mut command = std::process::Command::new(codex());
    command
        .args(&server)
        .args(["mcp", "get", "cide"])
        .current_dir(&dir);
    let filtered = run_filter_with(
        command,
        None,
        Duration::from_secs(30),
        &[codex().parent().unwrap().to_path_buf()],
    )
    .expect("codex ran");
    assert!(filtered.ok, "{}", filtered.stderr);
    let stdout = String::from_utf8_lossy(&filtered.stdout);
    for needle in [
        "/opt/cide/cide-hook",
        "mcp",
        "CIDE_RUN",
        "CIDE_SESSION",
        "CIDE_AGENT_SOCK",
    ] {
        assert!(stdout.contains(needle), "no {needle} in:\n{stdout}");
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// A stand-in `cide-hook`: every hook frame appended to `<dir>/frames.log` as
/// `<Event>\t<payload>`, and `mcp` refused at once so codex does not wait on a server.
fn recorder(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("cide-hook");
    let log = dir.join("frames.log");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n[ \"$1\" = mcp ] && exit 1\n{{ printf '%s\\t' \"$1\"; tr -d '\\n'; echo; }} >> '{}'\nexit 0\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// The recorded frames, parsed.
fn frames(dir: &Path) -> Vec<cide_claude::HookFrame> {
    std::fs::read_to_string(dir.join("frames.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (event, payload) = line.split_once('\t')?;
            Some(cide_claude::HookFrame::new(
                event,
                serde_json::from_str(payload).ok()?,
            ))
        })
        .collect()
}

/// Wait until `count` `Stop` frames have arrived, and return every frame so far.
fn until_stops(dir: &Path, count: usize, deadline: Duration) -> Vec<cide_claude::HookFrame> {
    let end = std::time::Instant::now() + deadline;
    loop {
        let all = frames(dir);
        if all.iter().filter(|f| f.event == "Stop").count() >= count {
            return all;
        }
        assert!(
            std::time::Instant::now() < end,
            "no Stop #{count} in time; frames so far: {:?}",
            all.iter().map(|f| f.event.as_str()).collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn last_message(frames: &[cide_claude::HookFrame]) -> String {
    frames
        .iter()
        .rev()
        .find(|f| f.event == "Stop")
        .and_then(|f| f.payload.get("last_assistant_message"))
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_lowercase()
}

/// One real TUI run: the command-line hook table fires, `SessionStart` names a uuid thread the
/// harness captures, the frames walk the run `Idle → Running → Idle`, a typed follow-up is a
/// second turn in the same thread, and `codex resume <thread>` continues it from another
/// directory.
#[test]
#[ignore = "spawns the real codex and spends the user's quota"]
fn a_real_turn_starts_a_thread_completes_and_resumes_from_anywhere() {
    let dir = temp_dir("turn");
    let agent = role("You answer with exactly one word and nothing else.");
    let mut plan = plan(&agent, &dir, "Reply with exactly the single word: pong");
    plan.hook_bin = Some(recorder(&dir));
    plan.geometry = Geometry {
        cols: 120,
        rows: 40,
        ..Geometry::default()
    };
    let spawn = CodexHarness.spawn_spec(&plan).expect("spawnable");
    assert!(matches!(spawn.binding, SessionBinding::Caller));
    let pty = cide_pty::PtySession::spawn(spawn.spec).expect("codex starts in a PTY");

    let first = until_stops(&dir, 1, Duration::from_secs(180));
    let start = first
        .iter()
        .find(|f| f.event == "SessionStart")
        .expect("SessionStart fired from a command-line hook");
    let thread = CodexHarness
        .capture_hook(start)
        .expect("the thread is captured");
    assert!(
        thread.parse::<SessionId>().is_ok(),
        "a thread id is a uuid: {thread}"
    );
    assert!(
        last_message(&first).contains("pong"),
        "{:?}",
        last_message(&first)
    );

    let mut state = RunState::Starting;
    let mut seen = Vec::new();
    for frame in &first {
        if let Some(next) = CodexHarness.observe(state.clone(), Observation::Hook(frame)) {
            seen.push(next.clone());
            state = next;
        }
    }
    assert!(seen.contains(&RunState::Running), "{seen:?}");
    assert_eq!(
        state,
        RunState::Idle,
        "a Stop hands the turn back: {seen:?}"
    );

    // A typed follow-up, as the app types one: the harness's own bytes.
    let Delivery::Stdin(bytes) = CodexHarness.deliver("What single word did you reply with?")
    else {
        panic!("codex takes a follow-up typed");
    };
    pty.write(bytes);
    let second = until_stops(&dir, 2, Duration::from_secs(180));
    assert!(
        last_message(&second).contains("pong"),
        "{:?}",
        last_message(&second)
    );
    assert!(
        second
            .iter()
            .filter_map(|f| f.session_id())
            .all(|id| id == thread),
        "every frame is the same thread"
    );
    pty.kill();

    // Back on the thread from somewhere else.
    let elsewhere = temp_dir("elsewhere");
    let mut away = self::plan(&agent, &elsewhere, "Say that single word once more.");
    away.hook_bin = Some(recorder(&elsewhere));
    let respawn = CodexHarness
        .respawn_spec(&away, &thread)
        .expect("continuable");
    let resumed = cide_pty::PtySession::spawn(respawn.spec).expect("codex resumes");
    let third = until_stops(&elsewhere, 1, Duration::from_secs(180));
    assert!(
        third
            .iter()
            .filter_map(|f| f.session_id())
            .all(|id| id == thread),
        "a conversation is found by its id alone"
    );
    assert!(
        last_message(&third).contains("pong"),
        "{:?}",
        last_message(&third)
    );
    resumed.kill();

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&elsewhere).ok();
}
