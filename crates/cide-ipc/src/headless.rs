//! The non-interactive Claude lane's wire types.
//!
//! This lane is `claude -p` — commit-message generation, "explain this selection", palette
//! one-shots. It never draws a TUI, never owns a pane and never resumes: one prompt in, one
//! answer out. The interactive spawn path in `cide-app::cmd::session` has nothing to do with
//! it beyond sharing a binary.
//!
//! # The envelope these types describe was read off the real CLI, not the docs
//!
//! `claude --help` on 2.1.226 says `--output-format json` produces a *"single result"*. It
//! does not. Verified by running it:
//!
//! ```text
//! $ printf 'Reply with exactly: ok2' | claude -p --output-format json --no-session-persistence
//! [{"type":"rate_limit_event",…},{"type":"assistant",…},{"is_error":false,…,"subtype":"success",
//!  "result":"ok2","type":"result","duration_ms":4552,"total_cost_usd":0.077,…}]
//! ```
//!
//! Top level is an **array** of transcript frames, and the answer is the last frame whose
//! `type` is `"result"`. Which frames precede it varies between runs — one invocation began
//! with `{"type":"system","subtype":"init"}` and another with `{"type":"rate_limit_event"}` —
//! so nothing may key off position. [`cide_claude::headless`] parses accordingly, and accepts
//! a bare object too in case a future release makes the help text true.

use serde::{Deserialize, Serialize};
use std::fmt;
use ts_rs::TS;

/// One non-interactive request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct HeadlessRequest {
    /// The whole prompt, including any diff or selection it embeds.
    ///
    /// Delivered to the child on **stdin**, never as argv. A commit-message prompt carries a
    /// staged diff, which routinely runs to tens of kilobytes and can pass `ARG_MAX`; and a
    /// prompt beginning with `-` would be parsed as a flag. Both failures are silent in
    /// different ways, and stdin has neither.
    pub prompt: String,

    /// A model alias (`opus`, `sonnet`, `haiku`) or a full name. `None` uses the user's own
    /// default, which is the right choice for a feature they did not ask to configure.
    #[ts(optional)]
    pub model: Option<String>,

    /// A JSON Schema for structured output, passed through as `--json-schema`.
    ///
    /// When set, [`HeadlessResult::structured`] carries the parsed object; `text` still
    /// carries whatever the CLI printed, so a caller that gets malformed structure can show
    /// something rather than nothing.
    #[ts(optional, type = "unknown")]
    pub json_schema: Option<serde_json::Value>,

    /// A hard ceiling on what this one call may spend, as `--max-budget-usd`.
    ///
    /// Worth setting on anything triggered by a keystroke: a palette one-shot that goes
    /// looping is a bill the user never agreed to and no UI in this app would show them.
    #[ts(optional)]
    pub max_budget_usd: Option<f64>,
}

impl HeadlessRequest {
    /// A prompt-only request: the user's default model, no schema, no budget cap.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: None,
            json_schema: None,
            max_budget_usd: None,
        }
    }
}

/// What one non-interactive run produced.
///
/// A flat mirror of the CLI's terminal `result` frame rather than an enum over success and
/// failure. The CLI reports its own verdict in `is_error` *and still fills in the rest* —
/// a run that stopped on `error_max_turns` has partial text, a cost and a duration worth
/// showing — so splitting the shape by outcome would throw away the fields that make the
/// failure explicable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HeadlessResult {
    /// The `result` field: the final assistant text.
    pub text: String,

    /// `text` parsed as JSON, when a schema was requested and the output parses.
    #[ts(type = "unknown")]
    pub structured: Option<serde_json::Value>,

    /// The CLI's own verdict, not ours. A `false` here with a non-zero exit status is
    /// possible and is treated as failure by [`cide_claude::headless`].
    pub is_error: bool,

    /// `success`, `error_max_turns`, `error_during_execution`, … Passed through verbatim:
    /// the set is not documented and a release may add to it.
    pub subtype: String,

    /// The uuid the CLI minted for this run.
    ///
    /// Reported **even with `--no-session-persistence`**, which this lane always passes —
    /// verified against 2.1.226 by `cide-claude`'s `real_headless` test, which originally
    /// asserted the opposite and failed. The flag governs whether the transcript reaches
    /// disk, not whether the run has an id. So this is not evidence that a one-shot is
    /// resumable, and nothing should offer to resume it.
    ///
    /// `Option` because the field is not guaranteed to be in the frame, not because it is
    /// expected to be absent.
    pub session: Option<String>,

    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub num_turns: Option<u32>,
}

/// Why a non-interactive run produced no answer at all.
///
/// A tagged enum rather than a string, so the frontend branches on a variant: "the binary is
/// not installed" wants an install hint, "it timed out" wants a retry, and "it printed
/// something we could not read" wants a bug report with the head of what it printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum HeadlessError {
    /// No `claude` on `PATH`, or it could not be executed.
    NotInstalled { detail: String },
    /// The project the run was asked for is no longer open, so there is no directory to run
    /// in. Not a detail: the working directory decides which `CLAUDE.md` and which settings
    /// the CLI picks up, so guessing one would quietly answer about the wrong repository.
    NoProject { project: String },
    /// The child was still running at the deadline and was killed.
    TimedOut { seconds: u64 },
    /// The child exited without printing a `result` frame we could read.
    Exited {
        code: Option<i32>,
        /// The tail of stderr, truncated — enough to name the cause, not enough to paste a
        /// user's whole environment into a toast.
        stderr: String,
    },
    /// Output arrived but did not match the envelope this module documents.
    ///
    /// Carries the head of what was printed, because this is the variant that fires when the
    /// CLI changes shape under us and the only useful bug report is the bytes themselves.
    Malformed { detail: String, head: String },
}

impl fmt::Display for HeadlessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled { detail } => write!(f, "claude could not be started: {detail}"),
            Self::NoProject { project } => write!(f, "project {project} is not open"),
            Self::TimedOut { seconds } => write!(f, "claude did not answer within {seconds}s"),
            Self::Exited { code, stderr } => match code {
                Some(code) => write!(f, "claude exited {code}: {stderr}"),
                None => write!(f, "claude was killed by a signal: {stderr}"),
            },
            Self::Malformed { detail, head } => {
                write!(f, "could not read claude's output ({detail}): {head}")
            }
        }
    }
}

impl std::error::Error for HeadlessError {}
