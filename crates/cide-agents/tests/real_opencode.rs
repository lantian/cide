//! The facts about the opencode CLI that only the real binary can confirm. (M45)
//!
//! `#[ignore]`d, like every test in this workspace that spawns a real harness — but unlike most of
//! them these spend **nothing**. Both point a provider at a port nothing is listening on, so the
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

use std::path::PathBuf;

use cide_agents::FailoverReason;
use cide_agents::harness::opencode::failover;
use cide_core::child_env::run_filter_with;

/// A provider pointing at a closed port, in the shape `provider_members` emits.
///
/// Port 1 is chosen deliberately: it is privileged, nothing binds it, and a connection attempt
/// fails immediately rather than hanging — so this test is fast as well as free.
fn dead_provider() -> String {
    serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "provider": {
            "cide-test-dead": {
                "npm": "@ai-sdk/openai-compatible",
                "name": "cide test (nothing listening)",
                "options": { "baseURL": "http://127.0.0.1:1/v1", "apiKey": "not-a-key" },
                "models": { "nothing-here": { "name": "cide test" } }
            }
        }
    })
    .to_string()
}

fn opencode() -> Option<PathBuf> {
    cide_core::toolchain::which("opencode")
}

/// The load-bearing one: a real unreachable provider still classifies as `Unreachable`.
///
/// If this fails and the corpus still passes, opencode has changed the shape of an error and the
/// classifier is reading a field that is no longer there.
#[test]
#[ignore = "spawns the real opencode; spends nothing"]
fn a_real_unreachable_provider_is_classified() {
    let Some(binary) = opencode() else {
        eprintln!("no `opencode` on PATH; skipping");
        return;
    };
    let mut command = std::process::Command::new(&binary);
    command.args([
        "run",
        "--model",
        "cide-test-dead/nothing-here",
        "--format",
        "json",
    ]);
    command.arg("say ok");
    command.env("NO_COLOR", "1");
    command.env("OPENCODE_CONFIG_CONTENT", dead_provider());

    let bin_dir: Vec<PathBuf> = binary
        .parent()
        .map(|d| vec![d.to_path_buf()])
        .unwrap_or_default();
    let out = run_filter_with(command, None, std::time::Duration::from_secs(120), &bin_dir)
        .expect("opencode ran");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let classified: Vec<_> = stdout.lines().filter_map(failover).collect();
    assert_eq!(
        classified.first(),
        Some(&FailoverReason::Unreachable),
        "a provider on a closed port must classify as unreachable.\n\
         If this is empty, opencode has changed its error shape and `failover` is reading a field \
         that no longer exists — which would silently stop every pool from failing over.\n\
         stdout was:\n{stdout}"
    );
    assert!(
        !out.ok,
        "and the child exits non-zero, which is what `plan_failover` guards on"
    );
}

/// The negative half, and the more valuable one: a model id that does not exist must **not**
/// classify, so a typo fails loudly once instead of burning every candidate in the pool.
#[test]
#[ignore = "spawns the real opencode; spends nothing"]
fn a_real_typo_still_refuses_to_fail_over() {
    let Some(binary) = opencode() else {
        eprintln!("no `opencode` on PATH; skipping");
        return;
    };
    let mut command = std::process::Command::new(&binary);
    command.args([
        "run",
        "--model",
        "cide-test-dead/no-such-model-anywhere",
        "--format",
        "json",
    ]);
    command.arg("say ok");
    command.env("NO_COLOR", "1");
    command.env("OPENCODE_CONFIG_CONTENT", dead_provider());

    let bin_dir: Vec<PathBuf> = binary
        .parent()
        .map(|d| vec![d.to_path_buf()])
        .unwrap_or_default();
    let out = run_filter_with(command, None, std::time::Duration::from_secs(120), &bin_dir)
        .expect("opencode ran");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.lines().filter_map(failover).next().is_none(),
        "a model id that does not exist is a typo, and no other candidate in a pool fixes a \
         typo. Classifying it would spend the whole pool one turn at a time and report the last \
         candidate's error as the cause of death.\nstdout was:\n{stdout}"
    );
}
