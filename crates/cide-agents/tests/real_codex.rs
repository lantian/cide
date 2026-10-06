//! The facts about the Codex CLI that only the real binary can confirm. (M44, rehosted M93)
//!
//! `#[ignore]`d, like every test in this workspace that spawns a real harness: they need
//! `codex` on `PATH` and a logged-in account. Two of the three spend **nothing** — `codex debug
//! prompt-input` renders what the model would be shown without calling one, and `codex mcp get`
//! reads configuration — and are worth running on every change to `harness/codex.rs`. So is the
//! `needs: [gpu]` one (M125), which runs a command in `codex sandbox` and no model, and skips
//! itself on a machine without an NVIDIA GPU; it is the one a codex update that changes how it
//! calls bwrap breaks:
//!
//! ```sh
//! cargo test -p cide-agents --test real_codex -- --ignored --skip a_real_turn
//! ```
//!
//! The other two spend the user's own quota and are run deliberately — two short turns and a
//! resume, and (M118) one `codex exec` turn proving a sandboxed task run can commit from its
//! worktree. Both are named `a_real_turn_…`, so the `--skip` above leaves them out:
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
        sandbox: Default::default(),
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
        opencode_cli: cide_ipc::OpencodeCli::default(),
        opencode_flags: None,
        llm: cide_ipc::LlmSettings::default(),
        choice: None,
        harness: agent.def.harness,
        // The project default: a bypassed sandbox, which is what a task run gets.
        unattended: cide_agents::config::Unattended::Bypass,
        tracker_paragraphs: true,
        review: false,
        server: None,
        git_dirs: Vec::new(),
        codex_git_permissions: None,
        sandbox_brief: None,
        codex_trust_root: None,
        codex_path_prepend: None,
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

/// Run `git` in `dir` with the user's own configuration kept out of it.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "cide tests")
        .env("GIT_AUTHOR_EMAIL", "tests@cide.invalid")
        .env("GIT_COMMITTER_NAME", "cide tests")
        .env("GIT_COMMITTER_EMAIL", "tests@cide.invalid")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A role that says `permission-mode: auto` runs under `--approve-for-me`, codex's
/// workspace-write sandbox, and a task run in a linked worktree can still commit — because the
/// harness adds the worktree's git dir and the common `.git` as `--add-dir`. (M118)
///
/// Selfcraft's t-572 was the failure: `index.lock: Read-only file system`, on every codex run.
/// The sandbox and `--add-dir` tokens are taken **from the harness's own argv** and handed to
/// `codex exec` (one turn, not the TUI), so this pins what cide passes rather than a copy of it.
/// Measured on 0.157.1: commits with the tokens; the same command without them is refused.
/// Spends one short turn.
#[test]
#[ignore = "spawns the real codex and spends the user's quota"]
fn a_real_turn_commits_from_a_sandboxed_worktree() {
    let root = temp_dir("commit");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["commit", "-q", "--allow-empty", "-m", "base"]);
    std::fs::create_dir_all(root.join(".cide/worktrees")).unwrap();
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "cide/probe-t-1",
            ".cide/worktrees/probe-t-1",
        ],
    );
    let checkout = root.join(".cide/worktrees/probe-t-1");
    let absolute = |p: String| {
        let p = PathBuf::from(p);
        std::fs::canonicalize(if p.is_absolute() { p } else { checkout.join(p) }).unwrap()
    };
    let dirs = vec![
        absolute(git(&checkout, &["rev-parse", "--git-dir"])),
        absolute(git(&checkout, &["rev-parse", "--git-common-dir"])),
    ];

    let mut agent = role("Do exactly what you are asked.");
    agent.permission_mode = Some("auto".into());
    let mut run = plan(&agent, &checkout, "unused");
    run.git_dirs = dirs;
    let args = CodexHarness.spawn_spec(&run).expect("spawnable").spec.args;
    assert!(args.iter().any(|a| a == "--approve-for-me"), "{args:?}");
    let mut sandbox: Vec<String> = vec!["--approve-for-me".into()];
    for pair in args.windows(2).filter(|w| w[0] == "--add-dir") {
        sandbox.extend(pair.iter().cloned());
    }
    assert_eq!(sandbox.len(), 5, "both directories are added: {args:?}");

    let before = git(&checkout, &["rev-list", "--count", "HEAD"]);
    let out = std::process::Command::new(codex())
        .current_dir(&checkout)
        .arg("exec")
        .args(&sandbox)
        .arg("-C")
        .arg(&checkout)
        .arg(
            "Run exactly this one shell command and nothing else, then reply with its output: \
             sh -c 'echo probe > probe && git add probe && git -c user.name=probe \
             -c user.email=probe@cide.invalid commit -qm probe && echo COMMITTED'",
        )
        .output()
        .expect("codex exec runs");
    let after = git(&checkout, &["rev-list", "--count", "HEAD"]);
    assert_ne!(
        before,
        after,
        "no commit landed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// What a `codex sandbox` command printed, and whether it succeeded.
fn in_codex_sandbox(dir: &Path, path: &str, script: &str) -> (bool, String, String) {
    let out = std::process::Command::new(codex())
        .current_dir(dir)
        .env("PATH", path)
        // 0.157.1's spelling: `-P` is required, and `:workspace` is the built-in workspace-write
        // profile (`workspace-write` without the colon asks the user's config for a
        // `[permissions]` table). `-C` is where the command runs.
        .args([
            "sandbox",
            "-P",
            ":workspace",
            "-c",
            "sandbox_workspace_write.network_access=true",
            "-C",
        ])
        .arg(dir)
        .args(["--", "sh", "-c", script])
        .output()
        .expect("codex sandbox runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Free, real enforcement of the exact shared profile used by worker and console launches.
/// Run deliberately: requires a Codex installation with standard workspace permissions.
#[test]
#[ignore = "runs the real Codex sandbox and permission preflight; no model calls"]
fn scoped_git_permissions_allow_commits_and_integration_but_protect_agent_config() {
    use cide_core::codex_permissions::{GitPermissions, GitProfile, PROFILE_NAME, preflight};
    let root = temp_dir("scoped-git");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let checkout = root.join("worker");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "worker",
            checkout.to_str().unwrap(),
        ],
    );
    let dirs = vec![root.join(".git"), root.join(".git/worktrees/worker")];
    let cli = cide_ipc::CodexCli::default();
    let permissions = preflight(&cli, &checkout, &dirs, &[], false, &[]);
    let GitPermissions::Applied(profile) = permissions else {
        panic!("preflight: {permissions:?}");
    };
    assert_eq!(profile, GitProfile::new(&dirs, &[], false));
    let mut agent = role("Do exactly what you are asked.");
    agent.permission_mode = Some("auto".into());
    let mut run = plan(&agent, &checkout, "unused");
    run.git_dirs = dirs.clone();
    run.codex_git_permissions = Some(GitPermissions::Applied(profile.clone()));
    let args = CodexHarness.spawn_spec(&run).unwrap().spec.args;
    assert!(
        !args
            .iter()
            .any(|a| a == "-s" || a == "--approve-for-me" || a == "--add-dir")
    );
    assert!(args.contains(&profile.config));
    // Take the profile overrides from the harness, not a second implementation of its argv.
    let mut config = overrides(&args, "permissions.");
    config.extend(overrides(&args, "default_permissions"));
    let sandbox = |cwd: &Path, scoped: bool, command: &[&str]| {
        let mut process = std::process::Command::new(codex());
        process
            .args([
                "sandbox",
                "-P",
                if scoped { PROFILE_NAME } else { ":workspace" },
                "-C",
            ])
            .arg(cwd);
        if scoped {
            process.args(&config);
        }
        process.arg("--").args(command).output().unwrap()
    };
    for cwd in [&root, &checkout] {
        let out = sandbox(
            cwd,
            false,
            &["git", "commit", "--allow-empty", "-m", "denied"],
        );
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("Read-only file system"),
            "{out:?}"
        );
        for name in [".codex", ".agents"] {
            std::fs::create_dir_all(cwd.join(name)).unwrap();
        }
        std::fs::write(cwd.join("probe"), "scoped Git write\n").unwrap();
        let message = if cwd == &root {
            "primary-scoped-git"
        } else {
            "worker-scoped-git"
        };
        for command in [
            vec!["git", "add", "probe"],
            vec!["git", "commit", "-qm", message],
        ] {
            let out = sandbox(cwd, true, &command);
            assert!(out.status.success(), "{out:?}");
        }
        for protected in [".codex/probe", ".agents/probe"] {
            let out = sandbox(cwd, true, &["touch", protected]);
            assert!(!out.status.success(), "{protected} became writable");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("Read-only file system"),
                "{out:?}"
            );
        }
    }
    let out = sandbox(
        &root,
        true,
        &["git", "merge", "--no-ff", "worker", "-m", "integration"],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        git(&root, &["rev-parse", "HEAD^2"]),
        git(&checkout, &["rev-parse", "HEAD"])
    );
    for (value, warning) in [
        ("sandbox_mode=\"workspace-write\"", true),
        ("default_permissions=\":read-only\"", false),
        ("default_permissions=\":danger-full-access\"", false),
    ] {
        let mut cli = cli.clone();
        cli.args = vec!["-c".into(), value.into()];
        let result = preflight(&cli, &checkout, &dirs, &[], false, &[]);
        assert!(result.profile().is_none(), "{result:?}");
        assert_eq!(result.warning().is_some(), warning, "{result:?}");
    }
    let _ = std::fs::remove_dir_all(root);
}

/// `needs: [gpu]` puts the real GPU inside codex's sandbox (M125): the `PATH` the harness hands a
/// `permission-mode: auto` run starts with the shim, and a real `codex sandbox` run on that
/// `PATH` sees `/dev/dri` and `/dev/nvidia0` — which the same command on the plain `PATH` does
/// not. Spends nothing: `codex sandbox` runs a command, not a model.
///
/// **This is the test a codex update breaks.** The shim depends on codex running the system
/// `bwrap` off its `PATH` with a `--dev /dev` pair (0.157.1). If codex bundles its own bwrap,
/// resolves it by absolute path, or builds `/dev` another way, the devices vanish and this fails
/// — see the assertions for which of those it looks like. Skipped, not failed, on a machine with
/// no codex, no bwrap or no NVIDIA nodes.
#[test]
#[ignore = "runs the real codex sandbox; needs codex, bwrap and an NVIDIA GPU's device nodes"]
fn the_real_codex_sandbox_sees_the_gpu_through_the_shim() {
    let missing: Vec<&str> = [
        ("codex", cide_core::toolchain::which("codex").is_none()),
        ("bwrap", cide_core::toolchain::which("bwrap").is_none()),
        ("/dev/dri", !Path::new("/dev/dri").exists()),
        ("/dev/nvidia0", !Path::new("/dev/nvidia0").exists()),
    ]
    .into_iter()
    .filter_map(|(what, absent)| absent.then_some(what))
    .collect();
    if !missing.is_empty() {
        eprintln!("skipped: this machine has no {}", missing.join(", "));
        return;
    }

    // Two directories, not one: codex skips a `PATH` bwrap that lies inside one of the sandbox's
    // writable roots (measured on 0.157.1 — a sandboxed command could rewrite it), and `-C` is
    // one. With the shim under the cwd this test measured codex's bundled bwrap and failed.
    let dir = temp_dir("gpu");
    let shim_dir = temp_dir("gpu-shim");
    cide_agents::sandbox::write_bwrap_gpu_shim(&shim_dir).expect("the shim is written");

    // The PATH as the harness composes it, not as this test would.
    let mut agent = role("unused");
    agent.permission_mode = Some("auto".into());
    agent.sandbox.needs = vec![cide_ipc::SandboxNeed::Gpu];
    let mut run = plan(&agent, &dir, "unused");
    run.codex_path_prepend = Some(shim_dir.clone());
    let spec = CodexHarness.spawn_spec(&run).expect("spawnable").spec;
    let path = spec
        .env
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.clone())
        .expect("the harness sets PATH");
    assert!(
        path.starts_with(&format!("{}:", shim_dir.display())),
        "the shim is first: {path}"
    );

    const PROBE: &str = "for n in /dev/dri /dev/nvidia0; do \
                         if [ -e \"$n\" ]; then echo \"HAS $n\"; else echo \"NO $n\"; fi; done";

    let plain = std::env::var("PATH").unwrap_or_default();
    let (ok, stdout, stderr) = in_codex_sandbox(&dir, &plain, PROBE);
    assert!(ok, "codex sandbox did not run at all:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("NO /dev/nvidia0"),
        "codex's own sandbox now has the GPU — the shim may be unnecessary; re-measure:\n{stdout}"
    );

    let (ok, stdout, stderr) = in_codex_sandbox(&dir, &path, PROBE);
    assert!(
        ok,
        "codex sandbox failed under the shim:\n{stdout}\n{stderr}"
    );
    assert!(
        !stderr.contains("cide gpu shim"),
        "the shim ran and did not recognise codex's bwrap call — codex changed how it builds /dev:\n{stderr}"
    );
    for node in ["/dev/dri", "/dev/nvidia0"] {
        assert!(
            stdout.contains(&format!("HAS {node}")),
            "{node} is not in the sandbox under the shim. If the shim printed nothing, codex no \
             longer runs `bwrap` off its PATH (bundled, or by absolute path), or the shim sits \
             inside a writable root:\n{stdout}\n{stderr}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&shim_dir).ok();
}
