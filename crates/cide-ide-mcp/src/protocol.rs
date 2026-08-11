//! The Claude Code IDE-integration protocol, as the shipped binary actually speaks it.
//!
//! # Provenance, and why this file is written the way it is
//!
//! None of this is documented. Every constant below was read out of the `claude` binary on
//! this machine and re-verified against 2.1.226 before the code was written. There is **no
//! version field** anywhere — not in the lockfile, not in the MCP handshake — so a build
//! cannot negotiate, feature-detect, or degrade gracefully. It can only be right or wrong.
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
//! Verified against 2.1.224, 2.1.226 and 2.1.227. [`SUPPORTED_CLI`] records that, and the app
//! warns outside it rather than failing — a protocol change should degrade the diff view, never
//! break the terminal.
//!
//! 2.1.227 was added on evidence rather than optimism, and the distinction matters because the
//! same release **did** change something: it began rejecting `--resume <id> --session-id <new>`,
//! which broke resume until `cide_claude::session` was corrected. So the protocol was re-checked
//! rather than assumed stable — `server::live::a_real_claude_finds_us_and_completes_the_handshake`
//! was run against the installed 2.1.227 and passed, meaning a real CLI read our lockfile, chose
//! the WebSocket transport, sent the auth header and completed the MCP handshake. That is the
//! whole discovery path end to end, which no amount of grepping the binary establishes.

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
pub const SUPPORTED_CLI: &[&str] = &["2.1.224", "2.1.226", "2.1.227"];

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
    /// Language-server diagnostics. Answers empty in v1 — no language server ships.
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
