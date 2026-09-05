//! What this binary does when it is handed arguments it was not written for.
//!
//! # The failure this module exists for
//!
//! cide parsed exactly one flag — `--wait`, handled a layer above in [`crate::edit_wait::cli`] —
//! and *started the whole application* for everything else. `cide --help` opened a second IDE.
//!
//! That is not a hypothetical. `child_env::editor_env` sets `EDITOR=<this binary> --wait` in
//! every PTY child, so the path to a running cide is in the environment of every `claude`, every
//! shell pane and every dispatched agent run. On 2026-09-04 an agent run went looking for cide's
//! CLI, ran `--help` to find it, got a second full IDE on the same profile, and then tried to
//! clean the stray up with `pkill -f mount_cide` — which matched every `claude` in the *first*
//! instance, because their `--settings`/`--mcp-config` argv carries the path of that instance's
//! `cide-hook`. Every session in every project of the installed build died at once. Twice.
//!
//! So the refusal below is not tidiness about POSIX conventions. The launch it prevents is the
//! expensive half of that morning: a probe costs a sentence instead of an IDE.
//!
//! # Why this is not a `clap`
//!
//! There are three flags and there has never been a fourth. A dependency here would have to be
//! parsed before the graphics ladder on a path that must not fail, and would earn nothing but a
//! prettier `--help`. What matters is that an *unrecognised* argument stops, which is one `if`.
//!
//! Positional arguments are deliberately **not** an error — see [`respond`].

use cide_core::child_env::WAIT_FLAG;

/// Exit code for an argument that was answered here: `--help`, `--version`.
pub const EXIT_ANSWERED: i32 = 0;

/// Exit code for an argument nothing understands.
///
/// 2 rather than 1, so a caller can tell "cide refused to parse this" from "cide ran and
/// failed" — [`crate::edit_wait`] already spends 1 on the latter and 2 on a usage error, and
/// two exit vocabularies out of one binary would be worse than either.
pub const EXIT_USAGE: i32 = 2;

/// What [`cli`] decided the arguments were asking for.
///
/// A value rather than a `println!` at the point of the decision, so the whole of [`respond`]
/// is testable without capturing the process's own streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// Print the usage on stdout and exit [`EXIT_ANSWERED`].
    Help,
    /// Print the version on stdout and exit [`EXIT_ANSWERED`].
    Version,
    /// Print this on stderr and exit [`EXIT_USAGE`]. Carries the offending argument.
    Unknown(String),
}

/// `cide --help`, `cide --version`, or `cide --nonsense`, if that is what this process was asked
/// to be.
///
/// `None` means the arguments are not a question this module answers and the caller should go on
/// to start the application. `Some(code)` means the answer has been printed and the process
/// should exit with it — **before the graphics ladder and before anything touches GTK**, which
/// is the same rule [`crate::edit_wait::cli`] states and for the same reason: neither mode
/// creates a window, and both must work on a machine with no display.
pub fn cli() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    Some(match respond(&args)? {
        Response::Help => {
            println!("{}", usage());
            EXIT_ANSWERED
        }
        Response::Version => {
            println!("{}", version());
            EXIT_ANSWERED
        }
        Response::Unknown(argument) => {
            eprintln!("{}", unknown(&argument));
            EXIT_USAGE
        }
    })
}

/// The whole decision, over arguments the caller supplies. `None` starts the application.
///
/// # Two rules, and why the second one is not the first
///
/// **A flag nothing here knows is a refusal.** That is the rule the module exists for: the
/// argument was meant for a CLI, and starting a GUI instead is the surprise.
///
/// **A positional argument is not.** It is tempting to refuse those too — `cide foo.rs` does not
/// open `foo.rs`, it silently starts the IDE, which is the same class of surprise. But cide has
/// never opened a file from the command line, a desktop entry or a file manager can hand a
/// binary a path without being asked, and turning that from "starts the app" into "refuses to
/// start" is a regression for anybody doing it today. The bias everywhere in this module and in
/// [`crate::instance`] is that a *wrong refusal to launch* is worse than a wrong launch. So a
/// path is ignored exactly as it always was, and [`usage`] says so out loud rather than leaving
/// the user to discover it.
pub fn respond(args: &[String]) -> Option<Response> {
    for argument in args {
        match argument.as_str() {
            // Everything after `--` is positional by convention, including things that look
            // like flags. Nothing downstream reads them, but refusing them would break the
            // convention rather than uphold it.
            "--" => return None,
            "--help" | "-h" => return Some(Response::Help),
            "--version" | "-V" => return Some(Response::Version),
            // Consumed by `edit_wait::cli`, which `main` runs first and which exits for every
            // spelling of this flag — so this arm is unreachable today. It is here so that a
            // reordering of `main` degrades to the old behaviour (the app starts) rather than
            // to cide refusing a flag it documents, which is the more confusing of the two.
            flag if flag == WAIT_FLAG || flag == "-w" => return None,
            // `-` alone is stdin by convention and not a flag; a bare word is positional.
            flag if flag.starts_with('-') && flag != "-" => {
                return Some(Response::Unknown(flag.to_string()));
            }
            _ => {}
        }
    }
    None
}

/// `cide 0.7.1-dev`. The version is the crate's, so it moves with the release workflow's bump.
pub fn version() -> String {
    format!("cide {}", env!("CARGO_PKG_VERSION"))
}

/// The sentence an unrecognised argument gets.
///
/// It names what would otherwise have happened, because that is the part nobody guesses: the
/// person or agent reading this line asked a *binary* a question and would have been handed an
/// IDE. Saying "unrecognised option" alone would leave them to try `--usage` next, and get one.
pub fn unknown(argument: &str) -> String {
    format!(
        "cide: unrecognised option '{argument}'\n\
         cide takes almost no arguments — with none it starts the full IDE, which is\n\
         very probably not what this call wanted. Try 'cide --help'."
    )
}

/// The usage text. Names the environment variables because the two that matter here are the two
/// that decide *which* cide a launch becomes, and neither is discoverable from a flag list.
pub fn usage() -> String {
    format!(
        "{}\n\
         An IDE built around a live Claude Code session.\n\
         \n\
         USAGE:\n    \
             cide [OPTIONS]\n\
         \n\
         OPTIONS:\n    \
             {WAIT_FLAG}, -w <file>   Open <file> in the cide that started this session and\n                        \
                                 block until its tab is closed. This is what $EDITOR is set\n                        \
                                 to inside a cide pane; it opens no window and needs\n                        \
                                 $CIDE_EDIT_SOCK, which only such a session has.\n    \
             --version, -V       Print the version and exit.\n    \
             --help, -h          Print this and exit.\n\
         \n\
         ENVIRONMENT:\n    \
             CIDE_PROFILE                 Name a profile: its own workspace, settings and\n                                 \
                                          window state, sharing nothing with the default one.\n                                 \
                                          This is how to run a second cide on one machine.\n    \
             CIDE_ALLOW_SECOND_INSTANCE   Start even though this profile already has a running\n                                 \
                                          instance. Both then write one workspace.json and the\n                                 \
                                          last to exit wins, so prefer CIDE_PROFILE.\n\
         \n\
         With no arguments cide starts the application and restores its profile's workspace.\n\
         A file path on its own is *not* opened: it is ignored and the workspace is restored\n\
         as usual. To open a file in a running cide, use {WAIT_FLAG} from inside one of its panes.",
        version()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_start_the_application() {
        assert_eq!(respond(&[]), None);
    }

    #[test]
    fn help_and_version_are_answered() {
        for flag in ["--help", "-h"] {
            assert_eq!(respond(&args(&[flag])), Some(Response::Help), "{flag}");
        }
        for flag in ["--version", "-V"] {
            assert_eq!(respond(&args(&[flag])), Some(Response::Version), "{flag}");
        }
    }

    /// The whole point. `cide --help` used to reach `run()`.
    #[test]
    fn an_unknown_flag_refuses_rather_than_starting_an_ide() {
        assert_eq!(
            respond(&args(&["--nonsense"])),
            Some(Response::Unknown("--nonsense".into()))
        );
        assert_eq!(
            respond(&args(&["-x"])),
            Some(Response::Unknown("-x".into()))
        );
        // And the sentence has to name the consequence, or the next thing tried is `--usage`.
        assert!(unknown("--usage").contains("starts the full IDE"));
    }

    /// A path is the one argument that must go on being ignored rather than refused.
    #[test]
    fn positional_arguments_still_start_the_application() {
        assert_eq!(respond(&args(&["/home/u/work/main.rs"])), None);
        assert_eq!(respond(&args(&["--", "--not-a-flag-here"])), None);
    }

    /// `--wait` belongs to `edit_wait`, and must never be answered as unknown here.
    #[test]
    fn the_wait_flag_is_left_to_its_own_handler() {
        assert_eq!(respond(&args(&[WAIT_FLAG, "file.txt"])), None);
        assert_eq!(respond(&args(&["-w", "file.txt"])), None);
    }

    /// The flag can arrive after a positional argument, and a real `--help` must still win over
    /// a path that precedes it.
    #[test]
    fn a_flag_is_found_wherever_it_sits() {
        assert_eq!(respond(&args(&["file.rs", "--help"])), Some(Response::Help));
    }

    #[test]
    fn the_usage_names_the_flags_it_documents() {
        let text = usage();
        for wanted in [WAIT_FLAG, "--version", "--help", "CIDE_PROFILE"] {
            assert!(text.contains(wanted), "usage does not mention {wanted}");
        }
        assert!(version().starts_with("cide "));
    }
}
