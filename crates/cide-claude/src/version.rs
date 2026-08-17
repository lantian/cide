//! Which `claude` is on `PATH`, and whether the IDE protocol was ever checked against it.
//!
//! # Why this is worth a module
//!
//! [`cide_ide_mcp::protocol`] is a transcription of an undocumented wire protocol, read out of
//! a specific build of the CLI. There is no *protocol* version anywhere in it — not in the
//! lockfile, not in the MCP handshake — so a running cide cannot negotiate, feature-detect or
//! degrade. It can only be right or wrong, and it finds out by breaking.
//!
//! (The handshake does carry the CLI's own `clientInfo.version`, which this module's doc used
//! to deny. It changes nothing about negotiation — you cannot negotiate with a name — but it
//! is a better *record* than the probe below, because it names the binary that actually
//! connected rather than whichever one answered a latched `--version`. `cide_ide_mcp::server`
//! reads it; `cide_core::handshake` stores it.)
//!
//! The CLI also updates itself: 2.1.221 through 2.1.226 inside a fortnight on the development
//! machine. So the interesting question is never "does this version work" — nobody can answer
//! that from inside — but "is this a version anybody checked". `SUPPORTED_CLI` is the record
//! of what was checked, and until this module existed nothing read it. A warning here is the
//! earliest signal available that the protocol may have drifted, and it costs one line in a
//! log against a bug report that would otherwise begin with "the diff view stopped working".
//!
//! # What is left here, and what moved
//!
//! The comparison — `Support`, `support_of`, `verified_range`, the version parse — now lives
//! beside `SUPPORTED_CLI` in `cide_ide_mcp::protocol`, and is re-exported from here so no
//! call site moved. It went there because a real-CLI test inside that crate has to judge the
//! version of a binary it just handshook with, and reaching back into this crate for the rule
//! would be a dev-dependency cycle pointing the wrong way up the graph. What remains is the
//! part that is about running the program: [`probe`] and [`check_once`].
//!
//! # Recorded, never enforced
//!
//! Refusing to run outside the range would break the app roughly every fortnight to protect
//! a feature that mostly keeps working across releases. The rule from `protocol` holds: a
//! protocol change degrades the diff view and never breaks the terminal. So the strongest
//! thing here is a warning, and the strongest thing a *user* sees is a note in Settings.
//!
//! # What else in this codebase a patch release can retract
//!
//! Worth listing in one place, because 2.1.227 proved the list is not theoretical: it removed
//! `--session-id`'s compatibility with a plain `--resume`, a combination that had been checked
//! against 2.1.226 and recorded in a comment, and the result was that every Resume click in
//! the app failed. A comment cannot be re-run; a test can. Each assertion below therefore
//! names the `#[ignore]`d test that puts the real binary behind it — run them all with
//! `cargo test --workspace -- --ignored` when something that used to work stops.
//!
//! | Assertion | Where | Real-CLI check | If it breaks |
//! |---|---|---|---|
//! | argv shapes for resume and fork | [`crate::session`] | `cide-claude/tests/real_session_args.rs` | a pane refuses to start |
//! | hook events and `--settings` schema | [`crate::hook`], [`crate::settings`] | `cide-claude/tests/real_hooks.rs` | silent: no token figures, no busy-vs-idle |
//! | `--print --output-format json` envelope | [`crate::headless`] | `cide-claude/tests/real_headless.rs` | commit messages and explanations fail |
//! | IDE tool names, lockfile shape | `cide_ide_mcp::protocol`, `::lockfile` | `cide-ide-mcp/tests/real_cli.rs` | silent: diffs stop appearing |
//! | transcript path `<projects>/<encoded cwd>/<id>.jsonl` | `cide_app::lifecycle` | none — see below | a resumable pane offers no resume |
//!
//! The last row is the one with no test and deliberately so: it is a *filesystem* detail, it
//! is read-only, and `transcript_exists` is written so that every way of being wrong answers
//! `false`. Its own doc says why a wrong `false` is the failure mode worth choosing.

use std::path::Path;
use std::sync::OnceLock;

// The comparison itself lives beside the constant it compares against, in
// `cide_ide_mcp::protocol`. It moved there when the real-CLI check arrived: a test inside that
// crate has to judge the version of a binary it has just completed a handshake with, and
// reaching back here for the arithmetic would be a dev-dependency cycle pointing the wrong way
// up the graph. What is left in this module is the part that is genuinely about *this* crate —
// running the CLI and latching what it said.
//
// Re-exported rather than merely used, because every call site in `cide-app` names
// `cide_claude::version::…` and none of them had any reason to move.
pub use cide_ide_mcp::protocol::{Support, support_of, verified_range};

/// Probe `claude --version` once per process and report the verdict, warning if it is one.
///
/// **Once, not per pane.** Every Claude spawn is a reasonable place to want this check and a
/// terrible place to run it: a `fork`/`exec` per pane to learn something that cannot change
/// while the process runs, and a warning repeated into the log for every pane the user opens
/// until it means nothing. The latch makes calling it from a spawn path free after the first
/// time, which is what lets the call sites be the obvious ones.
///
/// The version genuinely can change under a running cide — the CLI self-updates — and this
/// deliberately does not notice. Re-probing would mean re-warning, and the answer that
/// matters is the one for the sessions that are already running.
///
/// **And that latch is exactly why this is the weaker of the two version sources cide now
/// has.** It describes whichever binary was on `PATH` at the first Claude spawn of this
/// process. The `clientInfo.version` in a real `initialize` names the binary that *actually
/// connected*, self-reported, at the moment it connected — see `cide_ide_mcp::server`'s
/// `ServerEvent::Connected` and the handshake record in `cide_core::handshake`. Both are kept:
/// this one is available before anything has connected, and on a machine where nothing ever
/// does.
pub fn check_once(program: &Path) -> &'static Support {
    static CHECKED: OnceLock<Support> = OnceLock::new();
    CHECKED.get_or_init(|| {
        let support = support_of(probe(program).as_deref());
        if support.is_warning() {
            tracing::warn!(
                version = support.version().unwrap_or("unknown"),
                verified = %verified_range(),
                "{}",
                support.message()
            );
        } else {
            tracing::debug!(version = support.version().unwrap_or("none"), "claude cli");
        }
        support
    })
}

/// Run `claude --version` and return what it printed, trimmed.
///
/// Separate from [`support_of`] so that the comparison — the part with rules in it — is a pure
/// function that tests can drive without a CLI on `PATH`.
///
/// **Scrubbed, since M16.** This built a bare `Command` and did not, which was a real hole
/// rather than an omission of principle: the identical probe in `cide_app::cmd::app` *does*
/// scrub and says why. Under the AppImage this one ran a Node with the bundle's
/// `LD_LIBRARY_PATH` and `PYTHONHOME` — the ADR 0007 environment — so the version cide reported
/// came from a process launched in a way no pane is ever launched in, and on a bad day from a
/// process that failed to start at all and was reported as *not installed*.
pub fn probe(program: &Path) -> Option<String> {
    let mut command = std::process::Command::new(program);
    cide_core::child_env::scrub_command(&mut command);
    let output = command.arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    //! The comparison's own tests moved with it, to `cide_ide_mcp::protocol` — including the
    //! one that used to sit here as a tautology (it iterated `SUPPORTED_CLI` and asserted
    //! every entry read back as `Verified`, which is the constant compared against itself and
    //! cannot fail). What is left is what this module still decides.

    use super::*;

    #[test]
    fn probing_something_that_is_not_a_program_answers_missing() {
        assert_eq!(probe(Path::new("/nonexistent/claude")), None);
        assert_eq!(support_of(None), Support::Missing);
    }

    /// The re-export is the whole compatibility story for `cide-app`, so it is worth one
    /// assertion that it is the same function and not a second copy that has drifted.
    #[test]
    fn the_verdict_re_exported_here_is_the_one_beside_the_constant() {
        assert_eq!(
            support_of(Some("2.1.226 (Claude Code)")),
            cide_ide_mcp::protocol::support_of(Some("2.1.226 (Claude Code)"))
        );
        assert_eq!(verified_range(), cide_ide_mcp::protocol::verified_range());
    }
}
