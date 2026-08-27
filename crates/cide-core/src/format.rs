//! Which formatter reformats a buffer, and the argv it runs. (M26)
//!
//! The pure half of Reformat code. It answers one question — *given this language and these
//! settings, what runs?* — and nothing here spawns anything, reads a file, or knows what a
//! language server is. `cide_app::lsp` asks the question and acts on the answer;
//! `cide_core::child_env::run_filter` does the running.
//!
//! # The precedence, and why it is this way round
//!
//! A configured filter **beats** the language server. Both shipped servers are bundled now, so
//! rust-analyzer's rustfmt and gopls' gofmt are what a user gets without asking for anything —
//! and this map is the only lever they have over that choice. The alternative ordering makes it
//! no lever at all: a `rust` row would be dead text for as long as a Rust server was running,
//! and the only way to reach it would be switching the whole server to
//! [`cide_ipc::settings::ServerBinaryChoice::System`] — a far larger hammer than "I want
//! `rustfmt +nightly`".
//!
//! # Never a shell
//!
//! [`argv`] produces a `Vec<String>` that reaches `execvp` directly. `$HOME`, `*`, `|`, `&&`,
//! backticks and a semicolon are ordinary argument bytes, because there is no interpreter to
//! read them. The two placeholders below are cide's own and are substituted *inside* a token,
//! so `--stdin-filepath=${file}` is one argument and stays one however many spaces the path
//! contains. That is the whole of the substitution language, on purpose: a user who needs a
//! pipeline writes a two-line script and names the script.
//!
//! # The builtin road (M32)
//!
//! [`configured`] still answers only the settings question. [`builtin`] is a different one —
//! *does cide itself know how to format this language?* — and it is the floor beneath both
//! other roads: `cide_app::cmd::format` reaches it only when no settings row exists **and**
//! no language server claims the language, so a user's row and an installed server both stay
//! live levers over it. Today the answer is yes for exactly one language, JSON, whose
//! reprinter lives in [`json`].

use std::path::Path;

pub mod json;

/// What the readout names when cide's own formatter did the work.
///
/// Every other `by` is a binary's file name — `prettier`, `rustfmt` — because every other
/// road runs a binary. This road is compiled in, so the honest name is the application's own;
/// the exception is documented on `FormatAnswer::Formatted::by`.
pub const BUILTIN_FORMATTER_NAME: &str = "cide";

/// cide's own formatter for `language_id`, when it has one.
///
/// `None` — cide has no builtin for this language, and the caller falls through to its
/// no-formatter sentence. `Some(Err(sentence))` — cide has one and the text refuses to
/// parse; the sentence is shown to the user verbatim, the same refused-rather-than-ignored
/// contract as [`Configured::Refused`]. `Some(Ok(text))` may equal the input, which the
/// caller maps to `Unchanged` so a reflexive reformat stays silent.
#[must_use]
pub fn builtin(
    language_id: &str,
    text: &str,
    tab_size: u8,
    insert_spaces: bool,
) -> Option<Result<String, String>> {
    match language_id {
        "json" => Some(
            json::reformat(text, tab_size, insert_spaces)
                .map_err(|error| format!("Not formatted: {error}. The buffer was left alone.")),
        ),
        _ => None,
    }
}

/// The placeholder for the buffer's own path — absolute, as the editor holds it.
///
/// Needed far more often than it looks: `prettier` and `eslint` pick a *parser* from the file
/// name and cannot do it from a stdin stream, so `--stdin-filepath ${file}` is the difference
/// between formatting TypeScript and formatting nothing. The file need not exist on disk with
/// this content — that is the point of a filter — so the name is all that is promised.
pub const FILE_PLACEHOLDER: &str = "${file}";

/// The placeholder for the directory holding the buffer, for a tool that resolves its own
/// configuration by walking up from a working directory rather than from a file name.
pub const DIR_PLACEHOLDER: &str = "${dir}";

/// What the settings say about formatting one language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Configured {
    /// Nothing is configured. Ask the language server.
    None,
    /// Run this, as a filter: buffer in on stdin, formatted text out on stdout.
    Filter {
        /// Program first, arguments after, placeholders already substituted.
        argv: Vec<String>,
        /// What the readout names — the program's file name, without its directory. `prettier`,
        /// not `/home/me/.local/bin/prettier`: the sentence is for a person who configured it
        /// and already knows where it lives.
        name: String,
    },
    /// A row exists for this language and cannot be run. **Refused rather than ignored**, and
    /// that distinction is the whole reason this arm exists: falling through to the language
    /// server would make a typo in the settings look exactly like "no formatter configured",
    /// and the user would be left tuning a row that was never being read.
    Refused(String),
}

/// What the settings say about `language_id`, with placeholders resolved against `path`.
///
/// `path` is the buffer's own path; it is only ever substituted into arguments, never opened.
#[must_use]
pub fn configured(
    formatters: &std::collections::BTreeMap<String, Vec<String>>,
    language_id: &str,
    path: &Path,
) -> Configured {
    // Absent means "nothing configured", which is the common case and the one that must stay
    // cheap — the same absent-means-default rule `InspectionSettings::server_binaries` states.
    let Some(tokens) = formatters.get(language_id) else {
        return Configured::None;
    };

    // A row of nothing is a row somebody started and abandoned. It is *not* "no formatter":
    // see `Configured::Refused`.
    let program = tokens.iter().find(|token| !token.trim().is_empty());
    let Some(program) = program else {
        return Configured::Refused(format!(
            "The formatter configured for {language_id} has no program. Settings → Editor → \
             Formatters."
        ));
    };
    // The program is the *first* token, and an empty leading row is a mistake worth naming
    // rather than silently skipping: skipping it would run the second token as the program,
    // which is how `["", "prettier"]` quietly becomes something nobody wrote.
    if tokens[0].trim().is_empty() {
        return Configured::Refused(format!(
            "The formatter configured for {language_id} starts with an empty argument. \
             Settings → Editor → Formatters."
        ));
    }

    let name = Path::new(program).file_name().map_or_else(
        || program.clone(),
        |name| name.to_string_lossy().into_owned(),
    );

    Configured::Filter {
        argv: argv(tokens, path),
        name,
    }
}

/// Substitute cide's two placeholders through a token list.
///
/// Substitution is textual and *within* a token, so `--stdin-filepath=${file}` stays one
/// argument. An unrecognised `${…}` is left exactly as written rather than replaced with an
/// empty string: a tool that receives `${flie}` says so, and a tool that receives an empty
/// argument usually does something silently wrong with it.
///
/// A path that is not valid UTF-8 is substituted lossily. The alternative — refusing to format
/// — is worse for a case that on Linux means a filename nobody can type anyway, and the tool
/// receives a name that at worst does not match a file it was never going to open.
#[must_use]
pub fn argv(tokens: &[String], path: &Path) -> Vec<String> {
    let file = path.to_string_lossy();
    let dir = path.parent().map_or_else(
        || std::borrow::Cow::Borrowed(""),
        |dir| dir.to_string_lossy(),
    );
    tokens
        .iter()
        .map(|token| {
            token
                .replace(FILE_PLACEHOLDER, &file)
                .replace(DIR_PLACEHOLDER, &dir)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(rows: &[(&str, &[&str])]) -> std::collections::BTreeMap<String, Vec<String>> {
        rows.iter()
            .map(|(id, tokens)| {
                (
                    (*id).to_string(),
                    tokens.iter().map(|t| (*t).to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_language_with_no_row_asks_the_language_server() {
        let settings = map(&[("typescript", &["prettier"])]);
        assert_eq!(
            configured(&settings, "rust", Path::new("/w/src/main.rs")),
            Configured::None,
            "absent means the server, not a refusal — otherwise Rust would stop formatting the \
             moment anybody configured prettier for TypeScript"
        );
    }

    #[test]
    fn a_configured_row_wins_over_the_language_server() {
        // The precedence the module header argues for: `rust` has a bundled server that formats,
        // and a row here still takes it. This is the only lever a user has over the shipped
        // rustfmt.
        let settings = map(&[("rust", &["rustfmt", "+nightly", "--emit", "stdout"])]);
        let Configured::Filter { argv, name } =
            configured(&settings, "rust", Path::new("/w/src/main.rs"))
        else {
            panic!("a configured row must not fall through to the server");
        };
        assert_eq!(argv, ["rustfmt", "+nightly", "--emit", "stdout"]);
        assert_eq!(name, "rustfmt");
    }

    #[test]
    fn the_file_placeholder_survives_a_path_with_spaces_as_one_argument() {
        // THE ONE THAT MATTERS for the `Vec<String>`-not-a-string decision. A shell string would
        // need a quoting parser here and would split this into two arguments; prettier would
        // then be handed `/w/my` and report a file that does not exist.
        let settings = map(&[("typescript", &["prettier", "--stdin-filepath", "${file}"])]);
        let Configured::Filter { argv, .. } = configured(
            &settings,
            "typescript",
            Path::new("/w/my project/src/a b.ts"),
        ) else {
            panic!("configured");
        };
        assert_eq!(argv.len(), 3, "three tokens in, three arguments out");
        assert_eq!(argv[2], "/w/my project/src/a b.ts");
    }

    #[test]
    fn a_placeholder_substitutes_inside_a_token() {
        // `--flag=${file}` is one argument and has to stay one.
        let out = argv(
            &["--stdin-filepath=${file}".to_string()],
            Path::new("/w/a.ts"),
        );
        assert_eq!(out, ["--stdin-filepath=/w/a.ts"]);
    }

    #[test]
    fn the_dir_placeholder_is_the_parent() {
        let out = argv(
            &["--config-dir".to_string(), "${dir}".to_string()],
            Path::new("/w/src/a.ts"),
        );
        assert_eq!(out, ["--config-dir", "/w/src"]);
    }

    #[test]
    fn nothing_is_expanded_that_cide_did_not_define() {
        // No shell means no shell *syntax*. Every one of these is ordinary argument text, and a
        // reader who assumes otherwise is the reason this test names them one by one.
        let out = argv(
            &[
                "$HOME".to_string(),
                "*.ts".to_string(),
                "a|b".to_string(),
                "x && rm -rf /".to_string(),
                "`id`".to_string(),
                "${flie}".to_string(),
            ],
            Path::new("/w/a.ts"),
        );
        assert_eq!(
            out,
            ["$HOME", "*.ts", "a|b", "x && rm -rf /", "`id`", "${flie}"],
            "a misspelt placeholder stays literal rather than becoming an empty argument"
        );
    }

    #[test]
    fn an_empty_row_is_refused_and_not_treated_as_absent() {
        // The distinction `Configured::Refused` exists for. Falling through to the server here
        // would make a half-typed settings row indistinguishable from no row at all.
        for tokens in [&[][..], &[""][..], &["   "][..]] {
            let settings = map(&[("typescript", tokens)]);
            let answer = configured(&settings, "typescript", Path::new("/w/a.ts"));
            assert!(
                matches!(answer, Configured::Refused(_)),
                "{tokens:?} must be refused with a sentence, got {answer:?}"
            );
        }
    }

    #[test]
    fn an_empty_leading_argument_never_promotes_the_second_token_to_the_program() {
        // `["", "prettier"]` must not quietly run prettier: the user wrote a program slot and
        // left it blank, and running whatever came next is how a settings typo becomes a
        // different program than anybody chose.
        let settings = map(&[("typescript", &["", "prettier"])]);
        let answer = configured(&settings, "typescript", Path::new("/w/a.ts"));
        assert!(matches!(answer, Configured::Refused(_)), "got {answer:?}");
    }

    #[test]
    fn json_has_a_builtin_and_typescript_does_not() {
        // The `None` side is as load-bearing as the `Some`: a builtin for a language cide
        // cannot actually reprint would silently mangle files, so the table is a closed match
        // and this test pins its edge.
        assert!(matches!(builtin("json", "{\"a\":1}", 2, true), Some(Ok(_))));
        assert_eq!(builtin("typescript", "let x = 1;\n", 2, true), None);
    }

    #[test]
    fn the_builtin_reports_a_syntax_error_as_a_sentence() {
        let Some(Err(sentence)) = builtin("json", "{\"a\": 1", 2, true) else {
            panic!("broken JSON must be refused with a sentence");
        };
        assert!(sentence.contains("line"), "names the position: {sentence}");
    }

    #[test]
    fn the_readout_name_is_the_programs_file_name() {
        let settings = map(&[("python", &["/home/me/.venv/bin/black", "-"])]);
        let Configured::Filter { name, argv } =
            configured(&settings, "python", Path::new("/w/a.py"))
        else {
            panic!("configured");
        };
        assert_eq!(
            name, "black",
            "the sentence names the tool, not its install path"
        );
        assert_eq!(
            argv[0], "/home/me/.venv/bin/black",
            "but the argv keeps the path it was given — resolving it here would undo the reason \
             somebody wrote an absolute path"
        );
    }
}
