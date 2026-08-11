//! Does the installed `claude` accept the argument shapes [`cide_claude::conversation`] builds?
//!
//! # Why this test exists
//!
//! Because the last answer to that question was a comment. `cmd/session.rs` recorded that
//! `--resume <parent> --session-id <ours>` had been *verified against 2.1.226*, and it had
//! been — then 2.1.227 shipped and answered
//!
//! > `Error: --session-id can only be used with --continue or --resume if --fork-session is
//! > also specified.`
//!
//! which is every Resume click in the app failing before a pane ever appears. A comment cannot
//! be re-run. This can: one command, the next time a pane will not start.
//!
//! This is the general lesson as much as the one flag. cide asserts a great deal about an
//! undocumented, self-updating CLI — the IDE protocol's tool names and lockfile shape
//! (`cide_ide_mcp::protocol`, `SUPPORTED_CLI`), the hook event vocabulary and settings schema
//! (`cide_claude::hook`, `cide_claude::settings`), the headless `--print --output-format json`
//! envelope (`cide_claude::headless`) — and each of those already has an `#[ignore]`d test that
//! puts the real binary behind it. The argument shapes were the one claim with no such test.
//! Now they have one.
//!
//! # Why it costs nothing
//!
//! It never reaches the network and never spends a token. Every case resumes a uuid that
//! cannot exist, so argument validation either rejects the command line — which is the thing
//! under test — or the CLI answers `No conversation found with session ID: …` and stops. The
//! two outcomes are distinguishable in the output, which is the whole assertion.
//!
//! `#[ignore]`d all the same, because it spawns the real binary and CI has no `claude`.
//!
//! Run with: `cargo test -p cide-claude -- --ignored`

use std::path::PathBuf;
use std::process::Command;

use cide_claude::conversation;
use cide_ipc::SessionId;

/// The exact prose 2.1.227 prints when `--session-id` is used where it may not be.
///
/// Matched on the distinctive clause rather than the whole sentence: the wording around it is
/// the CLI's to change, but a rejection that no longer says this is a *different* rejection and
/// deserves to fail this test rather than pass it by accident.
const REJECTION: &str = "--session-id can only be used with";

fn on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

/// Run `claude <args> --print x` in a scratch directory and return everything it said.
///
/// `--print` so the CLI never opens a TUI and never waits for input. The prompt is a single
/// character that is never sent anywhere: both branches end before a request is made.
fn says(claude: &PathBuf, args: &[String]) -> String {
    let out = Command::new(claude)
        .args(args)
        .arg("--print")
        .arg("x")
        .current_dir(std::env::temp_dir())
        .output()
        .expect("run claude");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
#[ignore = "spawns the real claude binary: needs it on PATH. Costs no tokens — every case \
            resumes a uuid that cannot exist. Run with: cargo test -p cide-claude -- --ignored"]
fn the_real_cli_accepts_every_shape_we_build() {
    let Some(claude) = on_path("claude") else {
        eprintln!("SKIP: no `claude` on PATH");
        return;
    };

    let minted = SessionId::new();
    // Valid uuids, and deliberately conversations that do not exist: the point is to get past
    // argument validation and be told so, not to resume anything.
    let parent = SessionId::new();

    for (name, (_, args)) in [
        ("fresh", conversation(minted, None, false)),
        ("resume", conversation(minted, Some(parent), false)),
        ("resume + fork", conversation(minted, Some(parent), true)),
    ] {
        let said = says(&claude, &args);
        assert!(
            !said.contains(REJECTION),
            "the CLI rejected the `{name}` shape cide builds — {args:?}\n{said}"
        );
    }
}

#[test]
#[ignore = "spawns the real claude binary: needs it on PATH. Run with: cargo test -p \
            cide-claude -- --ignored"]
fn the_combination_that_broke_resume_is_still_the_one_the_cli_refuses() {
    // The negative half, and it is not decoration. Without it, a `conversation` that quietly
    // stopped passing `--resume` at all would sail through the test above: nothing would be
    // rejected, and every Resume click would silently start a *new* conversation instead of
    // failing loudly. This pins that the CLI's rule is still the rule we are coding around, so
    // a release that relaxes it again is a visible change rather than an invisible one.
    let Some(claude) = on_path("claude") else {
        eprintln!("SKIP: no `claude` on PATH");
        return;
    };

    let said = says(
        &claude,
        &[
            "--resume".into(),
            SessionId::new().to_string(),
            "--session-id".into(),
            SessionId::new().to_string(),
        ],
    );
    assert!(
        said.contains(REJECTION),
        "`--resume <parent> --session-id <ours>` is accepted again. That is good news, but \
         `cide_claude::conversation` is written around it being refused — re-read its docs \
         before changing anything.\n{said}"
    );
}
