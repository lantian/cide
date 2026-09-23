//! The facts about MiMo Code (`mimo`) that only the real binary can confirm. (M81)
//!
//! `#[ignore]`d, like every test in this workspace that spawns a real harness, and they need
//! `mimo` on `PATH`. The first three spend **nothing**: one asks `mimo debug agent` whether the
//! role cide hands over is registered, and two point a provider at a port nothing listens on, so
//! no model is ever reached. Run them on every change to `harness/opencode.rs`:
//!
//! ```sh
//! cargo test -p cide-agents --test real_mimo -- --ignored --skip a_real_turn
//! ```
//!
//! The fourth spends one small turn and one follow-up of the user's own quota, on whichever
//! model `CIDE_MIMO_MODEL` names (`xiaomi/mimo-v2.6-flash` when unset), and is run deliberately.
//!
//! # Why mimo has its own file when it shares opencode's code
//!
//! Because it shares the *code* and not the *binary*. Every fact the shared implementation leans
//! on — the variable the document travels in, the event envelope, the error shape `failover`
//! reads by field name — is another project's to change, and this one is a fork that has already
//! diverged twice where it matters: it retries a dead endpoint silently for up to fifteen minutes
//! rather than failing, and it exits 0 after a fatal error where opencode exits 1. Both were
//! found here, and both would otherwise have meant a pool that never fails over and not one test
//! failing anywhere.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cide_agents::harness::opencode::{MIMO_CLI, failover, test_model};
use cide_agents::{FailoverReason, Harness as _, LoadedAgent, MimoHarness, Observation, RunPlan};
use cide_core::child_env::run_filter_with;
use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, RunState, SessionId, Theme};
use cide_pty::SpawnSpec;

fn role(prompt: &str) -> LoadedAgent {
    LoadedAgent {
        def: AgentDef {
            id: AgentId("probe".into()),
            label: "Probe".into(),
            scope: cide_ipc::agents::AgentScope::Project,
            harness: cide_ipc::Harness::Mimo,
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
        hook_bin: None,
        hook_sock: None,
        agent_sock: None,
        events_path: None,
        theme: Theme::Dark,
        proxy: cide_core::proxy::ProxyEnv::default(),
        geometry: Geometry::default(),
        claude: cide_ipc::ClaudeSettings::default(),
        llm: cide_ipc::LlmSettings::default(),
        choice: None,
        harness: agent.def.harness,
        unattended: cide_agents::config::Unattended::Bypass,
    }
}

/// A provider on a closed port, as a person would declare one in Settings → Models.
///
/// Port 1 for `real_opencode.rs`'s reason: privileged, never bound, refused at once.
fn dead_llm() -> cide_ipc::LlmSettings {
    cide_ipc::LlmSettings {
        providers: vec![cide_ipc::LlmProvider::Custom {
            id: "cide-test-dead".into(),
            label: "cide test (nothing listening)".into(),
            enabled: true,
            npm: "@ai-sdk/openai-compatible".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: "not-a-key".into(),
            models: vec![cide_ipc::LlmModel {
                id: "nothing-here".into(),
                label: "cide test".into(),
                context: 0,
                output: 0,
            }],
        }],
        pools: Vec::new(),
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cide-real-mimo-{tag}-{}", SessionId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mimo() -> Option<PathBuf> {
    cide_core::toolchain::which("mimo")
}

/// The spec, run to completion as a plain child rather than in a PTY, so stdout is pure JSONL.
fn run(spec: &SpawnSpec, deadline: Duration) -> (bool, String, String) {
    let binary = mimo().expect("mimo on PATH");
    let mut command = std::process::Command::new(&spec.program);
    command.args(&spec.args).current_dir(&spec.cwd);
    for (k, v) in &spec.env {
        command.env(k, v);
    }
    for k in &spec.env_remove {
        command.env_remove(k);
    }
    let bin_dir = vec![binary.parent().unwrap().to_path_buf()];
    let filtered = run_filter_with(command, None, deadline, &bin_dir).expect("mimo ran");
    (
        filtered.ok,
        String::from_utf8_lossy(&filtered.stdout).into_owned(),
        filtered.stderr,
    )
}

/// The document cide hands over registers the role, with its brief verbatim. If this fails,
/// mimo has stopped reading `MIMOCODE_CONFIG_CONTENT` and every run is silently its default
/// agent. Spends nothing.
#[test]
#[ignore = "runs the real mimo; spends nothing"]
fn the_inline_role_is_registered_with_its_brief() {
    if mimo().is_none() {
        eprintln!("no `mimo` on PATH; skipping");
        return;
    }
    let dir = temp_dir("role");
    let agent = role("PROBE BRIEF 7f3a: answer in one word.");
    let spawned = MimoHarness
        .spawn_spec(&plan(&agent, &dir, "say ok"))
        .expect("spawnable");
    let document = spawned
        .spec
        .env
        .iter()
        .find(|(key, _)| key == "MIMOCODE_CONFIG_CONTENT")
        .map(|(_, value)| value.clone())
        .expect("the document");

    let mut command = std::process::Command::new(mimo().unwrap());
    command.args(["debug", "agent", "probe"]).current_dir(&dir);
    command.env("MIMOCODE_CONFIG_CONTENT", document);
    command.env("NO_COLOR", "1");
    let out = run_filter_with(command, None, Duration::from_secs(60), &[]).expect("mimo ran");
    let printed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("`debug agent` prints JSON");
    assert_eq!(printed["name"], "probe", "{printed}");
    assert_eq!(printed["mode"], "primary", "{printed}");
    assert!(
        printed["prompt"]
            .as_str()
            .is_some_and(|prompt| prompt.starts_with("PROBE BRIEF 7f3a")),
        "{printed}"
    );
}

/// A dead pool candidate is reported, classified, and reported **fast** — the whole reason
/// `pool_retry` exists. Without the bounded `retry` block this child prints nothing for fifteen
/// minutes. Spends nothing.
#[test]
#[ignore = "runs the real mimo; spends nothing"]
fn a_dead_pool_candidate_is_classified_within_seconds() {
    if mimo().is_none() {
        eprintln!("no `mimo` on PATH; skipping");
        return;
    }
    let dir = temp_dir("dead");
    let agent = role("Answer in one word.");
    let mut plan = plan(&agent, &dir, "say ok");
    plan.llm = dead_llm();
    plan.choice = Some(cide_ipc::PoolChoice {
        pool: "test".into(),
        index: 0,
        entry: cide_ipc::PoolEntry {
            provider: "cide-test-dead".into(),
            model: "nothing-here".into(),
            variant: String::new(),
        },
    });
    let spawned = MimoHarness.spawn_spec(&plan).expect("spawnable");

    let started = std::time::Instant::now();
    let (ok, stdout, stderr) = run(&spawned.spec, Duration::from_secs(120));
    let classified: Vec<_> = stdout.lines().filter_map(failover).collect();
    assert_eq!(
        classified.first(),
        Some(&FailoverReason::Unreachable),
        "a closed port must classify as unreachable.\nstdout:\n{stdout}\nstderr tail:\n{}",
        stderr
            .chars()
            .rev()
            .take(600)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    );
    assert!(
        started.elapsed() < Duration::from_secs(90),
        "the bounded retry must end the child: {:?}",
        started.elapsed()
    );
    // The divergence `failure_exits_zero` exists for. If this starts failing because mimo now
    // exits non-zero, the gate still works — flip the flavour's flag and delete this line.
    assert!(ok, "mimo 0.1.15 exits 0 after a fatal provider error");
    assert!(MimoHarness.failure_exits_zero());
}

/// The Test button's road, on the flavour a mimo-only machine gets: a dead model is a sentence
/// in seconds, not a spinner for fifteen minutes. Spends nothing.
#[test]
#[ignore = "runs the real mimo; spends nothing"]
fn the_model_test_refuses_a_dead_endpoint_quickly() {
    if mimo().is_none() {
        eprintln!("no `mimo` on PATH; skipping");
        return;
    }
    let dir = temp_dir("test-model");
    let answer = test_model(
        &MIMO_CLI,
        Some(&dir),
        &dead_llm(),
        "cide-test-dead/nothing-here",
    );
    let sentence = answer.expect_err("nothing is listening");
    assert!(sentence.contains("APIError"), "{sentence}");
}

/// One real turn and one follow-up in the same `ses_…`: the stream moves the run, the id is
/// captured from the first line, a step's spend is read, and `--session` continues the
/// conversation. Spends a turn and a follow-up.
#[test]
#[ignore = "runs the real mimo and spends two small turns of the user's quota"]
fn a_real_turn_streams_and_continues() {
    if mimo().is_none() {
        eprintln!("no `mimo` on PATH; skipping");
        return;
    }
    let dir = temp_dir("turn");
    let mut agent = role("You are a probe. Reply with exactly one lowercase word.");
    agent.def.model =
        Some(std::env::var("CIDE_MIMO_MODEL").unwrap_or_else(|_| "xiaomi/mimo-v2.6-flash".into()));
    let first = plan(&agent, &dir, "Reply with the word: apple");
    let spawned = MimoHarness.spawn_spec(&first).expect("spawnable");
    let cide_agents::SessionBinding::Harness { capture, .. } = spawned.binding else {
        panic!("mimo mints its own id");
    };

    let (ok, stdout, stderr) = run(&spawned.spec, Duration::from_secs(180));
    assert!(ok, "the turn failed:\n{stderr}");
    let session = stdout
        .lines()
        .find_map(capture)
        .expect("a session id on the first line");
    assert!(session.starts_with("ses_"), "{session}");

    let mut state = RunState::Starting;
    for line in stdout.lines() {
        if let Some(next) = MimoHarness.observe(state.clone(), Observation::Line(line)) {
            state = next;
        }
    }
    assert_eq!(
        state,
        RunState::Running,
        "the stream moved the run:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| MimoHarness.usage(line).is_some()),
        "a step reported its spend:\n{stdout}"
    );

    let follow = MimoHarness
        .respawn_spec(
            &plan(&agent, &dir, "Now reply with the word: pear"),
            &session,
        )
        .expect("a follow-up");
    let (ok, stdout, stderr) = run(&follow.spec, Duration::from_secs(180));
    assert!(ok, "the follow-up failed:\n{stderr}");
    assert_eq!(
        stdout.lines().find_map(capture).as_deref(),
        Some(session.as_str()),
        "the follow-up continued the same conversation"
    );
}
