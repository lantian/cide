//! The Claude Code IDE-integration protocol, as the shipped binary actually speaks it.
//!
//! # Provenance, and why this file is written the way it is
//!
//! None of this is documented. Every constant below was read out of the `claude` binary on
//! this machine and re-verified against 2.1.226 before the code was written. There is no
//! *protocol* version anywhere — not in the lockfile, not in the MCP handshake — so a build
//! cannot negotiate, feature-detect, or degrade gracefully. It can only be right or wrong.
//!
//! **One correction to a claim this file used to make, because it changed what is possible.**
//! The handshake *does* carry a version: the CLI's `initialize` names itself in
//! `clientInfo.version`, captured byte-for-byte from a real 2.1.226 session in
//! `tests/real_cli.rs`. It is not a protocol version and none of the paragraph above is
//! weakened by it — nothing can be negotiated with it — but it is the version of the binary
//! that **actually connected**, self-reported, and that is strictly better evidence than
//! `claude --version` on `PATH`, which describes whichever binary answered a probe latched at
//! the first Claude spawn of the process. `crate::server` records it and the application
//! keeps the last one; see `ServerEvent::Connected`.
//!
//! That is why the whole surface lives in one module: when a CLI release changes something,
//! the diff is confined here rather than scattered through the server.
//!
//! ## What is actually implemented
//!
//! Five tools and three notifications. An earlier plan for this milestone listed twelve
//! tools — `getCurrentSelection`, `getOpenEditors`, `saveDocument` and friends — taken from
//! secondary sources. Searching 2.1.226 for them returns **nothing**: they do not exist, and
//! serving them would be seven handlers no client will ever call.
//!
//! ## Known-good range
//!
//! Verified against 2.1.224, 2.1.226, 2.1.227 and 2.1.231. [`SUPPORTED_CLI`] records that, and
//! the app warns outside it rather than failing — a protocol change should degrade the diff
//! view, never break the terminal.
//!
//! 2.1.227 was added on evidence rather than optimism, and the distinction matters because the
//! same release **did** change something: it began rejecting `--resume <id> --session-id <new>`,
//! which broke resume until `cide_claude::session` was corrected. So the protocol was re-checked
//! rather than assumed stable — `server::live::a_real_claude_finds_us_and_completes_the_handshake`
//! was run against the installed 2.1.227 and passed, meaning a real CLI read our lockfile, chose
//! the WebSocket transport, sent the auth header and completed the MCP handshake. That is the
//! whole discovery path end to end, which no amount of grepping the binary establishes.
//!
//! **2.1.231 was added by the machinery that now exists to produce exactly this record**, and
//! it is worth writing down what that run said, because it is the first entry in this list
//! whose evidence is reproducible rather than remembered. `cargo xtask verify-cli`, against
//! the CLI installed on the development machine:
//!
//! ```text
//! xtask: using /home/u/.local/bin/claude
//! xtask: `claude --version` says 2.1.231 (Claude Code)
//! a real claude connected and named pid 2320201
//! the CLI that connected calls itself 2.1.231
//! VERIFY-CLI: handshake=ok version=2.1.231 range=2.1.224–2.1.227 verdict=newer
//! ```
//!
//! That is the *newer* cell of the 2×2 in `tests/real_cli.rs`: the protocol still works, and
//! nothing recorded it. The failure names the one-line edit, this list is that edit, and the
//! version the run reports is `clientInfo.version` — self-reported by the process that
//! actually completed the handshake, not by a probe of `PATH`.

use serde::{Deserialize, Serialize};

/// CLI versions this protocol description was checked against.
///
/// Recorded rather than enforced. The CLI self-updates — 2.1.221 through 2.1.227 inside a
/// fortnight on this machine — so refusing to run outside the range would break the app far
/// more often than the protocol actually changes.
///
/// Left stale, this is worse than useless: it warns on every launch about a version that is
/// fine, which is how a warning stops being read before the launch that should have been
/// warned about.
///
/// # What puts a version in this list
///
/// Not a hand edit on optimism. `cargo xtask verify-cli` runs
/// `tests/real_cli.rs::the_installed_cli_is_one_this_build_has_checked`, which spawns the real
/// binary, waits for it to complete the entire discovery-plus-handshake chain — lockfile,
/// `transport: "ws"`, auth header, subprotocol, our `initialize` reply accepted, `tools/list`,
/// `ide_connected` — and *then* compares the version that did it against this list. A version
/// is appended with that run as its evidence.
///
/// Until that test existed, this constant was the mirror image of the defect this project
/// keeps finding: not "implemented and nothing calls it" but **recorded and nothing produces
/// the record**. Two tests looked as though they checked it and were tautologies — one
/// iterated the list and asserted each entry read as `Verified`, which is a constant compared
/// against itself, and the other compared a latched value against a second read of the same
/// latch. Neither could fail, on any machine, for any CLI.
///
/// **Ascending, without duplicates**, and [`tests::the_recorded_range_is_ordered`] holds it
/// there: [`verified_range`] renders `first()`–`last()`, so a version appended out of order
/// prints a backwards range at the user, and [`support_of`] folds min/max and would silently
/// disagree with what the screen said.
pub const SUPPORTED_CLI: &[&str] = &["2.1.224", "2.1.226", "2.1.227", "2.1.231"];

// --- reading a version against that list ---------------------------------------------------
//
// This arithmetic used to live in `cide_claude::version`, one crate away from the constant it
// compares against. It moved here on this module's own argument — "the whole surface lives in
// one module, so when a CLI release changes something the diff is confined here" — and for one
// concrete reason: a test *inside this crate* has to be able to judge the version of a real
// CLI it just handshook with, and reaching back to `cide-claude` for it would be a
// dev-dependency cycle pointing the wrong way up the graph.
//
// `cide_claude::version` keeps what is genuinely its own: running `claude --version` and
// latching the answer for the process. It re-exports these, so no call site moved.

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

/// The verified range, rendered for a human. `"2.1.224–2.1.227"`, or a single version.
pub fn verified_range() -> String {
    match (SUPPORTED_CLI.first(), SUPPORTED_CLI.last()) {
        (Some(low), Some(high)) if low != high => format!("{low}–{high}"),
        (Some(only), _) => (*only).to_string(),
        _ => "no verified version".to_string(),
    }
}

/// Compare a reported version string against [`SUPPORTED_CLI`].
///
/// `reported` is the whole line the CLI printed — `"2.1.226 (Claude Code)"` — not a cleaned
/// version, because that is what `claude --version` gives and what `Capabilities` already
/// carries. It is also what `clientInfo.version` gives, without the parenthetical.
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
pub type Version = (u32, u32, u32);

/// Pull `MAJOR.MINOR.PATCH` out of a `claude --version` line.
///
/// Tolerant of what follows, because the real output is `"2.1.226 (Claude Code)"` and the
/// parenthetical is not ours to depend on. Intolerant of what precedes it: a leading token
/// would mean the format changed enough that guessing is not safe.
pub fn parse(reported: &str) -> Option<Version> {
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

/// The header carrying the lockfile's `authToken`.
pub const AUTH_HEADER: &str = "x-claude-code-ide-authorization";

/// The WebSocket subprotocol the CLI asks for.
pub const SUBPROTOCOL: &str = "mcp";

/// Tools the IDE serves. Anything not here is answered as unknown.
pub mod tool {
    /// Show a proposed edit and **block the agent's turn** until a human answers.
    pub const OPEN_DIFF: &str = "openDiff";
    /// Close one diff tab by name.
    pub const CLOSE_TAB: &str = "close_tab";
    /// Close every diff tab. Sent by the CLI's own `beforeExit`.
    pub const CLOSE_ALL_DIFF_TABS: &str = "closeAllDiffTabs";
    /// Language-server diagnostics, read from whatever
    /// [`crate::DiagnosticSource`](crate::DiagnosticSource) the app installed.
    ///
    /// Answers `[]` when nothing installed one, which is still a true sentence — see
    /// [`crate::tools::get_diagnostics`].
    pub const GET_DIAGNOSTICS: &str = "getDiagnostics";
    /// Reveal a file, optionally at a selection.
    pub const OPEN_FILE: &str = "openFile";

    pub const ALL: &[&str] = &[
        OPEN_DIFF,
        CLOSE_TAB,
        CLOSE_ALL_DIFF_TABS,
        GET_DIAGNOSTICS,
        OPEN_FILE,
    ];
}

/// Notifications, in both directions.
pub mod notify {
    /// Sent **by the CLI** on connect, carrying its pid.
    ///
    /// The pid is the join key: `cide-pty` already knows each child's pid, so one WebSocket
    /// connection maps to exactly one pane. Attributing by `tab_name` or by cwd would fail
    /// the moment a project has two Claude panes, which is the normal case here.
    pub const IDE_CONNECTED: &str = "ide_connected";
    /// Sent **by the IDE** when the editor selection moves.
    pub const SELECTION_CHANGED: &str = "selection_changed";
    /// Sent **by the IDE** to put a file reference into the prompt.
    pub const AT_MENTIONED: &str = "at_mentioned";
}

/// The three answers `openDiff` may return.
///
/// These are sentinel *strings* in the tool result's content, not a status field. The CLI
/// reads `content[0].text` for the outcome and, for an accepted-with-edits diff,
/// `content[1].text` for the final file — which is what lets a user edit inside our diff
/// view and have their version be the one Claude writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOutcome {
    /// Accepted, possibly after the user edited it. Carries the final contents.
    Saved { contents: String },
    /// Accepted exactly as proposed, by closing the tab.
    TabClosed,
    /// Rejected.
    Rejected,
}

impl DiffOutcome {
    pub const SAVED: &'static str = "FILE_SAVED";
    pub const REJECTED: &'static str = "DIFF_REJECTED";
    pub const TAB_CLOSED: &'static str = "TAB_CLOSED";

    /// The `content` array of the MCP tool result.
    pub fn to_content(&self) -> Vec<Content> {
        match self {
            // Two entries, in this order. The CLI reads the final text from `content[1]`.
            Self::Saved { contents } => {
                vec![Content::text(Self::SAVED), Content::text(contents.clone())]
            }
            Self::TabClosed => vec![Content::text(Self::TAB_CLOSED)],
            Self::Rejected => vec![Content::text(Self::REJECTED)],
        }
    }
}

/// One entry of an MCP tool result's `content` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Content {
    Text { text: String },
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

/// Arguments to `openDiff`.
///
/// Snake_case on the wire, unlike the camelCase tool names. That inconsistency is the
/// protocol's, not ours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDiffParams {
    pub old_file_path: String,
    pub new_file_path: String,
    pub new_file_contents: String,
    /// Identifies the tab for a later `close_tab`.
    pub tab_name: String,
}

/// Arguments to `close_tab`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseTabParams {
    pub tab_name: String,
}

/// The payload of an `ide_connected` notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeConnected {
    pub pid: u32,
}

// --- `getDiagnostics`' answer -------------------------------------------------------------
//
// **LSP's shapes, not cide's**, and that is the whole reason they live here rather than being
// reused from `cide-ipc`: the CLI parses `content[0].text` as a JSON array of
// `{uri, diagnostics}` where each diagnostic is an LSP one — 0-based `range`, numeric
// `severity`. Handing it `cide_ipc::Diagnostic` would be handing it 1-based lines and a string
// severity, which it would read as garbage or drop.
//
// This module's header already states the rule for exactly this case: when a CLI release changes
// something, the diff is confined here.

/// A 0-based LSP position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

/// One diagnostic, in the shape the CLI expects rather than the shape cide stores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnostic {
    pub range: LspRange,
    /// LSP's own numbering: 1 Error, 2 Warning, 3 Information, 4 Hint. Not cide's names.
    pub severity: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
}

/// One file's worth, as the CLI reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UriDiagnostics {
    pub uri: String,
    pub diagnostics: Vec<LspDiagnostic>,
}

/// The payload the IDE sends on `selection_changed`.
///
/// Line numbers are **0-based on the wire**, which is the opposite of everything a user
/// sees. Converting at this boundary keeps the rest of the codebase 1-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionChanged {
    pub file_path: String,
    pub text: String,
    /// 0-based, inclusive.
    pub start_line: u32,
    /// 0-based, inclusive.
    pub end_line: u32,
}

/// The payload the IDE sends on `at_mentioned`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtMentioned {
    pub file_path: String,
    /// 0-based, inclusive. `None` mentions the whole file.
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
}

// --- JSON-RPC 2.0 envelopes -------------------------------------------------------------

/// An inbound message. Requests carry an id; notifications do not.
#[derive(Debug, Clone, Deserialize)]
pub struct Incoming {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

impl Incoming {
    /// A message with no id expects no reply, and replying to one is a protocol error.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

/// JSON-RPC's own code for a method the server does not implement.
pub const METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC's code for a request whose params do not fit the method.
pub const INVALID_PARAMS: i32 = -32602;

#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: serde_json::Value,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accepted_diff_carries_the_final_contents_second() {
        let content = DiffOutcome::Saved {
            contents: "edited".into(),
        }
        .to_content();

        // Order is the protocol's, not a preference: the CLI reads `content[1].text` for the
        // file it is about to write. Swapping these writes the sentinel into the user's file.
        assert_eq!(content[0], Content::text("FILE_SAVED"));
        assert_eq!(content[1], Content::text("edited"));
    }

    #[test]
    fn the_other_two_outcomes_carry_one_entry() {
        assert_eq!(DiffOutcome::Rejected.to_content().len(), 1);
        assert_eq!(DiffOutcome::TabClosed.to_content().len(), 1);
    }

    // --- the recorded range ------------------------------------------------------------------

    #[test]
    fn the_real_version_line_parses() {
        // What 2.1.226 actually prints, and what `Capabilities::claude_version` carries.
        assert_eq!(parse("2.1.226 (Claude Code)"), Some((2, 1, 226)));
        assert_eq!(parse("  2.1.224\n"), Some((2, 1, 224)));
        assert_eq!(parse("2.1.226-beta.1"), Some((2, 1, 226)));
        // And what `clientInfo.version` carries, which has no parenthetical.
        assert_eq!(parse("2.1.226"), Some((2, 1, 226)));
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

    /// **This replaces a tautology.**
    ///
    /// What was here iterated `SUPPORTED_CLI` and asserted that every entry read back as
    /// `Verified`. That is the constant compared against itself through a function whose
    /// endpoints come from the same constant: it is true for *any* list of parseable
    /// versions, in any order, and it cannot fail on any machine for any CLI. Its doc comment
    /// claimed it "fails when somebody adds a version in a shape this cannot read", which was
    /// the one real thing in it — so that is what is asserted here, plus the property it was
    /// silently not checking.
    ///
    /// The ordering half is a live defect, not a tidiness rule. [`verified_range`] renders
    /// `first()`–`last()` while [`support_of`] folds min/max, so appending `2.1.223` to the
    /// end of this list would put `"2.1.224–2.1.223"` on the Settings screen and in every log
    /// line, while the comparison behind it went on using 2.1.223–2.1.227. The two would
    /// disagree and only one of them is visible.
    #[test]
    fn the_recorded_range_is_ordered() {
        assert!(
            !SUPPORTED_CLI.is_empty(),
            "a build with no verified version"
        );

        let mut previous: Option<Version> = None;
        for entry in SUPPORTED_CLI {
            let version = parse(entry).unwrap_or_else(|| {
                panic!("{entry:?} is in SUPPORTED_CLI and `parse` cannot read it")
            });
            if let Some(previous) = previous {
                assert!(
                    version > previous,
                    "SUPPORTED_CLI must be ascending and duplicate-free: {entry:?} does not \
                     follow {previous:?}. `verified_range` prints first–last while \
                     `support_of` compares min–max, so an out-of-order entry makes the screen \
                     and the check disagree."
                );
            }
            previous = Some(version);
        }

        // And the rendered range therefore names the two ends it actually compares against.
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
    fn a_message_without_an_id_is_a_notification() {
        let n: Incoming = serde_json::from_str(r#"{"method":"ide_connected","params":{"pid":7}}"#)
            .expect("parses");
        assert!(n.is_notification());

        let r: Incoming =
            serde_json::from_str(r#"{"id":1,"method":"tools/list","params":{}}"#).expect("parses");
        assert!(!r.is_notification());
    }
}
