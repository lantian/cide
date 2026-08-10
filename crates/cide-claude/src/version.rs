//! Which `claude` is on `PATH`, and whether the IDE protocol was ever checked against it.
//!
//! # Why this is worth a module
//!
//! [`cide_ide_mcp::protocol`] is a transcription of an undocumented wire protocol, read out of
//! a specific build of the CLI. There is no version field anywhere in it — not in the
//! lockfile, not in the MCP handshake — so a running cide cannot negotiate, feature-detect or
//! degrade. It can only be right or wrong, and it finds out by breaking.
//!
//! The CLI also updates itself: 2.1.221 through 2.1.226 inside a fortnight on the development
//! machine. So the interesting question is never "does this version work" — nobody can answer
//! that from inside — but "is this a version anybody checked". `SUPPORTED_CLI` is the record
//! of what was checked, and until this module existed nothing read it. A warning here is the
//! earliest signal available that the protocol may have drifted, and it costs one line in a
//! log against a bug report that would otherwise begin with "the diff view stopped working".
//!
//! # Recorded, never enforced
//!
//! Refusing to run outside the range would break the app roughly every fortnight to protect
//! a feature that mostly keeps working across releases. The rule from `protocol` holds: a
//! protocol change degrades the diff view and never breaks the terminal. So the strongest
//! thing here is a warning, and the strongest thing a *user* sees is a note in Settings.

use std::path::Path;
use std::sync::OnceLock;

use cide_ide_mcp::protocol::SUPPORTED_CLI;

/// What the installed CLI is, relative to what the protocol was verified against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// Inside the verified range.
    Verified { version: String },
    /// Older than the oldest version this build was checked against.
    Older { version: String },
    /// Newer than the newest. The common case, and the one worth a warning: the CLI updates
    /// itself and this build does not.
    Newer { version: String },
    /// A version string that does not look like `MAJOR.MINOR.PATCH`. Not treated as a
    /// failure — the CLI is free to print what it likes, and guessing would be worse than
    /// saying we could not tell.
    Unreadable { reported: String },
    /// Nothing answered `--version`.
    Missing,
}

impl Support {
    /// Whether the user should be told. False for [`Support::Missing`]: "claude is not
    /// installed" is a different message, already carried by `Capabilities::claude_version`,
    /// and repeating it as a protocol warning would be noise on every machine without the CLI.
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            Self::Older { .. } | Self::Newer { .. } | Self::Unreadable { .. }
        )
    }

    /// The version as reported, when there was one.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Verified { version } | Self::Older { version } | Self::Newer { version } => {
                Some(version)
            }
            Self::Unreadable { reported } => Some(reported),
            Self::Missing => None,
        }
    }

    /// One sentence, for a log line and for the Settings note. Written here rather than in the
    /// frontend so both say the same thing and neither has to know the range.
    pub fn message(&self) -> String {
        let range = verified_range();
        match self {
            Self::Verified { .. } => {
                format!("the IDE protocol was verified against {range}")
            }
            Self::Older { version } => format!(
                "claude {version} is older than the {range} this build's IDE protocol was \
                 verified against; diffs and @-mentions may not work"
            ),
            Self::Newer { version } => format!(
                "claude {version} is newer than the {range} this build's IDE protocol was \
                 verified against; if diffs stop appearing in the editor, this is the first \
                 thing to suspect"
            ),
            Self::Unreadable { reported } => format!(
                "could not read a version out of `claude --version` ({reported:?}), so it \
                 cannot be checked against the verified {range}"
            ),
            Self::Missing => "claude is not on PATH".to_string(),
        }
    }
}

/// The verified range, rendered for a human. `"2.1.224–2.1.226"`, or a single version.
pub fn verified_range() -> String {
    match (SUPPORTED_CLI.first(), SUPPORTED_CLI.last()) {
        (Some(low), Some(high)) if low != high => format!("{low}–{high}"),
        (Some(only), _) => (*only).to_string(),
        _ => "no verified version".to_string(),
    }
}

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
pub fn probe(program: &Path) -> Option<String> {
    let output = std::process::Command::new(program)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Compare a reported version string against [`SUPPORTED_CLI`].
///
/// `reported` is the whole line the CLI printed — `"2.1.226 (Claude Code)"` — not a cleaned
/// version, because that is what `claude --version` gives and what `Capabilities` already
/// carries.
pub fn support_of(reported: Option<&str>) -> Support {
    let Some(reported) = reported else {
        return Support::Missing;
    };
    let Some(version) = parse(reported) else {
        return Support::Unreadable {
            reported: reported.to_string(),
        };
    };

    // Endpoints of the recorded list rather than membership. `SUPPORTED_CLI` names the
    // versions that were actually run against, and treating anything between them as
    // unverified would warn about 2.1.225 — a release nobody has any reason to think differs
    // from the two either side of it. The signal this exists to give is "the CLI has moved
    // past what anyone checked", and that is an endpoint question.
    let mut bounds = SUPPORTED_CLI.iter().filter_map(|v| parse(v));
    let Some(first) = bounds.next() else {
        return Support::Unreadable {
            reported: reported.to_string(),
        };
    };
    let (low, high) = bounds.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));

    if version < low {
        Support::Older {
            version: rendered(version),
        }
    } else if version > high {
        Support::Newer {
            version: rendered(version),
        }
    } else {
        Support::Verified {
            version: rendered(version),
        }
    }
}

/// A version as three numbers, so `2.1.9` sorts below `2.1.226`.
///
/// A tuple rather than a semver dependency: this compares two strings of the same shape from
/// the same producer, and the tuple's ordering is the derived lexicographic one, which is
/// exactly right for it. String comparison is what would be wrong — `"2.1.9" > "2.1.226"`.
type Version = (u32, u32, u32);

/// Pull `MAJOR.MINOR.PATCH` out of a `claude --version` line.
///
/// Tolerant of what follows, because the real output is `"2.1.226 (Claude Code)"` and the
/// parenthetical is not ours to depend on. Intolerant of what precedes it: a leading token
/// would mean the format changed enough that guessing is not safe.
fn parse(reported: &str) -> Option<Version> {
    // No `trim()` first: `split_whitespace` already skips leading whitespace, and clippy
    // rejects the pair as redundant.
    let head = reported.split_whitespace().next()?;
    let mut parts = head.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    // `226-beta.1` and the like: take the leading digits and ignore any pre-release tail
    // rather than refusing to read the version at all.
    let patch_field = parts.next()?;
    let digits: String = patch_field
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let patch = digits.parse().ok()?;
    Some((major, minor, patch))
}

fn rendered((major, minor, patch): Version) -> String {
    format!("{major}.{minor}.{patch}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_version_line_parses() {
        // What 2.1.226 actually prints, and what `Capabilities::claude_version` carries.
        assert_eq!(parse("2.1.226 (Claude Code)"), Some((2, 1, 226)));
        assert_eq!(parse("  2.1.224\n"), Some((2, 1, 224)));
        assert_eq!(parse("2.1.226-beta.1"), Some((2, 1, 226)));
    }

    #[test]
    fn a_line_that_is_not_a_version_is_not_guessed_at() {
        for odd in ["claude 2.1.226", "", "unknown", "2.1", "v2.1.226"] {
            assert_eq!(parse(odd), None, "{odd:?} was read as a version");
        }
    }

    #[test]
    fn patch_numbers_compare_numerically() {
        // The bug a string comparison ships with, and the reason `Version` is a tuple:
        // "2.1.9" sorts *above* "2.1.226" as text, so every user on a supported build would
        // be warned that their CLI was too new.
        assert!(parse("2.1.9").unwrap() < parse("2.1.226").unwrap());
        assert!(parse("2.2.0").unwrap() > parse("2.1.226").unwrap());
        assert!(parse("10.0.0").unwrap() > parse("9.9.9").unwrap());
    }

    #[test]
    fn every_version_in_the_recorded_range_is_verified() {
        // Asserted against the constant rather than against literals: this is the test that
        // fails when somebody adds a version to `SUPPORTED_CLI` in a shape this cannot read.
        for version in SUPPORTED_CLI {
            let support = support_of(Some(version));
            assert!(
                matches!(support, Support::Verified { .. }),
                "{version} is in SUPPORTED_CLI and reads as {support:?}"
            );
            assert!(!support.is_warning());
        }
    }

    #[test]
    fn a_version_between_the_endpoints_is_not_warned_about() {
        // 2.1.225 exists and sits between the two that were checked. Warning about it would
        // train the user to ignore the warning that matters.
        assert!(matches!(
            support_of(Some("2.1.225 (Claude Code)")),
            Support::Verified { .. }
        ));
    }

    #[test]
    fn a_newer_cli_is_the_warning_this_module_exists_for() {
        let support = support_of(Some("2.4.0 (Claude Code)"));
        assert_eq!(
            support,
            Support::Newer {
                version: "2.4.0".into()
            }
        );
        assert!(support.is_warning());
        // The message has to name both numbers or it is unactionable in a bug report.
        let message = support.message();
        assert!(message.contains("2.4.0"), "{message}");
        assert!(
            message.contains(SUPPORTED_CLI.last().expect("a range")),
            "{message}"
        );
    }

    #[test]
    fn an_older_cli_warns_too() {
        let support = support_of(Some("2.0.1"));
        assert_eq!(
            support,
            Support::Older {
                version: "2.0.1".into()
            }
        );
        assert!(support.is_warning());
    }

    #[test]
    fn an_unreadable_version_warns_but_a_missing_one_does_not() {
        // Different failures with different remedies. "No claude on PATH" is already said, in
        // plainer words, by the Settings screen's own heading; saying it again as a protocol
        // warning would put a warning on every machine that has not installed the CLI yet.
        assert!(support_of(Some("Claude Code v-next")).is_warning());
        assert_eq!(support_of(None), Support::Missing);
        assert!(!support_of(None).is_warning());
    }

    #[test]
    fn the_range_reads_as_a_range() {
        let range = verified_range();
        assert!(
            range.contains(SUPPORTED_CLI.first().expect("a range")),
            "{range}"
        );
        assert!(
            range.contains(SUPPORTED_CLI.last().expect("a range")),
            "{range}"
        );
    }

    #[test]
    fn probing_something_that_is_not_a_program_answers_missing() {
        assert_eq!(probe(Path::new("/nonexistent/claude")), None);
        assert_eq!(support_of(None), Support::Missing);
    }
}
