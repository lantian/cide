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
//! It never reaches the network and never spends a token, and that takes *two* things, not
//! one. The resuming cases name a uuid that cannot exist, so the CLI stops at
//! `No conversation found with session ID: …`. The `fresh` case has no `--resume` and nothing
//! to stop it: `claude --session-id <uuid> --print x` is a perfectly good command that starts
//! a conversation and bills for it, on the user's own credentials, once per run of this test.
//! So no case is given a prompt at all — `--print` with stdin closed fails validation with
//! `Input must be provided either through stdin or as a prompt argument`, which lands *after*
//! the argument-combination check this test is about and before any request. All three
//! outcomes stay distinguishable in the output, which is the whole assertion.
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

/// Run `claude <args> --print` with no prompt in a scratch directory, and return what it said.
///
/// `--print` so the CLI never opens a TUI. **No prompt, and stdin closed**, so it never sends
/// one either: the CLI answers "Input must be provided…" and exits. That check runs after the
/// argument-combination check under test here, which is the ordering this whole test relies
/// on — and it is what keeps the `fresh` shape, the one case with no unresumable uuid to stop
/// it, from starting and billing a real conversation every time someone runs this file.
fn says(claude: &PathBuf, args: &[String]) -> String {
    let out = Command::new(claude)
        .args(args)
        .arg("--print")
        .stdin(std::process::Stdio::null())
        .current_dir(std::env::temp_dir())
        .output()
        .expect("run claude");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The two ways a run is allowed to end. Anything else and it got further than it should.
///
/// `says` gives the CLI no prompt, so a shape whose arguments are accepted must stop at one of
/// these — either the conversation it was told to resume does not exist, or there was nothing
/// to say. A run that ends any other way has gone past both and is talking to the model on the
/// user's account, which is the thing this file promises it never does.
const HALTS: [&str; 2] = [
    "No conversation found with session ID",
    "Input must be provided",
];

#[test]
#[ignore = "spawns the real claude binary: needs it on PATH. Costs no tokens — see the module \
            docs. Run with: cargo test -p cide-claude -- --ignored"]
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
        assert!(
            HALTS.iter().any(|halt| said.contains(halt)),
            "the `{name}` shape got past every guard that was supposed to stop it before a \
             request. This test is documented as free; if the CLI has changed where it \
             validates, fix `says` before running this again — {args:?}\n{said}"
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
