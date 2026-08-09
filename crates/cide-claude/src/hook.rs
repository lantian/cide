//! Claude Code's hook events, and the frame `cide-hook` sends over the socket.
//!
//! # Provenance
//!
//! The event names and the settings shape below were read out of the shipped 2.1.226 binary,
//! which carries its own hook documentation as embedded text. That is worth stating because
//! the obvious guesses are wrong in both directions: `PreCompaction` returns zero matches
//! (the event is `PreCompact`), and there is a tenth event, [`HookEvent::PostToolBatch`],
//! that no summary of this API mentions.
//!
//! The config shape is likewise the binary's, quoted verbatim from its own docs:
//!
//! ```json
//! {"hooks": {"PostToolUse": [{"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "echo Done"}]}]}}
//! ```
//!
//! A matcher is "a tool name (`Bash`), pipe-separated list (`Edit|Write`), or empty to match
//! all".

use serde::{Deserialize, Serialize};

/// A hook point Claude Code will call out at.
///
/// Serialised with the CLI's exact spelling, because these strings are keys in a config file
/// the CLI parses — a rename here silently stops a hook firing rather than failing loudly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    /// A session began. Its payload carries the authoritative session uuid.
    SessionStart,
    /// The user submitted a prompt: the turn has started.
    UserPromptSubmit,
    /// About to run a tool.
    PreToolUse,
    /// A tool finished. Carries the paths it touched, which is the fast buffer-reload path.
    PostToolUse,
    /// A batch of tools finished.
    ///
    /// Present in 2.1.226 and absent from every summary of this API, including the plan this
    /// milestone was written from. Subscribed to for the same reason as `PostToolUse`: it is
    /// a point at which files may have changed under an open editor.
    PostToolBatch,
    /// The turn ended.
    Stop,
    /// A subagent finished. **Not** the end of the turn — see [`super::state`].
    SubagentStop,
    /// The transcript is about to be compacted.
    PreCompact,
    /// The session ended.
    SessionEnd,
    /// Something wants the user's attention — most importantly a permission prompt.
    Notification,
}

impl HookEvent {
    /// Every event cide subscribes to.
    pub const ALL: &'static [HookEvent] = &[
        Self::SessionStart,
        Self::UserPromptSubmit,
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolBatch,
        Self::Stop,
        Self::SubagentStop,
        Self::PreCompact,
        Self::SessionEnd,
        Self::Notification,
    ];

    /// The CLI's own spelling, which is the settings-file key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolBatch => "PostToolBatch",
            Self::Stop => "Stop",
            Self::SubagentStop => "SubagentStop",
            Self::PreCompact => "PreCompact",
            Self::SessionEnd => "SessionEnd",
            Self::Notification => "Notification",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|e| e.as_str() == s)
    }
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What `cide-hook` writes to the socket: one JSON object, one line.
///
/// Newline-delimited rather than length-prefixed because the writer is a shell-spawned
/// process that may die mid-write, and a truncated line is discardable by the reader whereas
/// a truncated length prefix desynchronises the stream for every later frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookFrame {
    pub event: String,
    /// The hook's own JSON payload, passed through untouched.
    ///
    /// Deliberately not parsed into a typed struct here. The payload shape varies per event
    /// and is not versioned; forwarding it whole means a CLI release that adds a field
    /// reaches the app instead of being dropped by a deserialiser that had not heard of it.
    pub payload: serde_json::Value,
}

impl HookFrame {
    pub fn new(event: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            event: event.into(),
            payload,
        }
    }

    pub fn kind(&self) -> Option<HookEvent> {
        HookEvent::parse(&self.event)
    }

    /// The session uuid the payload names, if any.
    ///
    /// `session_id` is the field the CLI uses. This is the authoritative source for a forked
    /// session's id: `--fork-session` composing with `--session-id` is unverified, so the id
    /// learned here wins over the one we asked for.
    pub fn session_id(&self) -> Option<&str> {
        self.payload.get("session_id")?.as_str()
    }

    /// File paths a tool touched, for the buffer-reload path.
    ///
    /// Both spellings are checked because the CLI's own documentation shows both
    /// `tool_input.file_path` and `tool_response.filePath` in its examples — snake_case in
    /// the input, camelCase in the response.
    pub fn touched_paths(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (obj, key) in [
            (self.payload.get("tool_input"), "file_path"),
            (self.payload.get("tool_response"), "filePath"),
        ] {
            if let Some(p) = obj.and_then(|o| o.get(key)).and_then(|v| v.as_str()) {
                out.push(p.to_owned());
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_event_round_trips_through_its_cli_spelling() {
        for e in HookEvent::ALL {
            assert_eq!(HookEvent::parse(e.as_str()), Some(*e));
        }
        assert_eq!(HookEvent::ALL.len(), 10);
    }

    #[test]
    fn the_spellings_are_the_clis_and_not_the_plausible_ones() {
        // Both of these were wrong in the milestone plan this was built from, and both fail
        // silently: a hook keyed on a name the CLI does not know simply never fires.
        assert!(HookEvent::parse("PreCompaction").is_none());
        assert_eq!(HookEvent::PreCompact.as_str(), "PreCompact");
        assert!(HookEvent::parse("PostToolBatch").is_some());
    }

    #[test]
    fn a_frame_finds_the_session_id_the_cli_reports() {
        let f = HookFrame::new(
            "SessionStart",
            json!({"session_id": "abc-123", "cwd": "/w"}),
        );
        assert_eq!(f.session_id(), Some("abc-123"));
        assert_eq!(f.kind(), Some(HookEvent::SessionStart));
    }

    #[test]
    fn touched_paths_reads_both_spellings() {
        let f = HookFrame::new(
            "PostToolUse",
            json!({
                "tool_input": {"file_path": "/w/a.rs"},
                "tool_response": {"filePath": "/w/b.rs"}
            }),
        );
        assert_eq!(f.touched_paths(), vec!["/w/a.rs", "/w/b.rs"]);
    }

    #[test]
    fn an_unknown_event_is_carried_rather_than_rejected() {
        // A CLI release that adds a hook should reach the app as an unrecognised frame, not
        // fail to parse. Dropping it at the edge would make a new event invisible.
        let f: HookFrame =
            serde_json::from_str(r#"{"event":"SomethingNew","payload":{"x":1}}"#).expect("parses");
        assert_eq!(f.kind(), None);
        assert_eq!(f.payload["x"], 1);
    }
}
