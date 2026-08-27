//! Running `openspec`, and reading what it answered. (M28)
//!
//! Split into [`run`] and [`parse`] for `cide_deps::cargo`'s stated reason: the shape of a real
//! `openspec --json` document can then be a unit test on a machine with no CLI, no Node and no
//! network, and only the spawning half needs the real binary.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::SpecError;

/// Reads: list, show, status, instructions.
pub const READ: Duration = Duration::from_secs(10);
/// Validation, which forks a concurrency-6 pool over every spec when asked for `--all`.
pub const VALIDATE: Duration = Duration::from_secs(30);
/// The three that write: `init` configures up to forty AI tools, `archive` rewrites `specs/`.
pub const MUTATE: Duration = Duration::from_secs(60);

/// Forces the CLI's non-interactive path, whatever the command.
///
/// A *second* guard beside the null stdin, and both are load-bearing because the failure they
/// prevent is not an error: a bare `openspec init` prompts for which AI tools to configure, and a
/// prompt on a command worker is a hang until the deadline rather than a message anybody sees.
const NON_INTERACTIVE: &str = "OPEN_SPEC_INTERACTIVE";

/// Run `openspec` and hand back stdout.
///
/// # Why `run_filter_with` and not `Command::output`
///
/// It is the workspace's spawn chokepoint: `prepare_command` and `arm` are applied *inside* it,
/// which is what makes the pair impossible to forget — `blame`'s own call site had one and not
/// the other for two milestones. It also reads stdout and stderr on their own threads, which
/// matters here because `validate --all` on a large project writes more than a pipe buffer.
///
/// The extra `PATH` entry is this call's own directory, and it is not optional: `openspec` is a
/// `#!/usr/bin/env node` script, so without it `execve` succeeds and the shebang dies with
/// `env: node: No such file or directory`. See `child_env::prepare_command_with`.
pub fn run(
    binary: &Path,
    cwd: &Path,
    args: &[&str],
    deadline: Duration,
) -> Result<Vec<u8>, SpecError> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(cwd)
        .env(NON_INTERACTIVE, "0")
        // Colour in a JSON document would be escape sequences inside string values.
        .env("NO_COLOR", "1");

    let bin_dir: Vec<std::path::PathBuf> = binary
        .parent()
        .map(|d| vec![d.to_path_buf()])
        .unwrap_or_default();

    let started = std::time::Instant::now();
    let filtered = cide_core::child_env::run_filter_with(command, None, deadline, &bin_dir)
        .map_err(|error| SpecError::Ran(describe(binary, args, &error)))?;
    tracing::debug!(
        args = ?args,
        ms = started.elapsed().as_millis() as u64,
        bytes = filtered.stdout.len(),
        ok = filtered.ok,
        "openspec"
    );

    // The exit status is *not* the verdict — see the crate header — so it is only consulted when
    // stdout carried nothing to read. A refusal with a JSON body is reported by `parse` from the
    // `status` array, in the CLI's own words.
    if !filtered.ok && filtered.stdout.iter().all(u8::is_ascii_whitespace) {
        let detail = first_line(&filtered.stderr);
        return Err(SpecError::Refused(if detail.is_empty() {
            format!("`openspec {}` failed and said nothing", args.join(" "))
        } else {
            detail
        }));
    }
    Ok(filtered.stdout)
}

/// Deserialise one `--json` document, checking the `status` array first.
///
/// # The order is the point
///
/// Every `--json` command exits **0** and reports failure as `status: [{severity, code, …}]` on
/// stdout. So a caller that deserialised into its expected shape and looked no further would read
/// `openspec show nope --json` as a change with no deltas — a real board, describing nothing.
/// `status` is therefore read out of the raw document *before* the typed one is built.
pub fn parse<T: serde::de::DeserializeOwned>(stdout: &[u8], what: &str) -> Result<T, SpecError> {
    let text = std::str::from_utf8(stdout).map_err(|error| SpecError::Unreadable {
        what: what.to_string(),
        detail: format!("its output was not UTF-8: {error}"),
    })?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(SpecError::Unreadable {
            what: what.to_string(),
            detail: "it answered nothing at all".to_string(),
        });
    }

    let raw: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|error| SpecError::Unreadable {
            what: what.to_string(),
            detail: format!("its output was not JSON: {error}"),
        })?;
    if let Some(refusal) = refusal_in(&raw) {
        return Err(SpecError::Refused(refusal));
    }

    serde_json::from_value(raw).map_err(|error| SpecError::Unreadable {
        what: what.to_string(),
        detail: error.to_string(),
    })
}

/// The `status` array's errors, as one sentence, or `None` when there are none.
///
/// Only `severity: "error"` refuses. The array also carries warnings and notes, and a build that
/// treated an INFO note as a failure would make a board unreadable over a spelling suggestion.
fn refusal_in(raw: &serde_json::Value) -> Option<String> {
    let entries = raw.get("status")?.as_array()?;
    let messages: Vec<String> = entries
        .iter()
        .filter(|entry| {
            entry
                .get("severity")
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("error"))
        })
        .map(|entry| {
            let message = entry
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("openspec refused");
            match entry.get("code").and_then(|c| c.as_str()) {
                Some(code) => format!("{message} ({code})"),
                None => message.to_string(),
            }
        })
        .collect();
    (!messages.is_empty()).then(|| messages.join("; "))
}

/// A spawn failure, in words that name the binary that could not be run.
fn describe(binary: &Path, args: &[&str], error: &cide_core::child_env::FilterError) -> String {
    use cide_core::child_env::FilterError;
    let call = format!("`{} {}`", binary.display(), args.join(" "));
    match error {
        FilterError::Spawn(io) => format!("{call} could not be started: {io}"),
        FilterError::Timeout => format!(
            "{call} did not finish in time and was stopped. If it was `init`, it may have been \
             waiting for an answer — cide runs it non-interactively, so that is a bug worth \
             reporting."
        ),
        FilterError::Unreadable => format!("{call} ran and its output could not be read"),
        FilterError::Wait(io) => format!("{call} ran and could not be reaped: {io}"),
    }
}

/// The first non-empty line, which is where a CLI puts the sentence worth showing.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;

    #[test]
    fn a_json_failure_is_a_status_array_and_not_an_exit_code() {
        // The single most load-bearing parse rule in this crate. `openspec show nope --json`
        // exits 0 with the command's null shape plus a status array; a build that deserialised
        // and looked no further would render this as a real board describing nothing.
        let stdout = br#"{"changes":[],"root":null,
            "status":[{"severity":"error","code":"unknown_item","message":"No change named nope"}]}"#;
        let error = parse::<model::ListChanges>(stdout, "show nope --json")
            .expect_err("the status array refuses");
        let sentence = error.to_string();
        assert!(sentence.contains("No change named nope"), "{sentence}");
        assert!(sentence.contains("unknown_item"), "{sentence}");
    }

    #[test]
    fn a_warning_in_the_status_array_is_not_a_refusal() {
        // The array carries notes and warnings too, and a board unreadable over a spelling
        // suggestion would be the cure being worse than the disease.
        let stdout = br#"{"changes":[],"root":{"path":"/repo","source":"nearest"},
            "status":[{"severity":"warning","message":"a spec has no requirements"}]}"#;
        let parsed: model::ListChanges = parse(stdout, "list --json").expect("warnings pass");
        assert!(parsed.changes.is_empty());
    }

    #[test]
    fn output_that_is_not_json_is_reported_rather_than_becoming_an_empty_board() {
        // `cide_deps::cargo`'s test, one crate over: the failure mode to avoid is a silent empty
        // answer that looks exactly like a project with nothing in it.
        let error = parse::<model::ListChanges>(b"Usage: openspec [options]", "list --json")
            .expect_err("prose is not a document");
        assert!(error.to_string().contains("not JSON"), "{error}");

        let error = parse::<model::ListChanges>(b"   ", "list --json")
            .expect_err("silence is not a document");
        assert!(error.to_string().contains("nothing at all"), "{error}");
    }

    #[test]
    fn a_document_missing_a_field_this_build_needs_names_the_command() {
        // A field whose *type* changed, which is what an upstream rename looks like from here —
        // and note `{"progress":{}}` deliberately does **not** fail: every field this build can
        // do without carries a default, so a newer CLI adding fields still produces a board.
        let error = parse::<model::ApplyInstructions>(
            br#"{"tasks":[],"progress":"none"}"#,
            "instructions apply",
        )
        .expect_err("the shape does not fit");
        let sentence = error.to_string();
        assert!(sentence.contains("instructions apply"), "{sentence}");
        assert!(
            sentence.contains("older than the installed CLI"),
            "the likeliest cause is named, because that is the one the user can act on: \
             {sentence}"
        );
    }
}
