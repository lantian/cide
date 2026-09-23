//! The one fact in M79 that only a real CLI can settle: **what plan mode actually ends at, and
//! whether cide can answer it.**
//!
//! Everything else in the auto-accept road is a pure table with a test beside it —
//! `cide_claude::plan::approval` and its negative corpus, `cide_claude::permission::parse`,
//! `cide_app::spinner::should_spin`. This is a claim about somebody else's TUI, and a claim
//! about a TUI is measured or it is a guess.
//!
//! # What this test already found, and why the design changed
//!
//! The first cut approved the plan through a **hook**: `cide-hook` answering `PreToolUse` for
//! `ExitPlanMode` with `permissionDecision: "allow"`, on a child marked by an environment
//! variable. That is the exact way to do it — it names one tool, reads no screen, has no timing
//! — and against 2.1.278 the model planned and the approval prompt drew anyway:
//!
//! ```text
//! Claude has written up a plan and is ready to execute. Would you like to proceed?
//!  ❯ 1. Yes, and use auto mode
//!    2. Yes, manually approve edits
//!    3. Tell Claude what to change
//! ```
//!
//! The reason is structural and is in the shipped binary: `ExitPlanMode` carries its own
//! `checkPermissions`, returning `behavior: "ask"` unconditionally outside the teammate path,
//! and a `requiresUserInteraction()` that answers `true`. A hook decision does not override a
//! tool that declares it needs a human. So the hook road was removed rather than left inert, and
//! cide answers the prompt itself — `cide_claude::plan`, off the rendered grid, on a session it
//! spawned and nowhere else.
//!
//! This test now guards the road that shipped. If a later CLI moves plan exit onto the ordinary
//! permission path, a hook rule becomes possible and would be better than reading a screen;
//! `cide-hook`'s guard module carries that note where somebody would look.
//!
//! `#[ignore]`d like every test in this workspace that spawns a real harness: it needs `claude`
//! on `PATH`, an authenticated account, and it spends the user's own quota — one turn.
//!
//! ```sh
//! cargo test -p cide-agents --test real_plan -- --ignored
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cide_agents::{ClaudeHarness, Harness as _, LoadedAgent, RunPlan};
use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, Theme};
use cide_pty::PtySession;

fn role() -> LoadedAgent {
    LoadedAgent {
        def: AgentDef {
            id: AgentId("planner".into()),
            label: "Planner".into(),
            scope: cide_ipc::agents::AgentScope::Project,
            harness: cide_ipc::Harness::Claude,
            description: "Plans and then acts.".into(),
            system_prompt: "You do exactly what you are asked and nothing else.".into(),
            model: None,
            color: None,
            unavailable: None,
            max_concurrent: 1,
            worktree: false,
        },
        origin: PathBuf::from("/nowhere/.cide/agents/planner.md"),
        shadows: None,
        tools: Vec::new(),
        // The mode the spinner's child runs under. This is the whole subject of the test.
        permission_mode: Some("plan".into()),
        effort: None,
        extras: Vec::new(),
    }
}

/// What is on screen, as **plain text**.
///
/// `capture_screen` and not `full_state` (the raw retained bytes), and the difference is not
/// cosmetic: the mirror encodes
/// a run of spaces as a cursor-forward escape, so `full_state` renders *"Yes, I trust this
/// folder"* as `Yes,\x1b[CI\x1b[Ctrust\x1b[Cthis\x1b[Cfolder` and any needle containing a space
/// silently never matches. The first draft of this file searched `full_state` for
/// `"trust this folder"`, found nothing, concluded the workspace was already trusted, and
/// reported a four-minute failure of the feature — which was in fact a child sitting on an
/// unanswered safety prompt. `cide_pty::screen::capture` walks the grid cell by cell and gives
/// the characters.
fn visible(pty: &PtySession) -> String {
    pty.capture_screen()
        .lines
        .iter()
        .map(|line| {
            line.runs
                .iter()
                .map(|run| run.text.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wait until the **visible screen** contains one of `needles`. Answers which one arrived.
fn wait_for_any(pty: &PtySession, needles: &[&str], within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    loop {
        let seen = visible(pty);
        if let Some(found) = needles.iter().find(|needle| seen.contains(**needle)) {
            return Some((*found).to_string());
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Clear the CLI's first-run workspace-trust interstitial, if it draws one.
///
/// **Test setup, and it is not papering over the thing being measured.** The prompt is per
/// directory and asks whether the *folder* may be read and executed in; a real spinner run
/// happens in a project root the user opened in cide and has long since trusted, so this
/// interstitial is an artefact of the test spawning into a brand-new temp directory. Without it
/// the run never reaches a plan at all and the test reports a failure of the feature.
///
/// It is answered with an arrow and an Enter because the default option is **"No, exit"** — the
/// safe one — so a bare Enter here would end the child. That cide is willing to type at *this*
/// list and not at a permission prompt is not an inconsistency: this is a test choosing a
/// directory it created a moment earlier, not the application answering a question about
/// somebody's repository.
fn trust_the_workspace(pty: &PtySession) {
    if wait_for_any(pty, &["trust this folder"], Duration::from_secs(20)).is_none() {
        // Already trusted, or this build does not ask. Either is fine.
        return;
    }
    // Let the list finish drawing before typing at it. A key written into a TUI that is still
    // painting its first frame is read by nothing — `type_submitted_line`'s measured lesson, one
    // gesture over.
    std::thread::sleep(Duration::from_millis(800));

    // Down to "Yes, I trust this folder", then confirm.
    //
    // **Which byte sequence Down *is* depends on the mode the program has asked for**: in
    // application-cursor mode (`DECCKM`, `CSI ?1h`) it is `ESC O B`, and in normal mode
    // `ESC [ B`. `ui/src/terminal/keys.ts` branches on exactly this for every arrow, and a test
    // that hardcoded one of them sent a sequence the TUI ignored — which looked identical to the
    // prompt not being answerable at all. The mirror knows which mode is in force, so it is
    // asked rather than guessed.
    let down: &[u8] = if pty.capture_screen().info.app_cursor {
        b"\x1bOB"
    } else {
        b"\x1b[B"
    };
    pty.write(down.to_vec());
    std::thread::sleep(Duration::from_millis(400));
    pty.write(b"\r".to_vec());

    // Assert it cleared rather than assuming. A prompt still on screen here means the answer
    // did not land, and every later assertion would be about a child that never started — which
    // is exactly how the first draft of this test spent four minutes blaming the feature.
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if !visible(pty).contains("trust this folder") {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!(
        "the workspace-trust prompt would not clear:\n{}",
        visible(pty)
    );
}

/// The shipped plan prompt is readable, cide recognises it, and answering it ends plan mode.
///
/// Three claims in one turn, because a turn costs the user's quota and they are only meaningful
/// together — a prompt cide can parse but not answer is as useless as one it cannot parse.
///
/// The prompt asks for something **impossible without leaving plan mode** — writing a file — so
/// the final assertion cannot be satisfied by a model that merely said it would proceed.
///
/// What this deliberately does *not* exercise is the `AwaitingPermission` gate, which needs a
/// hook server and an `AppHandle` that a `cide-agents` test has neither of. That gate is
/// `permission::parse`'s own first rule and is covered by its unit tests; what only a real CLI
/// can answer is whether the grid it draws parses at all, and whether the digit works.
#[test]
#[ignore = "spawns the real claude and spends the user's quota"]
fn a_plan_is_read_off_the_screen_and_answering_it_leaves_plan_mode() {
    let dir = std::env::temp_dir().join(format!("cide-real-plan-{}", SessionId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("proof.txt");

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
        prompt: "Make a one-step plan to create a file called proof.txt in this directory \
                 containing the word done, then carry it out."
            .into(),
        // No hooks: the decision under test is cide's own, read off the screen. An earlier cut
        // put `cide-hook` here because the approval was the hook's; the module header says why
        // that road is gone.
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
        // The role names `plan`, which wins over this — `claude.rs`'s rule. Stated so the test
        // reads as what the spinner does rather than as a coincidence.
        unattended: cide_agents::config::Unattended::Bypass,
    };

    let spawn = ClaudeHarness.spawn_spec(&plan).expect("spawnable");
    let opening = spawn.opening.clone().expect("claude is typed into");
    let pty = PtySession::spawn(spawn.spec).expect("spawn claude");

    trust_the_workspace(&pty);

    // The opening prompt, the way the app delivers it: text, a beat, then the Enter alone.
    let (text, _) = opening.split_at(opening.len() - 1);
    pty.write(text.to_vec());
    std::thread::sleep(Duration::from_millis(400));
    pty.write(b"\r".to_vec());

    // ---- 1. the prompt arrives, and cide can read it -------------------------------------
    //
    // Generous: a plan-mode turn is a survey of the directory, then a plan.
    let answer = {
        let deadline = Instant::now() + Duration::from_secs(300);
        let mut answer = None;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
            // `AwaitingPermission` is asserted rather than observed — see the doc above.
            let screen = pty.capture_screen();
            let Some(prompt) =
                cide_claude::permission::parse(&screen, cide_ipc::SessionState::AwaitingPermission)
            else {
                continue;
            };
            if let Some(number) = cide_claude::plan::approval(&prompt) {
                // The claim is that the *right* option was found by its words, so the label is
                // asserted rather than just the number: a parser that answered "3. Tell Claude
                // what to change" would satisfy a bare `is_some()` and stall the run for ever.
                let label = prompt
                    .options
                    .iter()
                    .find(|option| option.number == number)
                    .map(|option| option.label.to_lowercase())
                    .unwrap_or_default();
                assert!(
                    label.starts_with("yes"),
                    "the plan was answered with {label:?}, which does not proceed"
                );
                answer = Some(number);
                break;
            }
        }
        answer
    };

    let Some(number) = answer else {
        // Enough to tell the three ways this fails apart without spending another turn: no
        // prompt at all, a prompt `permission::parse` refused, or one it read and
        // `plan::approval` did not recognise. The numbered lines are counted because the
        // commonest refusal is *ambiguity* — the parser takes a second numbered block on the
        // screen as a reason to refuse, and a plan rendered above its own approval is exactly
        // where a second one comes from.
        let screen = pty.capture_screen();
        let numbered: Vec<String> = screen
            .lines
            .iter()
            .map(|line| {
                line.runs
                    .iter()
                    .map(|run| run.text.as_str())
                    .collect::<String>()
            })
            .enumerate()
            .filter(|(_, text)| {
                let t = text
                    .trim_start()
                    .trim_start_matches(['❯', '>'])
                    .trim_start();
                t.chars().next().is_some_and(|c| c.is_ascii_digit())
                    && t.get(1..2).is_some_and(|sep| sep == "." || sep == ")")
            })
            .map(|(row, text)| format!("  row {row}: {}", text.trim()))
            .collect();
        let parsed =
            cide_claude::permission::parse(&screen, cide_ipc::SessionState::AwaitingPermission);
        let _ = std::fs::remove_dir_all(&dir);
        panic!(
            "no plan prompt cide could recognise appeared.\n\
             rows: {}\n\
             permission::parse: {}\n\
             numbered lines on screen ({}):\n{}\n---\n{}",
            screen.info.rows,
            match &parsed {
                Some(prompt) => format!(
                    "read it — question {:?}, options {:?}; plan::approval said no",
                    prompt.question,
                    prompt
                        .options
                        .iter()
                        .map(|o| format!("{}. {}", o.number, o.label))
                        .collect::<Vec<_>>()
                ),
                None => "refused the screen".to_string(),
            },
            numbered.len(),
            numbered.join("\n"),
            visible(&pty)
        );
    };

    // ---- 2. answering it actually leaves plan mode ---------------------------------------
    //
    // Exactly as `claude_tab::watch_for_plan` writes it: move the highlight to the option and
    // confirm, paced. The digit alone was measured to do nothing here —
    // `cide_claude::plan::keystrokes` carries that finding — and the arrow's spelling follows
    // the cursor mode the program asked for.
    let screen = pty.capture_screen();
    let selected =
        cide_claude::permission::parse(&screen, cide_ipc::SessionState::AwaitingPermission)
            .and_then(|prompt| prompt.selected);
    for key in cide_claude::plan::keystrokes(selected, number, screen.info.app_cursor) {
        pty.write(key);
        std::thread::sleep(Duration::from_millis(200));
    }

    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if target.is_file() {
            let body = std::fs::read_to_string(&target).unwrap_or_default();
            assert!(
                body.to_lowercase().contains("done"),
                "the file was written but says {body:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let seen = visible(&pty);
    let _ = std::fs::remove_dir_all(&dir);
    panic!(
        "option {number} was moved to and confirmed, and plan mode did not end — the keys were \
         read by something else, or leaving plan mode now needs more than an approval.\n---\n{seen}"
    );
}
