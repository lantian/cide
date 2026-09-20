//! The one fact in M67 that only a real CLI can settle: **Esc ends a turn in flight, and a line
//! typed after it submits as the next turn rather than being eaten.**
//!
//! Every other rule in the graceful-stop road is a pure table with a test beside it —
//! `stop_route`, `wind_down_step`, `holds_its_checkout`, `wind_down_prompt`, `epitaph`. This one
//! is a claim about somebody else's TUI, and a claim about a TUI is measured or it is a guess.
//! `cide_app::agents::stop` writes the interrupt bytes and then hands the line to
//! `type_submitted_line`; if the CLI swallows what arrives while it is tearing a turn down, the
//! wind-down is never delivered, no state edge ever arrives, and every graceful stop silently
//! degrades to a kill after the grace — with nothing in any log to say so.
//!
//! `#[ignore]`d like every test in this workspace that spawns a real harness: it needs the
//! binary on `PATH`, an authenticated account, and it spends the user's own quota — two turns.
//!
//! ```sh
//! cargo test -p cide-agents --test real_winddown -- --ignored
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cide_agents::{ClaudeHarness, Harness as _, LoadedAgent, RunPlan};
use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, Theme};
use cide_pty::PtySession;

fn role() -> LoadedAgent {
    LoadedAgent {
        def: AgentDef {
            id: AgentId("probe".into()),
            label: "Probe".into(),
            scope: cide_ipc::agents::AgentScope::Project,
            harness: cide_ipc::Harness::Claude,
            description: "Counts slowly.".into(),
            system_prompt: "You do exactly what you are asked and nothing else.".into(),
            model: None,
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

/// Everything the mirror has retained, as text. `full_state` rather than `screen_state`,
/// because a 400-line count scrolls the answer off the viewport almost immediately and the
/// question here is whether a turn *happened*, not what is on screen now.
fn transcript(pty: &PtySession) -> String {
    String::from_utf8_lossy(&pty.full_state()).to_string()
}

/// Wait until the mirror contains `needle`, or give up. Answers whether it arrived.
fn wait_for(pty: &PtySession, needle: &str, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if transcript(pty).contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Esc ends the turn in flight, and the wind-down line typed after it is taken up as a new one.
///
/// The shape is the real stop road's, in order: a long turn is started, Esc is written straight
/// at the PTY (as `AgentRegistry::ask_to_wind_down` does, *not* through `type_submitted_line`,
/// which owes an Enter and would submit an empty composer), a beat passes, and the line goes in.
/// The assertion is that the second answer arrives at all — which it cannot if the Esc was
/// ignored (the first turn would still be running) or if the line was swallowed with it.
#[test]
#[ignore = "spawns the real claude and spends the user's quota"]
fn a_real_turn_is_interrupted_by_esc_and_the_next_line_is_taken_up() {
    let dir = std::env::temp_dir().join(format!("cide-real-winddown-{}", SessionId::new()));
    std::fs::create_dir_all(&dir).unwrap();

    let agent = role();
    let plan = RunPlan {
        run: RunId::new(),
        session: SessionId::new(),
        agent: &agent,
        cwd: dir.clone(),
        project: ProjectId::new(),
        task: None,
        task_title: None,
        change: None,
        spec_cli: None,
        spec_apply: None,
        // Long enough that the Esc lands mid-turn rather than after it. The whole point of the
        // interrupt is the case where the agent is busy, so a fast prompt would test nothing.
        prompt: "Count from 1 to 400, one number per line, with no other words.".into(),
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
        skip_permissions: true,
    };

    let spawn = ClaudeHarness.spawn_spec(&plan).expect("spawnable");
    let opening = spawn.opening.clone().expect("claude is typed into");
    let pty = PtySession::spawn(spawn.spec).expect("spawn claude");

    // The opening prompt, the way the app delivers it: text, a beat, then the Enter alone.
    // `type_submitted_line`'s split, by hand, because this test has no `AppHandle`.
    let (text, _) = opening.split_at(opening.len() - 1);
    pty.write(text.to_vec());
    std::thread::sleep(Duration::from_millis(400));
    pty.write(b"\r".to_vec());

    assert!(
        wait_for(&pty, "10", Duration::from_secs(120)),
        "the first turn never started: {}",
        transcript(&pty)
    );

    // The interrupt, exactly as `ask_to_wind_down` writes it.
    let bytes = ClaudeHarness
        .interrupt()
        .expect("claude can be interrupted");
    pty.write(bytes);
    // The beat the app gives it. A TUI still tearing a turn down reads what arrives in the same
    // chunk as the Esc and drops it — `type_submitted_line`'s measured failure, one gesture over.
    std::thread::sleep(Duration::from_millis(600));

    pty.write(b"Reply with exactly the single word: pong".to_vec());
    std::thread::sleep(Duration::from_millis(400));
    pty.write(b"\r".to_vec());

    assert!(
        wait_for(&pty, "pong", Duration::from_secs(120)),
        "the line after the Esc was never taken up — a graceful stop would degrade to a kill \
         after the grace, with nothing anywhere to say why: {}",
        transcript(&pty)
    );

    pty.kill();
    let _ = std::fs::remove_dir_all(&dir);
}
