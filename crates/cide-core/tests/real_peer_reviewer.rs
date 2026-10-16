//! Installed harness audits using production policy, isolated state and local fake
//! model endpoints. No account credentials or paid model calls are used.
use cide_core::peer_chat::{self, reviewer};
use serde_json::json;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn audit(harness: &str) {
    let scratch = reviewer::Scratch::new().unwrap();
    let resumed_scratch = reviewer::Scratch::new().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let policy = json!({
        "scratch":scratch.path(),
        "env":scratch.env(),
        "instructions":format!("{} {}", peer_chat::instructions(cide_ipc::ConsoleHarness::Claude, true), scratch.instructions()),
        "claudeArgs":reviewer::claude_args(&[]),
        "codexArgs":scratch.codex_args(),
        "opencodePermissions":reviewer::opencode_permissions(true),
        "opencodeWebsearch":{"provider":reviewer::OPENCODE_SEARCH_PROVIDER},
        "resumedScratch":resumed_scratch.path(),
        "resumedEnv":resumed_scratch.env(),
        "resumedInstructions":format!("{} {}", peer_chat::instructions(cide_ipc::ConsoleHarness::Claude, true), resumed_scratch.instructions()),
    });
    let mut child = Command::new("python3")
        .arg(root.join("scripts/audit-peer-reviewer-tools.py"))
        .arg(harness)
        .current_dir(root)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(policy.to_string().as_bytes())
        .unwrap();
    assert!(
        child.wait().unwrap().success(),
        "{harness} reviewer audit failed"
    );
}

#[test]
#[ignore = "installed OpenCode, local sockets, public web fetch/search; no paid model calls"]
fn real_opencode_reviewer_analysis_and_web() {
    audit("opencode");
}

#[test]
#[ignore = "installed Claude, local sockets and public web fetch; no paid model calls"]
fn real_claude_reviewer_analysis_and_web() {
    audit("claude");
}

#[test]
#[ignore = "installed Codex and OS sandbox support; no model calls"]
fn real_codex_reviewer_scratch_without_project_writes() {
    audit("codex");
}
