//! Does the real CLI still print the envelope `cide_claude::headless` parses?
//!
//! The unit tests in that module all run against a captured string. They prove the parser
//! reads what the CLI printed *on 2.1.226*, which is a different claim from "the parser reads
//! what the CLI prints". The output shape is undocumented — `--help` describes
//! `--output-format json` as a "single result" and it is in fact an array — the binary
//! self-updates, and a change here would leave every unit test green while every commit
//! message in the app came back as `Malformed`.
//!
//! `#[ignore]`d because it spawns the real `claude`: it needs the binary on PATH, network
//! access and the user's authentication, and it spends a small amount of money.
//!
//! Run with: `cargo test -p cide-claude -- --ignored`

use std::path::PathBuf;
use std::time::Duration;

use cide_claude::headless::{self, Headless, ToolAccess};
use cide_ipc::{HeadlessError, HeadlessRequest};

fn on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

/// What to do with a run that produced no result.
///
/// The distinction is the reason these tests are worth running. A rate limit, an expired
/// login, a missing binary or a slow network says nothing about the output envelope, and
/// failing on one gets the test disabled within a week. [`HeadlessError::Malformed`] is the
/// exact opposite: the CLI ran, printed, and printed something the parser could not read —
/// which is precisely the drift this file exists to catch, and the drift that leaves every
/// unit test green while every commit message in the app comes back as an error.
///
/// Returns `None` when the caller should skip; panics when the envelope has moved.
fn skip_or_fail(error: HeadlessError) -> Option<()> {
    match error {
        HeadlessError::Malformed { detail, head } => panic!(
            "the CLI's output no longer matches the envelope `cide_claude::headless` parses \
             ({detail}). Every headless feature is broken until the parser is updated. \
             It printed:\n{head}"
        ),
        other => {
            eprintln!("SKIP: {other}");
            None
        }
    }
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cide-m11-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
#[ignore = "spawns the real claude binary: needs it on PATH and the user's authentication. \
            Run with: cargo test -p cide-claude -- --ignored"]
fn a_one_shot_round_trips_through_the_real_cli() {
    let Some(claude) = on_path("claude") else {
        eprintln!("SKIP: no `claude` on PATH");
        return;
    };

    // A prompt that begins with `-` on purpose. As a positional argument this would be
    // parsed as a flag and the run would fail with a usage error; on stdin it is just text.
    // That is the whole reason `argv` does not carry the prompt, so the test that would
    // catch a regression has to use one.
    let request = HeadlessRequest::new("-- Reply with exactly one word: ok");
    let run = Headless::new(request, scratch())
        .tools(ToolAccess::None)
        .timeout(Duration::from_secs(120));

    let result = match headless::run(&claude, &run) {
        Ok(result) => result,
        Err(error) => {
            skip_or_fail(error);
            return;
        }
    };

    eprintln!(
        "subtype={} isError={} cost={:?} turns={:?} text={:?}",
        result.subtype, result.is_error, result.cost_usd, result.num_turns, result.text
    );

    assert!(
        !result.is_error,
        "the CLI reported a failure: {} — {}",
        result.subtype, result.text
    );
    assert!(
        !result.text.trim().is_empty(),
        "a successful run produced no text; the `result` field of the terminal frame has \
         moved or been renamed, and every headless feature would return an empty string"
    );
    assert_eq!(
        result.subtype, "success",
        "a successful run used to report subtype `success`"
    );
    assert!(
        result.duration_ms.is_some() && result.cost_usd.is_some(),
        "the result frame no longer carries duration_ms/total_cost_usd, which the palette \
         and the commit-message lane report to the user"
    );
    // Found by this test on 2.1.226, having been written the other way round first: with
    // `--no-session-persistence` the CLI *still* mints and reports a `session_id`. The flag
    // governs whether the transcript is written to disk, not whether the run has an id.
    //
    // Whether the transcript really stays off disk is deliberately not asserted here. The only
    // way to check is to look inside `~/.claude/projects/`, whose layout the project has
    // banned itself from depending on — it is documented as internal and version-unstable, and
    // a test that reads it would be the first thing to break when it changes.
    assert!(
        result.session.is_some(),
        "the result frame no longer reports a session id at all"
    );
}

#[test]
#[ignore = "spawns the real claude binary"]
fn structured_output_comes_back_parsed() {
    let Some(claude) = on_path("claude") else {
        eprintln!("SKIP: no `claude` on PATH");
        return;
    };

    // The commit-message lane's real shape: ask for an object, get an object. `--json-schema`
    // is what makes this reliable enough to feed into a text field without a parser of our
    // own guessing where the prose ends.
    let mut request = HeadlessRequest::new(
        "Summarise this change in a JSON object with a single `summary` field: \
         renamed `foo` to `bar` in one file.",
    );
    request.json_schema = Some(serde_json::json!({
        "type": "object",
        "properties": { "summary": { "type": "string" } },
        "required": ["summary"],
    }));
    request.max_budget_usd = Some(1.0);

    let run = Headless::new(request, scratch()).timeout(Duration::from_secs(120));

    let result = match headless::run(&claude, &run) {
        Ok(result) => result,
        Err(error) => {
            skip_or_fail(error);
            return;
        }
    };

    eprintln!("structured={:?} text={:?}", result.structured, result.text);
    let Some(structured) = result.structured else {
        panic!(
            "--json-schema no longer yields a JSON document in `result`; got {:?}",
            result.text
        );
    };
    assert!(
        structured.get("summary").and_then(|v| v.as_str()).is_some(),
        "the schema asked for a `summary` string and the CLI validated against it: {structured}"
    );
}
