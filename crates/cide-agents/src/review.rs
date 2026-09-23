//! The merge-request review vocabulary: what an agent reviewing a GitLab MR may call. (M85)
//!
//! # A tool set of its own, not six more task tools
//!
//! A review run is served **these and nothing else** — `cide_app::agent_rpc`'s `Scope::Review`.
//! It has no task, no worktree and no business on the board, and the one thing it produces is a
//! list of findings the user will read before any of them leaves the machine. Serving it the
//! tracker would invite it to file its findings as tasks; serving the orchestration tools would
//! let a reviewer dispatch developers. The line is drawn the way the run/pane line is: by which
//! names a connection lists, checked again at every call.
//!
//! # There is no publish tool, and that is the feature
//!
//! Every write here is a *draft* in cide's own store (`cide_gitlab::drafts`). Posting to GitLab
//! is a button in the MR panel, pressed by a person who has read the draft. An agent that could
//! publish would make the review panel an after-the-fact log of what a model already said in
//! public under the user's name.
//!
//! # Why the seam is a trait
//!
//! For `tools.rs`' reason: this crate reads no disk and holds no network client, and the GitLab
//! service lives in `cide-app`. [`ReviewSink`] is what `cide-app` implements over it; the
//! argument parsing, the refusals worded for a model, and the schemas live here where they can be
//! tested without an app.
use crate::harness::Harness;
use crate::tools::ToolResult;
use cide_ipc::gitlab::{GitLabDraft, GitLabSeverity, GitLabSide};
use serde_json::{Value, json};

pub mod tool {
    /// The MR: title, description, branches, SHAs, the checkout, changed files, existing threads.
    pub const MR_INFO: &str = "cide_mr_info";
    /// The unified diff of one changed file, or of all of them.
    pub const MR_DIFF: &str = "cide_mr_diff";
    /// Write one finding as a local draft, with its severity.
    pub const MR_DRAFT_COMMENT: &str = "cide_mr_draft_comment";
    /// The drafts on this MR so far, from every reviewer.
    pub const MR_DRAFTS: &str = "cide_mr_drafts";
    /// Change the body or severity of a draft this run wrote.
    pub const MR_DRAFT_EDIT: &str = "cide_mr_draft_edit";
    /// Drop a draft this run wrote.
    pub const MR_DRAFT_DISCARD: &str = "cide_mr_draft_discard";

    /// Read → write, the order a model skims — `tools.rs`' rule for [`crate::tools::tool::ALL`].
    pub const REVIEW: &[&str] = &[
        MR_INFO,
        MR_DIFF,
        MR_DRAFT_COMMENT,
        MR_DRAFTS,
        MR_DRAFT_EDIT,
        MR_DRAFT_DISCARD,
    ];
}

/// One finding as the agent asked for it. The GitLab position is the sink's to compute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRequest {
    pub severity: GitLabSeverity,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub side: GitLabSide,
}

/// Where the review tools read and write. One sink is one review, seen by one run.
pub trait ReviewSink {
    /// The MR summary, already shaped for a model to read.
    fn info(&self) -> Result<Value, String>;
    /// Unified diff text for `path`, or every changed file when `None`.
    fn diff(&self, path: Option<&str>) -> Result<String, String>;
    fn draft(&self, request: DraftRequest) -> Result<GitLabDraft, String>;
    fn drafts(&self) -> Result<Vec<GitLabDraft>, String>;
    /// Limited to this run's own drafts — enforced by the sink, which knows the run.
    fn edit(
        &self,
        draft: &str,
        body: Option<String>,
        severity: Option<GitLabSeverity>,
    ) -> Result<GitLabDraft, String>;
    fn discard(&self, draft: &str) -> Result<(), String>;
}

pub fn descriptors() -> Vec<Value> {
    tool::REVIEW
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "description": description(name),
                "inputSchema": input_schema(name),
            })
        })
        .collect()
}

pub fn description(name: &str) -> &'static str {
    match name {
        tool::MR_INFO => {
            "Read the merge request under review: title, description, source and target \
             branches, the base and head commit SHAs, the directory the MR's head is checked out \
             in, every changed file (new, deleted, renamed), and the discussion threads already \
             on it — read those so you do not repeat a point somebody has made. Call this first."
        }
        tool::MR_DIFF => {
            "The unified diff of one changed file (`path`), or of every changed file when `path` \
             is left out. Line numbers in the hunk headers are the ones cide_mr_draft_comment \
             takes: new-side numbers for added and context lines, old-side for removed ones."
        }
        tool::MR_DRAFT_COMMENT => {
            "Record ONE review finding as a draft comment. Drafts stay on this machine: nothing is \
             posted to GitLab — the user reads every draft and publishes the ones they agree \
             with. Give `severity` (critical: a bug, data loss, security hole or broken build — \
             must be fixed before merging; major: wrong behaviour or a serious design problem — \
             should be fixed; minor: a real but small problem; suggestion: optional improvement, \
             style or readability). Do not write the severity into `body`; it is shown beside the \
             comment. Anchor it with `path` and `line`, where `line` is on the new side (added or \
             unchanged lines) unless `side` is \"old\" for a removed line; the line must be inside \
             the diff (a changed line or a context line around one). Leave out `path` and `line` \
             for a general comment on the whole MR. One issue per draft; say what is wrong, why, \
             and what to do instead. Returns the draft id."
        }
        tool::MR_DRAFTS => {
            "List the draft comments on this MR so far, with id, severity, file, line and body — \
             including drafts from other reviewers and ones the user edited."
        }
        tool::MR_DRAFT_EDIT => {
            "Change the `body` and/or `severity` of a draft you wrote, by `draft` id. A draft \
             written by another reviewer cannot be changed."
        }
        tool::MR_DRAFT_DISCARD => {
            "Delete a draft you wrote, by `draft` id — for a finding that turned out to be wrong."
        }
        _ => "",
    }
}

pub fn input_schema(name: &str) -> Value {
    let severity: Vec<&str> = GitLabSeverity::ALL.iter().map(|s| s.as_str()).collect();
    match name {
        tool::MR_INFO | tool::MR_DRAFTS => {
            json!({ "type": "object", "properties": {}, "additionalProperties": false })
        }
        tool::MR_DIFF => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "A changed file's path (new or old). Omit for the whole MR." }
            },
            "additionalProperties": false
        }),
        tool::MR_DRAFT_COMMENT => json!({
            "type": "object",
            "properties": {
                "severity": { "type": "string", "enum": severity },
                "body": { "type": "string", "description": "The comment, in GitLab Markdown. No severity label." },
                "path": { "type": "string", "description": "Repository-relative path of a changed file. Omit for a general comment." },
                "line": { "type": "integer", "minimum": 1 },
                "side": { "type": "string", "enum": ["new", "old"], "default": "new" }
            },
            "required": ["severity", "body"],
            "additionalProperties": false
        }),
        tool::MR_DRAFT_EDIT => json!({
            "type": "object",
            "properties": {
                "draft": { "type": "string" },
                "body": { "type": "string" },
                "severity": { "type": "string", "enum": severity }
            },
            "required": ["draft"],
            "additionalProperties": false
        }),
        tool::MR_DRAFT_DISCARD => json!({
            "type": "object",
            "properties": { "draft": { "type": "string" } },
            "required": ["draft"],
            "additionalProperties": false
        }),
        _ => json!({ "type": "object" }),
    }
}

/// Run one review tool. `None` for a name that is not one — the caller's `no such tool`.
pub fn dispatch(name: &str, args: &Value, sink: &dyn ReviewSink) -> Option<ToolResult> {
    let result = match name {
        tool::MR_INFO => sink.info().map(|v| pretty(&v)),
        tool::MR_DIFF => optional_str(args, "path").and_then(|path| sink.diff(path.as_deref())),
        tool::MR_DRAFT_COMMENT => draft_request(args)
            .and_then(|request| sink.draft(request))
            .map(|d| {
                format!(
                    "Draft {} recorded ({}{}). It is not published; the user will review it.",
                    d.id,
                    d.severity.as_str(),
                    match (&d.path, d.line) {
                        (Some(path), Some(line)) => format!(", {path}:{line}"),
                        _ => ", general comment".into(),
                    }
                )
            }),
        tool::MR_DRAFTS => sink.drafts().map(|drafts| {
            if drafts.is_empty() {
                return "No drafts yet.".into();
            }
            pretty(&json!(
                drafts
                    .iter()
                    .map(|d| json!({
                        "draft": d.id,
                        "severity": d.severity.as_str(),
                        "path": d.path,
                        "line": d.line,
                        "side": d.side,
                        "author": d.author.label,
                        "body": d.body,
                    }))
                    .collect::<Vec<_>>()
            ))
        }),
        tool::MR_DRAFT_EDIT => required_str(args, "draft").and_then(|draft| {
            let body = optional_str(args, "body")?;
            let severity = optional_str(args, "severity")?
                .map(|s| parse_severity(&s))
                .transpose()?;
            if body.is_none() && severity.is_none() {
                return Err("Give `body`, `severity`, or both".into());
            }
            sink.edit(&draft, body, severity)
                .map(|d| format!("Draft {} updated ({}).", d.id, d.severity.as_str()))
        }),
        tool::MR_DRAFT_DISCARD => required_str(args, "draft").and_then(|draft| {
            sink.discard(&draft)
                .map(|()| format!("Draft {draft} discarded."))
        }),
        _ => return None,
    };
    Some(match result {
        Ok(text) => ToolResult::text(text),
        Err(error) => ToolResult::error(error),
    })
}

fn draft_request(args: &Value) -> Result<DraftRequest, String> {
    let severity = parse_severity(&required_str(args, "severity")?)?;
    let body = required_str(args, "body")?;
    let path = optional_str(args, "path")?.filter(|p| !p.trim().is_empty());
    let line = match args.get("line") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .filter(|n| (1..=u32::MAX as u64).contains(n))
                .ok_or("`line` must be a positive whole number")? as u32,
        ),
    };
    let side = match optional_str(args, "side")?.as_deref() {
        None | Some("new") => GitLabSide::New,
        Some("old") => GitLabSide::Old,
        Some(other) => return Err(format!("`side` is \"new\" or \"old\", not \"{other}\"")),
    };
    Ok(DraftRequest {
        severity,
        body,
        path,
        line,
        side,
    })
}

fn parse_severity(text: &str) -> Result<GitLabSeverity, String> {
    GitLabSeverity::parse(text).ok_or_else(|| {
        format!("`severity` is one of critical, major, minor, suggestion — not \"{text}\"")
    })
}

fn required_str(args: &Value, key: &str) -> Result<String, String> {
    optional_str(args, key)?.ok_or_else(|| format!("`{key}` is required"))
}

fn optional_str(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// The brief a review run is given, with every tool spelled the way `harness` presents it.
///
/// This is the run's **whole** system prompt: `RunPlan::tracker_paragraphs` is off for a review,
/// so nothing about tasks is appended after it.
pub fn review_brief(harness: &dyn Harness) -> String {
    let mut text = REVIEW_BRIEF.to_string();
    for name in tool::REVIEW {
        text = text.replace(&format!("{{{name}}}"), &harness.tool_name(name));
    }
    text
}

const REVIEW_BRIEF: &str = "You are reviewing a GitLab merge request inside cide, as a careful \
senior reviewer. Your working directory is a disposable checkout of the MR's head commit: the \
whole project as the MR would leave it. Read any file, search, and run read-only commands (git \
log, git show, git diff, builds or tests that do not write outside the checkout). Do not edit \
files, commit, push, or contact GitLab yourself.

Start with {cide_mr_info}. Read the diff ({cide_mr_diff}, or `git diff <base>..<head>` in the \
checkout) and enough of the surrounding code to judge each change in context: callers, tests, \
invariants the change must keep.

Every finding is one call to {cide_mr_draft_comment}, with a severity — critical (bug, data \
loss, security hole, broken build: must be fixed before merge), major (wrong behaviour or a \
serious design problem), minor (a real but small problem), suggestion (optional). Anchor it on \
the exact changed line it is about. Never put the severity into the comment text. Write the \
comment for the MR author: what is wrong, why it matters, what to do instead — concise, \
specific, no praise and no filler. Report real problems only; do not invent findings to fill a \
quota, and do not repeat points already made in the MR's existing threads. You may check your \
drafts with {cide_mr_drafts} and correct or drop your own with {cide_mr_draft_edit} and \
{cide_mr_draft_discard}.

Your drafts are private: the user reads them in cide and publishes the ones they agree with. \
When you have finished, end with a short summary: overall assessment, and the count of drafts \
per severity.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{claude::ClaudeHarness, opencode::OpencodeHarness};
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        asked: RefCell<Vec<DraftRequest>>,
    }
    impl ReviewSink for Fake {
        fn info(&self) -> Result<Value, String> {
            Ok(json!({"title": "t"}))
        }
        fn diff(&self, path: Option<&str>) -> Result<String, String> {
            Ok(path.unwrap_or("all").to_string())
        }
        fn draft(&self, request: DraftRequest) -> Result<GitLabDraft, String> {
            self.asked.borrow_mut().push(request.clone());
            Ok(GitLabDraft {
                id: "d1".into(),
                review: "r".into(),
                severity: request.severity,
                body: request.body,
                path: request.path,
                old_path: None,
                side: Some(request.side),
                line: request.line,
                position: None,
                head_sha: "h".into(),
                author: cide_ipc::gitlab::GitLabDraftAuthor {
                    label: "x".into(),
                    harness: None,
                    run: None,
                },
                created_unix_ms: 0,
            })
        }
        fn drafts(&self) -> Result<Vec<GitLabDraft>, String> {
            Ok(vec![])
        }
        fn edit(
            &self,
            _: &str,
            _: Option<String>,
            _: Option<GitLabSeverity>,
        ) -> Result<GitLabDraft, String> {
            Err("not yours".into())
        }
        fn discard(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_draft_needs_a_known_severity_and_defaults_to_the_new_side() {
        let sink = Fake::default();
        let ok = dispatch(
            tool::MR_DRAFT_COMMENT,
            &json!({"severity": "Critical", "body": "b", "path": "a.rs", "line": 3}),
            &sink,
        )
        .unwrap();
        assert!(!ok.is_error, "{ok:?}");
        assert_eq!(sink.asked.borrow()[0].side, GitLabSide::New);
        assert_eq!(sink.asked.borrow()[0].severity, GitLabSeverity::Critical);

        for bad in [
            json!({"severity": "blocker", "body": "b"}),
            json!({"body": "b"}),
            json!({"severity": "minor", "body": "b", "line": 0}),
            json!({"severity": "minor", "body": "b", "side": "left"}),
        ] {
            let result = dispatch(tool::MR_DRAFT_COMMENT, &bad, &sink).unwrap();
            assert!(result.is_error, "{bad} should be refused");
        }
        assert_eq!(sink.asked.borrow().len(), 1);
        assert!(dispatch("cide_task_create", &json!({}), &sink).is_none());
        assert!(
            dispatch(tool::MR_DRAFT_EDIT, &json!({"draft": "d1"}), &sink)
                .unwrap()
                .is_error
        );
    }

    #[test]
    fn every_review_tool_has_a_schema_and_a_description_and_none_publishes() {
        for entry in descriptors() {
            let name = entry["name"].as_str().unwrap();
            assert!(!entry["description"].as_str().unwrap().is_empty(), "{name}");
            assert_eq!(entry["inputSchema"]["type"], "object", "{name}");
            assert!(!name.contains("publish"), "publishing is the user's alone");
        }
        assert_eq!(descriptors().len(), tool::REVIEW.len());
    }

    #[test]
    fn the_brief_names_every_tool_the_way_the_harness_spells_it() {
        let claude = review_brief(&ClaudeHarness);
        assert!(
            claude.contains("mcp__cide__cide_mr_draft_comment"),
            "{claude}"
        );
        assert!(!claude.contains('{'), "no placeholder is left unfilled");
        let opencode = review_brief(&OpencodeHarness);
        assert!(opencode.contains(&OpencodeHarness.tool_name(tool::MR_INFO)));
        assert!(!opencode.contains("mcp__cide__"));
        assert!(
            !claude.contains("cide_task_"),
            "a reviewer is told nothing of the tracker"
        );
    }
}
