//! The facts about the Codex CLI that only the real binary can confirm. (M44)
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
//! The third spends one turn and one resume of the user's own quota, and is run deliberately:
//!
//! ```sh
//! cargo test -p cide-agents --test real_codex -- --ignored
//! ```
//!
//! What they pin is the half of the harness a `--help` probe could not reach: that the brief
//! decodes byte for byte out of the TOML the argv carries and lands as a developer message;
//! that the four `mcp_servers.cide.*` overrides register the server *with its environment*;
//! and — the quota-spending one — that a turn starts with `thread.started`, ends with
//! `turn.completed` and exit 0, and resumes under the same id from the same directory *and*
//! from any other: measured once, a codex conversation is found by its id alone, so the
//! worktree rule the file-based harnesses share does not apply here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cide_agents::{CodexHarness, Harness as _, LoadedAgent, Observation, RunPlan, SessionBinding};
use cide_core::child_env::run_filter_with;
use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, RunState, SessionId, Theme};
use cide_pty::SpawnSpec;

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

/// The spec, run to completion as a plain child rather than in a PTY, so stdout is pure
/// JSONL. Returns (ok, stdout, stderr).
fn run(spec: &SpawnSpec, deadline: Duration) -> (bool, String, String) {
    let mut command = std::process::Command::new(&spec.program);
    command.args(&spec.args).current_dir(&spec.cwd);
    for (k, v) in &spec.env {
        command.env(k, v);
    }
    for k in &spec.env_remove {
        command.env_remove(k);
    }
    let bin_dir = vec![codex().parent().unwrap().to_path_buf()];
    let filtered = run_filter_with(command, None, deadline, &bin_dir).expect("codex ran");
    (
        filtered.ok,
        String::from_utf8_lossy(&filtered.stdout).into_owned(),
        filtered.stderr,
    )
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
    assert_eq!(server.len(), 8, "{:?}", spawn.spec.args);

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
    for needle in ["/opt/cide/cide-hook", "mcp", "CIDE_RUN", "CIDE_AGENT_SOCK"] {
        assert!(stdout.contains(needle), "no {needle} in:\n{stdout}");
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// One real turn: `thread.started` first with a uuid the binding captures, `turn.completed`
/// last, exit 0, `Running` and never `Idle` on the way, `pong` in the model's words; then a
/// respawn on `resume` continues under the same id from the same directory — and from another
/// one, which the first run of this test measured and which the journal records: no worktree
/// rule on this harness.
#[test]
#[ignore = "spawns the real codex and spends the user's quota"]
fn a_real_turn_starts_a_thread_completes_and_resumes_from_anywhere() {
    // Deliberately not a git repository: `--skip-git-repo-check` is what lets a task-less run
    // stand in a project root that is not one.
    let dir = temp_dir("turn");
    let agent = role("You answer with exactly one word and nothing else.");
    let plan = plan(&agent, &dir, "Reply with exactly the single word: pong");
    let spawn = CodexHarness.spawn_spec(&plan).expect("spawnable");
    let SessionBinding::Harness { capture, .. } = spawn.binding else {
        panic!("codex binds by capture");
    };

    let (ok, stdout, stderr) = run(&spawn.spec, Duration::from_secs(180));
    println!("--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    assert!(ok, "the first turn exited non-zero: {stderr}");

    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with('{'))
        .collect();
    assert!(!lines.is_empty(), "no events on stdout");
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["type"], "thread.started", "{first}");
    let thread = capture(lines[0]).expect("the binding captures the first line");
    assert!(
        thread.parse::<SessionId>().is_ok(),
        "a thread id is a uuid: {thread}"
    );
    let last: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).unwrap();
    assert_eq!(last["type"], "turn.completed", "{last}");

    let mut state = RunState::Starting;
    for line in &lines {
        if let Some(next) = CodexHarness.observe(state.clone(), Observation::Line(line)) {
            assert_ne!(next, RunState::Idle, "no line ends a turn");
            state = next;
        }
    }
    assert_eq!(state, RunState::Running);
    assert_eq!(
        CodexHarness.observe(state, Observation::Exit(0)),
        Some(RunState::Finished { code: 0 })
    );
    assert!(
        stdout.contains("\"agent_message\"") && stdout.to_lowercase().contains("pong"),
        "the model's words are in the stream: {stdout}"
    );

    // A follow-up: the same conversation, from the same directory.
    let mut follow = self::plan(&agent, &dir, "What single word did you reply with?");
    follow.run = plan.run;
    let respawn = CodexHarness
        .respawn_spec(&follow, &thread)
        .expect("continuable");
    let (ok, stdout, stderr) = run(&respawn.spec, Duration::from_secs(180));
    println!("--- resume stdout ---\n{stdout}\n--- resume stderr ---\n{stderr}");
    assert!(ok, "the resume exited non-zero: {stderr}");
    let resumed = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .expect("an event");
    assert_eq!(
        capture(resumed).as_deref(),
        Some(thread.as_str()),
        "the resumed thread is the same thread: {resumed}"
    );
    assert!(stdout.to_lowercase().contains("pong"), "{stdout}");

    // Measured 2026-09-07: the id is found from another directory too.
    let elsewhere = temp_dir("elsewhere");
    let mut away = self::plan(&agent, &elsewhere, "What single word did you reply with?");
    away.run = plan.run;
    let respawn = CodexHarness
        .respawn_spec(&away, &thread)
        .expect("continuable");
    let (ok, stdout, stderr) = run(&respawn.spec, Duration::from_secs(180));
    println!("--- resume from another cwd ---\n{stdout}\n--- stderr ---\n{stderr}");
    assert!(ok, "the resume from elsewhere exited non-zero: {stderr}");
    let resumed = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .expect("an event");
    assert_eq!(
        capture(resumed).as_deref(),
        Some(thread.as_str()),
        "a conversation is found by its id alone: {resumed}"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&elsewhere).ok();
}
