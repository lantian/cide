//! Reformat code: one command, two roads. (M26)
//!
//! Ctrl+Alt+F reaches exactly one `#[tauri::command]`, and *which* formatter runs is decided
//! here rather than in the webview. That is the same argument `cmd::diagnostics` makes about
//! document sync: the frontend would need a copy of the settings map, a copy of the "is a server
//! running for this language" question and a copy of the precedence rule, and the copy is the
//! one that would be wrong when a third road is added.
//!
//! # Both roads are filters, and neither touches the disk
//!
//! The buffer arrives as `text`, the answer leaves as text, and nothing here opens, writes or
//! stats the file. Formatting the file *on disk* under a dirty buffer is the failure
//! `cide_core::document`'s `a_stale_precondition_refuses_and_leaves_the_file_alone` is written
//! against, and its worked example is `cargo fmt` — which is precisely the thing a user reaches
//! for when an editor will not format for them. The tab goes dirty; Ctrl+S is unchanged.
//!
//! # Never `Err`
//!
//! Every outcome is a [`cide_ipc::FormatAnswer`] arm carrying a sentence. A rejected promise
//! carries no sentence, and "no formatter for TypeScript" is an answer rather than a fault. Same
//! rule, and the same reason, as every command in `cmd::diagnostics`.

use cide_ipc::{FormatAnswer, FormatRange, ProjectId};
use tauri::State;

use crate::lsp::DiagnosticsRegistry;
use crate::workspace_state::WorkspaceState;

/// How long a formatter is given.
///
/// Longer than `DEFINITION_TIMEOUT`'s five seconds and far shorter than `USAGES_TIMEOUT`'s
/// twenty, because this is a keystroke the user is waiting on with their hands still on the
/// keyboard: rust-analyzer's `textDocument/formatting` shells out to `rustfmt`, and a cold
/// `prettier` pays Node's startup before it reads a byte. Ten seconds covers both on a loaded
/// machine and is short enough that a hung formatter is a pause rather than a lockup.
const FORMAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The largest buffer that is sent to a formatter.
///
/// **Not a performance guard — a correctness one, and it must match
/// `cmd::diagnostics::MAX_SYNC_BYTES`.** Above that cap `diagnostics_did_change` silently stops
/// sending the buffer, so the language server holds *the file as it was last saved*. Formatting
/// that and applying the result over the live buffer would discard every unsaved edit in a
/// megabyte-plus file without a word. Refusing is the only honest answer, and the sentence says
/// which cap was hit.
///
/// It bounds the configured-filter road too, where the reasoning is only about size — a buffer
/// that large is being written through a pipe and back — but one cap is easier to explain than
/// two that differ by road.
const MAX_FORMAT_BYTES: usize = 1 << 20;

/// Reformat the buffer, and say what happened.
///
/// `text` is the buffer, `languageId` routes it, and `range` is the selection when there is one
/// — used only if the language server advertises `documentRangeFormattingProvider`, and ignored
/// entirely on the configured-filter road, where a syntactic fragment on stdin is not something
/// any formatter can make sense of.
///
/// `spawn_blocking`, because both roads wait: one on a JSON-RPC reply, the other on a child
/// process. A command polled on the main thread holds the GTK loop and freezes every window.
#[tauri::command(rename_all = "camelCase")]
pub async fn format_document(
    registry: State<'_, DiagnosticsRegistry>,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    path: std::path::PathBuf,
    language_id: String,
    text: String,
    range: Option<FormatRange>,
) -> Result<FormatAnswer, ()> {
    // Read the managed state *before* the await — a `State<'_, _>` cannot be held across one,
    // the shape `cmd/session.rs` documents.
    let editor = state.with(|ws| ws.settings.editor.clone());
    let diagnostics = registry.get(project);

    Ok(tauri::async_runtime::spawn_blocking(move || {
        run(&editor, &diagnostics, &path, &language_id, &text, range)
    })
    .await
    .unwrap_or(FormatAnswer::Unavailable {
        reason: "The formatter did not finish.".to_string(),
    }))
}

/// The decision and the work, off the async runtime and away from `State`.
///
/// A free function over plain values so the precedence is testable without a Tauri app: see the
/// tests at the foot of this file.
fn run(
    editor: &cide_ipc::EditorSettings,
    diagnostics: &Option<std::sync::Arc<crate::lsp::ProjectDiagnostics>>,
    path: &std::path::Path,
    language_id: &str,
    text: &str,
    range: Option<FormatRange>,
) -> FormatAnswer {
    if text.len() > MAX_FORMAT_BYTES {
        return FormatAnswer::Unavailable {
            reason: format!(
                "This buffer is larger than {} MiB, which is the point above which cide stops \
                 sending it to a language server — formatting it could not see your unsaved \
                 edits.",
                MAX_FORMAT_BYTES / (1024 * 1024)
            ),
        };
    }

    // The precedence, stated once: a configured filter beats the language server. See
    // `cide_core::format`'s header for why that way round.
    match cide_core::format::configured(&editor.formatters, language_id, path) {
        cide_core::format::Configured::Filter { argv, name } => filter(&argv, &name, text),
        cide_core::format::Configured::Refused(reason) => FormatAnswer::Unavailable { reason },
        cide_core::format::Configured::None => match diagnostics {
            Some(diagnostics) => {
                diagnostics.format(path, text, range, options(editor), FORMAT_TIMEOUT)
            }
            // No server for this project at all — an unopened project, or one whose language has
            // none. The sentence names the settings road, because for the nine builtin languages
            // with no server that is the only road there is.
            None => FormatAnswer::Unavailable {
                reason: format!(
                    "No formatter for {language_id}: no language server is running for this \
                     project, and none is configured in Settings → Editor → Formatters."
                ),
            },
        },
    }
}

/// LSP's `FormattingOptions`, from the editor's own settings.
///
/// Both fields are required by the spec, and both are read rather than invented: a server told
/// `tabSize: 4` when the user set 2 reformats the file to the wrong width, which is a
/// disagreement the user sees on every line and cannot trace to a setting they did set.
///
/// Note what this does *not* do: it does not send `trimTrailingWhitespace`,
/// `insertFinalNewline` or `trimFinalNewlines`. Those are optional keys whose cide-side setting
/// (`EditorSettings::trim_trailing_whitespace_on_save`) is currently read by nothing, and
/// sending a value derived from an inert toggle would make the toggle look wired when it is not.
fn options(editor: &cide_ipc::EditorSettings) -> serde_json::Value {
    serde_json::json!({
        "tabSize": editor.tab_size,
        "insertSpaces": editor.insert_spaces,
    })
}

/// Run a user-configured filter: buffer in on stdin, formatted text out on stdout.
fn filter(argv: &[String], name: &str, text: &str) -> FormatAnswer {
    let Some((program, args)) = argv.split_first() else {
        // `cide_core::format::configured` refuses an empty argv before this is reached; a
        // legible answer beats an index panic if that ever stops being true.
        return FormatAnswer::Unavailable {
            reason: "The configured formatter has no program.".to_string(),
        };
    };
    let mut command = std::process::Command::new(program);
    command.args(args);

    // `prepare_command` and `arm` are applied inside `run_filter`, at the chokepoint — see its
    // docs for why they are not here.
    match cide_core::child_env::run_filter(command, Some(text.as_bytes()), FORMAT_TIMEOUT) {
        Ok(done) if done.ok => match String::from_utf8(done.stdout) {
            // A formatter that printed nothing while claiming success is not formatting an empty
            // file — it is a misconfiguration (a tool that rewrites in place and prints nothing,
            // `cargo fmt` being the one everybody tries). Applying it would silently empty the
            // buffer, which is the worst thing this feature could do, so it is refused by name.
            Ok(out) if out.is_empty() && !text.is_empty() => FormatAnswer::Unavailable {
                reason: format!(
                    "{name} succeeded but printed nothing. cide runs a formatter as a filter — \
                     it must read the buffer on stdin and write the result to stdout."
                ),
            },
            Ok(out) if out == text => FormatAnswer::Unchanged {
                by: name.to_owned(),
            },
            Ok(out) => FormatAnswer::Formatted {
                text: out,
                by: name.to_owned(),
            },
            // The buffer is UTF-8 by construction (`cide_core::document::read` refuses anything
            // else), so this is the formatter's output being broken, not the input's.
            Err(_) => FormatAnswer::Unavailable {
                reason: format!("{name} produced output that is not valid UTF-8."),
            },
        },
        // A non-zero exit is the normal way a formatter reports a syntax error, and its own
        // message is the useful part. First line only: `prettier` prints a code frame, and a
        // notice is not a terminal.
        Ok(done) => FormatAnswer::Unavailable {
            reason: match done.stderr.lines().find(|line| !line.trim().is_empty()) {
                Some(line) => format!("{name}: {}", line.trim()),
                None => format!("{name} exited with an error and said nothing."),
            },
        },
        Err(cide_core::child_env::FilterError::Spawn(error)) => FormatAnswer::Unavailable {
            reason: format!("{name} could not be started ({error})."),
        },
        Err(cide_core::child_env::FilterError::Timeout) => FormatAnswer::Unavailable {
            reason: format!(
                "{name} did not finish within {}s and was stopped.",
                FORMAT_TIMEOUT.as_secs()
            ),
        },
        Err(cide_core::child_env::FilterError::Unreadable) => FormatAnswer::Unavailable {
            reason: format!("{name} ran but its output could not be read."),
        },
        Err(cide_core::child_env::FilterError::Wait(error)) => FormatAnswer::Unavailable {
            reason: format!("{name} could not be waited for ({error})."),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(rows: &[(&str, &[&str])]) -> cide_ipc::EditorSettings {
        cide_ipc::EditorSettings {
            formatters: rows
                .iter()
                .map(|(id, tokens)| {
                    (
                        (*id).to_string(),
                        tokens.iter().map(|t| (*t).to_string()).collect(),
                    )
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_buffer_past_the_sync_cap_is_refused_rather_than_formatted_stale() {
        // THE ONE THAT MATTERS. Above `MAX_SYNC_BYTES` the server holds the file as last saved,
        // so formatting it and applying the result would throw away every unsaved edit. The
        // refusal is checked before the road is even chosen, so it holds for both.
        let editor = settings(&[]);
        let huge = "x".repeat(MAX_FORMAT_BYTES + 1);
        let answer = run(
            &editor,
            &None,
            std::path::Path::new("/w/a.rs"),
            "rust",
            &huge,
            None,
        );
        let FormatAnswer::Unavailable { reason } = answer else {
            panic!("a buffer past the cap must be refused, got {answer:?}");
        };
        assert!(
            reason.contains("MiB"),
            "the sentence names the cap: {reason}"
        );
    }

    #[test]
    fn the_cap_matches_the_document_sync_cap_it_exists_to_mirror() {
        // If `cmd::diagnostics`'s `MAX_SYNC_BYTES` moves and this does not, the gap between them
        // is a band of buffer sizes that format against stale text with nothing to notice it.
        assert_eq!(
            MAX_FORMAT_BYTES,
            1 << 20,
            "keep in step with `cmd::diagnostics::MAX_SYNC_BYTES`"
        );
    }

    #[test]
    fn no_server_and_no_row_names_the_settings_road() {
        // The nine builtin languages with no language server. The sentence has to point
        // somewhere the user can act, or Ctrl+Alt+F is a key that does nothing and says nothing.
        let answer = run(
            &settings(&[]),
            &None,
            std::path::Path::new("/w/a.ts"),
            "typescript",
            "let x=1\n",
            None,
        );
        let FormatAnswer::Unavailable { reason } = answer else {
            panic!("expected a refusal, got {answer:?}");
        };
        assert!(
            reason.contains("typescript"),
            "names the language: {reason}"
        );
        assert!(reason.contains("Formatters"), "names the screen: {reason}");
    }

    #[test]
    fn a_configured_filter_runs_without_any_language_server() {
        // The whole point of the second road: `diagnostics` is `None` here, which is what an
        // unopened project or a language with no server looks like.
        let editor = settings(&[("typescript", &["tr", "a-z", "A-Z"])]);
        let answer = run(
            &editor,
            &None,
            std::path::Path::new("/w/a.ts"),
            "typescript",
            "let x = 1;\n",
            None,
        );
        assert_eq!(
            answer,
            FormatAnswer::Formatted {
                text: "LET X = 1;\n".to_string(),
                by: "tr".to_string(),
            }
        );
    }

    #[test]
    fn a_filter_that_changes_nothing_is_unchanged_and_not_formatted() {
        // `cat` is the identity filter. `Unchanged` is what keeps a reflexive Ctrl+Alt+F from
        // dirtying the tab and pushing an undo step.
        let editor = settings(&[("typescript", &["cat"])]);
        let answer = run(
            &editor,
            &None,
            std::path::Path::new("/w/a.ts"),
            "typescript",
            "let x = 1;\n",
            None,
        );
        assert_eq!(
            answer,
            FormatAnswer::Unchanged {
                by: "cat".to_string()
            }
        );
    }

    #[test]
    fn a_filter_that_prints_nothing_is_refused_rather_than_emptying_the_buffer() {
        // The worst thing this feature could do, and the easiest to reach: `true` stands in for
        // every tool that rewrites files in place and prints nothing — `cargo fmt` above all,
        // which is exactly what a user reaches for first.
        let editor = settings(&[("rust", &["true"])]);
        let answer = run(
            &editor,
            &None,
            std::path::Path::new("/w/a.rs"),
            "rust",
            "fn  main(){}\n",
            None,
        );
        let FormatAnswer::Unavailable { reason } = answer else {
            panic!("must refuse, got {answer:?}");
        };
        assert!(
            reason.contains("stdout"),
            "says what the shape must be: {reason}"
        );
    }

    #[test]
    fn a_filter_that_fails_reports_its_own_first_line() {
        let editor = settings(&[("typescript", &["sh", "-c", "echo boom >&2; exit 1"])]);
        let answer = run(
            &editor,
            &None,
            std::path::Path::new("/w/a.ts"),
            "typescript",
            "x\n",
            None,
        );
        let FormatAnswer::Unavailable { reason } = answer else {
            panic!("must refuse, got {answer:?}");
        };
        assert!(
            reason.contains("boom"),
            "carries the tool's words: {reason}"
        );
    }

    #[test]
    fn a_formatter_that_is_not_installed_says_so_by_name() {
        let editor = settings(&[("typescript", &["cide-no-such-formatter-exists"])]);
        let answer = run(
            &editor,
            &None,
            std::path::Path::new("/w/a.ts"),
            "typescript",
            "x\n",
            None,
        );
        let FormatAnswer::Unavailable { reason } = answer else {
            panic!("must refuse, got {answer:?}");
        };
        assert!(
            reason.contains("cide-no-such-formatter-exists"),
            "names the program the user configured: {reason}"
        );
    }

    #[test]
    fn a_half_written_settings_row_is_refused_and_does_not_fall_through_to_the_server() {
        // Falling through would make a typo look exactly like "no formatter configured", and the
        // user would keep tuning a row nothing was reading.
        let editor = settings(&[("rust", &[""])]);
        let answer = run(
            &editor,
            &None,
            std::path::Path::new("/w/a.rs"),
            "rust",
            "fn main(){}\n",
            None,
        );
        assert!(
            matches!(answer, FormatAnswer::Unavailable { .. }),
            "got {answer:?}"
        );
    }

    #[test]
    fn the_options_come_from_the_users_own_indent_settings() {
        // A server told the wrong `tabSize` reformats every line to a width the user did not
        // choose, and nothing on screen points back at the setting that did it.
        let editor = cide_ipc::EditorSettings {
            tab_size: 2,
            insert_spaces: false,
            ..Default::default()
        };
        let options = options(&editor);
        assert_eq!(options["tabSize"], 2);
        assert_eq!(options["insertSpaces"], false);
    }
}
