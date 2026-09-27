//! The facts about the opencode CLI that only the real binary can confirm. (M45, M110)
//!
//! `#[ignore]`d, like every test in this workspace that spawns a real harness — but unlike most of
//! them these spend **nothing**. Each points a provider at a port nothing is listening on, so the
//! CLI never reaches a model: there is no token to bill and no account to be logged into, only
//! `opencode` on `PATH`.
//!
//! ```sh
//! cargo test -p cide-agents --test real_opencode -- --ignored
//! ```
//!
//! # Why these exist when the classifier already has a corpus
//!
//! The corpus in `harness/opencode.rs` is captured output, and captured output cannot notice when
//! opencode *changes*. Every branch of `failover` reads a field by name — `statusCode`,
//! `isRetryable`, the error's `name` — out of a JSON object whose shape is another project's to
//! change, and the published SDK type already disagrees with the wire (the live `APIError` carries
//! a `metadata` the type does not declare). A release that renames `isRetryable` would turn every
//! unreachable provider into an unclassified death: no failover, a run that dies on its first
//! candidate, and not one test failing anywhere. These are what fail instead.
//!
//! **opencode 2.0 is that release.** It renamed all three at once — `name` became `type`, the
//! detail moved out of `data`, and `isRetryable` went — and the corpus went on passing, which is
//! precisely the hole these tests were written for. So from M110 each of them asks the installed
//! binary which command line it speaks and builds the invocation for *that* one, and the
//! configuration document travels through the same producer a run uses rather than a copy of it.

use std::path::PathBuf;

use cide_agents::FailoverReason;
use cide_agents::harness::opencode::{Generation, OPENCODE_CLI, failover, provider_config_content};
use cide_core::child_env::run_filter_with;

/// How long a turn against a dead endpoint gets. opencode 2 retries a transport failure a few
/// times before giving up, which is slower than 1.x's single refusal and still free.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// A provider pointing at a closed port, **built by the producer a run's document is built by**.
///
/// Port 1 is chosen deliberately: it is privileged, nothing binds it, and a connection attempt
/// fails immediately rather than hanging — so this is fast as well as free.
///
/// Built through `provider_config_content` rather than hand-written here: a second copy of the
/// provider block is exactly the drift these tests exist to catch, and a fixture spelling
/// `options` after a release renamed it to `settings` would fail for the wrong reason and be
/// "fixed" by editing the fixture.
fn dead_provider(generation: Generation) -> String {
    let llm = cide_ipc::LlmSettings {
        providers: vec![cide_ipc::LlmProvider::Custom {
            id: "cide-test-dead".into(),
            label: "cide test (nothing listening)".into(),
            npm: "@ai-sdk/openai-compatible".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: "not-a-key".into(),
            enabled: true,
            models: vec![cide_ipc::LlmModel {
                id: "nothing-here".into(),
                label: "cide test".into(),
                ..Default::default()
            }],
        }],
        ..Default::default()
    };
    provider_config_content(&OPENCODE_CLI, &llm, generation).expect("a document")
}

/// The installed binary and the command line it speaks, or `None` with a printed reason.
fn probed() -> Option<(PathBuf, Generation)> {
    let binary = cide_core::toolchain::which("opencode")?;
    Some((binary, OPENCODE_CLI.probe_cli_flags().generation))
}

/// `opencode run …` for this generation, with the document in the environment.
///
/// `--standalone` on 2.0 is not a tuning knob: without it the child is a client of the
/// machine-wide background service, which was started with another environment, and the document
/// below is invisible to it. See `Generation`.
fn run_command(
    binary: &std::path::Path,
    generation: Generation,
    model: &str,
    document: &str,
) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command.arg("run");
    if matches!(generation, Generation::V2) {
        command.arg("--standalone");
    }
    command.args(["--model", model, "--format", "json"]);
    command.arg("say ok");
    command.env("NO_COLOR", "1");
    command.env("OPENCODE_CONFIG_CONTENT", document);
    command
}

fn bin_dir(binary: &std::path::Path) -> Vec<PathBuf> {
    binary
        .parent()
        .map(|d| vec![d.to_path_buf()])
        .unwrap_or_default()
}

/// The load-bearing one: a real unreachable provider still classifies as `Unreachable`.
///
/// If this fails and the corpus still passes, opencode has changed the shape of an error and the
/// classifier is reading a field that is no longer there.
#[test]
#[ignore = "spawns the real opencode; spends nothing"]
fn a_real_unreachable_provider_is_classified() {
    let Some((binary, generation)) = probed() else {
        eprintln!("no `opencode` on PATH; skipping");
        return;
    };
    let command = run_command(
        &binary,
        generation,
        "cide-test-dead/nothing-here",
        &dead_provider(generation),
    );
    let out = run_filter_with(command, None, DEADLINE, &bin_dir(&binary)).expect("opencode ran");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let classified: Vec<_> = stdout.lines().filter_map(failover).collect();
    assert_eq!(
        classified.first(),
        Some(&FailoverReason::Unreachable),
        "a provider on a closed port must classify as unreachable ({generation:?}).\n\
         If this is empty, opencode has changed its error shape and `failover` is reading a field \
         that no longer exists — which would silently stop every pool from failing over.\n\
         stdout was:\n{stdout}"
    );
    assert!(
        !out.ok,
        "and the child exits non-zero, which is what `plan_failover` guards on"
    );
}

/// A model id that does not exist classifies as `Rejected`, so the run moves on to the next entry
/// and cide names the broken one. (This was the negative half until t-1090, when the user ruled
/// that a pool's fallbacks are for exactly this.)
#[test]
#[ignore = "spawns the real opencode; spends nothing"]
fn a_real_typo_fails_over_as_rejected() {
    let Some((binary, generation)) = probed() else {
        eprintln!("no `opencode` on PATH; skipping");
        return;
    };
    let command = run_command(
        &binary,
        generation,
        "cide-test-dead/no-such-model-anywhere",
        &dead_provider(generation),
    );
    let out = run_filter_with(command, None, DEADLINE, &bin_dir(&binary)).expect("opencode ran");
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Reversed at the user's word (t-1090): a model id that does not exist is the entry's
    // configuration, and the run moves on to the next entry while cide names the broken one.
    assert_eq!(
        stdout.lines().find_map(failover),
        Some(cide_agents::FailoverReason::Rejected),
        "a missing model must fail over as Rejected.\nstdout was:\n{stdout}"
    );
}

/// **The negative that makes `--standalone` load-bearing.** (M110)
///
/// On opencode 2 a `run` that owns neither a private server nor a `--server` is a client of the
/// machine-wide background service, which cide did not start and whose environment does not carry
/// `OPENCODE_CONFIG_CONTENT`. So the inline role does not exist there, and the turn says so.
///
/// This asserts both halves, because only the pair is a proof: with the flag the role resolves,
/// without it the *same* invocation is refused by name. A refactor that dropped the flag would
/// otherwise leave runs that quietly used somebody else's agent, in somebody else's directory,
/// without cide's tools — which is the failure `docs/worktree-isolation.md` is written against.
///
/// Free: neither half reaches a model. The refusal happens before any provider call, and the
/// accepted half is pointed at the dead endpoint like every other test here.
#[test]
#[ignore = "spawns the real opencode; spends nothing"]
fn a_v2_run_without_its_own_server_does_not_see_cides_role() {
    let Some((binary, generation)) = probed() else {
        eprintln!("no `opencode` on PATH; skipping");
        return;
    };
    if matches!(generation, Generation::V1) {
        eprintln!("this opencode is 1.x, which has no background service; skipping");
        return;
    }

    // The document a run carries: cide's provider block plus an inline role that exists nowhere
    // else on the machine, which is what makes the lookup below decisive.
    let mut document: serde_json::Value =
        serde_json::from_str(&dead_provider(generation)).expect("json");
    document["agents"] = serde_json::json!({
        "cide-test-role": { "description": "cide test", "mode": "primary", "prompt": "You are a test." }
    });
    let document = document.to_string();

    let attached = |standalone: bool| {
        let mut command = std::process::Command::new(&binary);
        command.arg("run");
        if standalone {
            command.arg("--standalone");
        }
        command.args([
            "--agent",
            "cide-test-role",
            "--model",
            "cide-test-dead/nothing-here",
            "--format",
            "json",
        ]);
        command.arg("say ok");
        command.env("NO_COLOR", "1");
        command.env("OPENCODE_CONFIG_CONTENT", &document);
        let out =
            run_filter_with(command, None, DEADLINE, &bin_dir(&binary)).expect("opencode ran");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let refused = attached(false);
    assert!(
        refused.contains("Agent not found"),
        "without `--standalone` the run is a client of the machine's background service, which \
         never saw cide's document, so the inline role must be missing and said to be.\n\
         stdout was:\n{refused}"
    );

    let accepted = attached(true);
    assert!(
        !accepted.contains("Agent not found"),
        "with `--standalone` the child owns its server and reads cide's document, so the inline \
         role resolves.\nstdout was:\n{accepted}"
    );
}
