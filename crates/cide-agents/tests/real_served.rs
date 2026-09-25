//! An opencode-shaped run behind its own server, against the real binaries. (M104 follow-up)
//!
//! `#[ignore]`d because it spawns the real CLIs, and — like `real_opencode.rs` — it **spends
//! nothing**: the role's model is a provider on a local port that accepts connections and never
//! answers, so no model is ever reached and the turn simply stays busy for as long as the test
//! looks at it.
//!
//! ```sh
//! cargo test -p cide-agents --test real_served -- --ignored --nocapture
//! ```
//!
//! What only the real binaries can confirm, and what the whole feature stands on:
//!
//! * `serve` comes up on the port and password [`Flavor::serve_spec`] hands it, and refuses a
//!   request without the password;
//! * a turn built by `spawn_spec` with `RunPlan::server` set is a client of that server — its
//!   session appears there and is `busy`;
//! * `attach --session`, built by [`Flavor::attach_spec`], draws **that** conversation in the TUI.
//!
//! A release that renamed `--attach`, moved the health route or changed how `attach` picks its
//! session would break "open a subagent and get the real opencode" with every unit test green.

use std::io::Read as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cide_agents::harness::opencode::{Flavor, MIMO_CLI, OPENCODE_CLI};
use cide_agents::{LoadedAgent, MimoHarness, OpencodeHarness, RunPlan, RunServer};
use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, Theme};
use cide_pty::SpawnSpec;

/// A provider whose endpoint accepts and never replies: every turn against it stays in flight.
fn hanging_provider() -> (cide_ipc::LlmSettings, TcpListener) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let llm = cide_ipc::LlmSettings {
        providers: vec![cide_ipc::LlmProvider::Custom {
            id: "cide-test-hang".into(),
            label: "cide test (never answers)".into(),
            enabled: true,
            npm: "@ai-sdk/openai-compatible".into(),
            base_url: format!("http://127.0.0.1:{port}/v1"),
            api_key: "not-a-key".into(),
            models: vec![cide_ipc::LlmModel {
                id: "m".into(),
                label: "cide test".into(),
                context: 0,
                output: 0,
            }],
        }],
        pools: Vec::new(),
    };
    (llm, listener)
}

fn role(harness: cide_ipc::Harness) -> LoadedAgent {
    LoadedAgent {
        def: AgentDef {
            id: AgentId("probe".into()),
            label: "Probe".into(),
            scope: cide_ipc::agents::AgentScope::Project,
            harness,
            description: "Answers one word.".into(),
            system_prompt: "Answer with one word.".into(),
            // The hanging provider, named explicitly: without it the CLI would fall back to the
            // user's own default model, which is the one thing this test must never reach.
            model: Some("cide-test-hang/m".into()),
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

fn plan<'a>(
    agent: &'a LoadedAgent,
    cwd: &Path,
    llm: cide_ipc::LlmSettings,
    server: RunServer,
) -> RunPlan<'a> {
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
        prompt: "cide served probe".into(),
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
        llm,
        choice: None,
        harness: agent.def.harness,
        unattended: cide_agents::config::Unattended::Bypass,
        tracker_paragraphs: false,
        server: Some(server),
    }
}

/// A `SpawnSpec` as a plain process — these children need no terminal of their own.
fn command(spec: &SpawnSpec) -> Command {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(spec.env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// `curl`, with the server's credential: one GET, the body, or `None` on a non-2xx.
fn get(url: &str, credential: Option<(&str, &str)>) -> Option<String> {
    let mut curl = Command::new("curl");
    curl.args(["-sf", "-m", "2"]);
    if let Some((user, password)) = credential {
        curl.args(["-u", &format!("{user}:{password}")]);
    }
    let out = curl.arg(url).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn until<T>(within: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(found) = probe() {
            return Some(found);
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

struct Killed(Vec<Child>);

impl Drop for Killed {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn served(flavor: &'static Flavor, turn: &dyn cide_agents::Harness) {
    if cide_core::toolchain::which(flavor.program()).is_none() {
        eprintln!("no `{}` on PATH; skipping", flavor.program());
        return;
    }
    if cide_core::toolchain::which("curl").is_none()
        || cide_core::toolchain::which("script").is_none()
    {
        eprintln!("needs `curl` and `script`; skipping");
        return;
    }
    let cwd = std::env::temp_dir().join(format!("cide-real-served-{}", SessionId::new()));
    std::fs::create_dir_all(&cwd).expect("cwd");
    let (llm, _hang) = hanging_provider();
    let port = TcpListener::bind(("127.0.0.1", 0))
        .expect("port")
        .local_addr()
        .expect("addr")
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let password = SessionId::new().to_string();
    let user = flavor.server_user();
    let credential = Some((user.as_str(), password.as_str()));
    let agent = role(flavor.kind);
    let plan = plan(
        &agent,
        &cwd,
        llm,
        RunServer {
            url: url.clone(),
            password: password.clone(),
        },
    );
    let mut children = Killed(Vec::new());

    // The server, and its guard.
    let server = flavor
        .serve_spec(&plan, port, &password)
        .expect("a server spec");
    children
        .0
        .push(command(&server).spawn().expect("spawn the server"));
    until(Duration::from_secs(15), || {
        get(&format!("{url}/global/health"), credential)
    })
    .expect("the server answers its health route with the password");
    assert_eq!(
        get(&format!("{url}/global/health"), None),
        None,
        "and refuses it without"
    );

    // A turn, as a client of it: its session shows up there, busy.
    let spec = turn.spawn_spec(&plan).expect("a turn").spec;
    assert!(spec.args.iter().any(|a| a == "--attach"), "{:?}", spec.args);
    children
        .0
        .push(command(&spec).spawn().expect("spawn the turn"));
    let dir = cwd.to_string_lossy();
    let session = until(Duration::from_secs(20), || {
        let status = get(&format!("{url}/session/status?directory={dir}"), credential)?;
        let status: serde_json::Value = serde_json::from_str(&status).ok()?;
        status
            .as_object()?
            .iter()
            .find(|(_, state)| state["type"] == "busy")
            .map(|(id, _)| id.clone())
    })
    .expect("the attached turn's session is busy on the server");
    eprintln!("{} served {session} on {url}", flavor.program());

    // The pane's child: the full TUI, attached to that session, drawing its first message.
    let attach = flavor.attach_spec(
        &cide_ipc::HarnessSession {
            harness: flavor.kind,
            id: session,
            cwd: cwd.clone(),
        },
        &url,
    );
    let line = std::iter::once(attach.program.clone())
        .chain(attach.args.iter().cloned())
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut tui = Command::new("script")
        .args(["-qfc", &format!("timeout -s KILL 10 {line}"), "/dev/null"])
        .env(flavor.password_env(), &password)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the attached TUI");
    let mut screen = Vec::new();
    tui.stdout
        .take()
        .expect("stdout")
        .read_to_end(&mut screen)
        .expect("read the TUI");
    let _ = tui.wait();
    let screen = String::from_utf8_lossy(&screen);
    assert!(
        screen.contains("cide served probe"),
        "the attached TUI draws the run's conversation; it drew:\n{}",
        screen.chars().take(2000).collect::<String>()
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
#[ignore = "spawns the real opencode; spends nothing"]
fn a_real_opencode_run_is_served_and_its_tui_attaches() {
    served(&OPENCODE_CLI, &OpencodeHarness);
}

#[test]
#[ignore = "spawns the real mimo; spends nothing"]
fn a_real_mimo_run_is_served_and_its_tui_attaches() {
    served(&MIMO_CLI, &MimoHarness);
}
