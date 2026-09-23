//! An agent reviewing a GitLab merge request, from the MR panel's button to its draft comments.
//! (M85)
//!
//! # What it is made of
//!
//! Nothing new where something old would do. The run is an ordinary [`crate::agents`] run — the
//! same queue, the same fork, the same five harnesses, the same pane — distinguished only by
//! [`RunPurpose::MrReview`]: a role built in memory rather than read from `.cide/agents/`, a cwd
//! that is the MR's disposable checkout (`cide_gitlab`'s review workspace, with the base commit
//! fetched too), and a connection served [`cide_agents::review`]'s tools instead of the tracker's.
//!
//! # Why a run and not a `claude_tab`
//!
//! `claude_tab::open_with_prompt` is a `claude` in a pane and nothing else; the user picks any
//! harness here, and only the run machinery knows how to fork opencode, codex, qwen and mimo, feed
//! each its prompt, and render their JSON streams. The price is that a review shows in the Agents
//! panel beside the project's own runs — which is also where a person stops one.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cide_agents::review::{self, DraftRequest, ReviewSink};
use cide_agents::tools::ToolResult;
use cide_ipc::gitlab::{GitLabDraft, GitLabDraftAuthor, GitLabSeverity};
use cide_ipc::{Harness, RunId};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::agent_rpc::ToolAccess;

pub use crate::agents::RunPurpose;

/// The id every review run is filed under. Not a role anyone can dispatch: `RunPurpose` is what
/// makes a run a review, and this is only the key its queue and label hang off.
pub const REVIEW_AGENT: &str = "mr-review";

/// The longest diff one `cide_mr_diff` answers with, whole-MR. A model reading a 5 MB diff in
/// one tool result reads none of it; past this it is told to ask per file.
const DIFF_BUDGET: usize = 200 * 1024;

/// What `cide_mr_info` quotes of each existing note — enough to recognise a point, not a thread.
const NOTE_BUDGET: usize = 600;

/// The tools a review connection is served. One per connection, like `ProjectTools`.
pub(crate) struct ReviewTools {
    app: AppHandle,
    run: RunId,
    review: String,
    label: String,
    harness: Harness,
    cwd: PathBuf,
}

impl ReviewTools {
    pub(crate) fn new(
        app: AppHandle,
        run: RunId,
        review: String,
        label: String,
        harness: Harness,
        cwd: PathBuf,
    ) -> Self {
        Self {
            app,
            run,
            review,
            label,
            harness,
            cwd,
        }
    }
}

impl ToolAccess for ReviewTools {
    fn names(&self) -> &[&'static str] {
        review::tool::REVIEW
    }

    fn call(&self, name: &str, arguments: &Value) -> ToolResult {
        let service = match self
            .app
            .try_state::<crate::cmd::gitlab::GitLabState>()
            .ok_or_else(|| "GitLab is not available in this window".to_string())
            .and_then(|state| state.service())
        {
            Ok(service) => service,
            Err(error) => return ToolResult::error(error),
        };
        let sink = Sink {
            tools: self,
            service: &service,
            changed: AtomicBool::new(false),
        };
        let result = review::dispatch(name, arguments, &sink)
            .unwrap_or_else(|| ToolResult::error(format!("no such tool: {name}")));
        // After the write, outside anything the service locked: the panel re-reads the drafts.
        if sink.changed.load(Ordering::Relaxed) {
            crate::emit::gitlab_drafts_changed(&self.app, &self.review);
        }
        result
    }
}

struct Sink<'a> {
    tools: &'a ReviewTools,
    service: &'a cide_gitlab::GitLab,
    changed: AtomicBool,
}

impl Sink<'_> {
    fn run(&self) -> String {
        self.tools.run.to_string()
    }
}

impl ReviewSink for Sink<'_> {
    fn info(&self) -> Result<Value, String> {
        let review = &self.tools.review;
        let mr = self.service.detail(review)?;
        let version = self.service.latest_version(review)?;
        let discussions = self.service.discussions(review)?;
        Ok(brief_of(&mr, &version, &discussions, &self.tools.cwd))
    }

    fn diff(&self, path: Option<&str>) -> Result<String, String> {
        let version = self.service.latest_version(&self.tools.review)?;
        unified_diff(&version, path)
    }

    fn draft(&self, request: DraftRequest) -> Result<GitLabDraft, String> {
        let draft = self.service.draft_create(
            &self.tools.review,
            cide_gitlab::NewDraft {
                severity: request.severity,
                body: request.body,
                path: request.path,
                line: request.line,
                side: request.side,
            },
            GitLabDraftAuthor {
                label: self.tools.label.clone(),
                harness: Some(self.tools.harness),
                run: Some(self.run()),
            },
        )?;
        self.changed.store(true, Ordering::Relaxed);
        Ok(draft)
    }

    fn drafts(&self) -> Result<Vec<GitLabDraft>, String> {
        Ok(self.service.drafts(&self.tools.review))
    }

    fn edit(
        &self,
        draft: &str,
        body: Option<String>,
        severity: Option<GitLabSeverity>,
    ) -> Result<GitLabDraft, String> {
        let run = self.run();
        let edited =
            self.service
                .draft_edit(&self.tools.review, draft, body, severity, Some(&run))?;
        self.changed.store(true, Ordering::Relaxed);
        Ok(edited)
    }

    fn discard(&self, draft: &str) -> Result<(), String> {
        let run = self.run();
        let removed =
            self.service
                .draft_discard(&self.tools.review, &[draft.to_string()], Some(&run))?;
        if removed == 0 {
            return Err(format!("No draft `{draft}` on this review"));
        }
        self.changed.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// `cide_mr_info`'s answer: the MR shaped for a model, not GitLab's payload verbatim — which is
/// three screens of avatars, pipeline objects and URLs the review has no use for.
fn brief_of(mr: &Value, version: &Value, discussions: &[Value], cwd: &Path) -> Value {
    let base = version["base_commit_sha"].as_str().unwrap_or_default();
    let head = version["head_commit_sha"].as_str().unwrap_or_default();
    let files: Vec<Value> = version["diffs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let status = if c["new_file"].as_bool() == Some(true) {
                "added"
            } else if c["deleted_file"].as_bool() == Some(true) {
                "deleted"
            } else if c["renamed_file"].as_bool() == Some(true) {
                "renamed"
            } else {
                "modified"
            };
            let mut file = json!({ "path": c["new_path"], "status": status });
            if status == "renamed" {
                file["old_path"] = c["old_path"].clone();
            }
            if c["diff"].as_str().is_none_or(str::is_empty) && status != "deleted" {
                file["diff_unavailable"] = json!(true);
            }
            file
        })
        .collect();
    let threads: Vec<Value> = discussions
        .iter()
        .filter_map(|d| {
            let notes: Vec<&Value> = d["notes"]
                .as_array()?
                .iter()
                .filter(|n| n["system"].as_bool() != Some(true))
                .collect();
            let first = notes.first()?;
            let position = &first["position"];
            Some(json!({
                "path": position["new_path"].as_str().or(position["old_path"].as_str()),
                "line": position["new_line"].as_u64().or(position["old_line"].as_u64()),
                "resolved": notes.iter().any(|n| n["resolvable"].as_bool() == Some(true))
                    && notes.iter().filter(|n| n["resolvable"].as_bool() == Some(true))
                        .all(|n| n["resolved"].as_bool() == Some(true)),
                "notes": notes.iter().map(|n| json!({
                    "author": n["author"]["username"],
                    "body": clip(n["body"].as_str().unwrap_or_default(), NOTE_BUDGET),
                })).collect::<Vec<_>>(),
            }))
        })
        .collect();
    json!({
        "iid": mr["iid"],
        "title": mr["title"],
        "description": mr["description"],
        "state": mr["state"],
        "draft": mr["draft"],
        "author": mr["author"]["username"],
        "source_branch": mr["source_branch"],
        "target_branch": mr["target_branch"],
        "web_url": mr["web_url"],
        "checkout": cwd,
        "base_sha": base,
        "head_sha": head,
        "diff_command": format!("git diff {base} {head}"),
        "changed_files": files,
        "existing_threads": threads,
    })
}

/// The MR's diff as `git diff` would print it, from GitLab's per-file hunks.
fn unified_diff(version: &Value, path: Option<&str>) -> Result<String, String> {
    let mut out = String::new();
    let mut matched = false;
    for change in version["diffs"].as_array().into_iter().flatten() {
        let (old, new) = (
            change["old_path"].as_str().unwrap_or_default(),
            change["new_path"].as_str().unwrap_or_default(),
        );
        if path.is_some_and(|p| p != old && p != new) {
            continue;
        }
        matched = true;
        let body = change["diff"].as_str().unwrap_or_default();
        out.push_str(&format!("diff --git a/{old} b/{new}\n"));
        if body.is_empty() {
            out.push_str("(GitLab returned no diff for this file: too large, collapsed or binary. Read it in the checkout.)\n");
            continue;
        }
        let from = match change["new_file"].as_bool() == Some(true) {
            true => "/dev/null".to_string(),
            false => format!("a/{old}"),
        };
        let to = match change["deleted_file"].as_bool() == Some(true) {
            true => "/dev/null".to_string(),
            false => format!("b/{new}"),
        };
        out.push_str(&format!("--- {from}\n+++ {to}\n{body}"));
        if !body.ends_with('\n') {
            out.push('\n');
        }
        if path.is_none() && out.len() > DIFF_BUDGET {
            out.push_str("\n[the whole-MR diff is larger than one answer holds; ask for the remaining files one `path` at a time, or run `git diff` in the checkout]\n");
            return Ok(out);
        }
    }
    match (matched, path) {
        (false, Some(path)) => Err(format!(
            "`{path}` is not changed by this MR; cide_mr_info lists the changed files"
        )),
        _ => Ok(out),
    }
}

fn clip(text: &str, budget: usize) -> String {
    match text.char_indices().nth(budget) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// `--allowedTools` for a claude reviewer: its own six tools and reading, run without a prompt.
/// Everything else — an edit, an arbitrary command — still asks, so a reviewer that decides to
/// "just fix it" has to get past a person first. Read-only `git` is listed because the brief
/// tells it to diff in the checkout.
pub fn claude_allowed_tools() -> Vec<String> {
    let mut tools: Vec<String> = review::tool::REVIEW
        .iter()
        .map(|name| format!("mcp__{}__{name}", cide_agents::harness::SERVER))
        .collect();
    tools.extend(
        [
            "Read",
            "Grep",
            "Glob",
            "LS",
            "Bash(git diff:*)",
            "Bash(git log:*)",
            "Bash(git show:*)",
            "Bash(git blame:*)",
        ]
        .map(String::from),
    );
    tools
}

/// The run's opening line. **One line**, for `opening_prompt`'s reason: it is typed into a TUI
/// where every newline is an Enter. The detail lives in the brief and in `cide_mr_info`.
pub fn opening_line(
    mr: &Value,
    version: &Value,
    cwd: &Path,
    with_base: bool,
    local: Option<&Path>,
    extra: Option<&str>,
) -> String {
    let base = version["base_commit_sha"].as_str().unwrap_or_default();
    let head = version["head_commit_sha"].as_str().unwrap_or_default();
    let mut line = format!(
        "Review GitLab merge request !{} \"{}\" ({} into {}). The MR head is checked out in {} (base {}, head {}).",
        mr["iid"],
        mr["title"].as_str().unwrap_or_default(),
        mr["source_branch"].as_str().unwrap_or_default(),
        mr["target_branch"].as_str().unwrap_or_default(),
        cwd.display(),
        short(base),
        short(head),
    );
    line.push_str(match with_base {
        true => " `git diff <base> <head>` works there.",
        false => {
            " The base commit could not be fetched into it, so read the diff with the MR diff tool."
        }
    });
    if let Some(local) = local {
        line.push_str(&format!(
            " The user's own working copy of this project is at {} — it may be on another branch; read it for context only, never edit it.",
            local.display()
        ));
    }
    if let Some(extra) = extra
        .map(crate::agent_rpc::one_line)
        .filter(|e| !e.is_empty())
    {
        line.push_str(" The user adds: ");
        line.push_str(&extra);
    }
    crate::agent_rpc::one_line(&line)
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// A repository under `roots` whose remote is one of `projects` on `host` — the user's own clone
/// of the MR's project, when the open cide project is it. `cmd::gitlab::gitlab_local_file`'s
/// match, widened to the target project too: a fork's MR is still a change to the target.
pub fn local_repo_for(roots: &[PathBuf], host: &str, projects: &[String]) -> Option<PathBuf> {
    for info in cide_git::repo::discover(roots) {
        let Ok(repo) = cide_git::repo::open(&info.root) else {
            continue;
        };
        let Ok(names) = repo.remotes() else { continue };
        let matches = names.iter().flatten().flatten().any(|name| {
            repo.find_remote(name)
                .ok()
                .and_then(|r| r.url().ok().map(str::to_string))
                .and_then(|url| cide_gitlab::project_from_url(host, &url).ok())
                .is_some_and(|p| projects.contains(&p))
        });
        if matches {
            return Some(info.root);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> Value {
        json!({
            "base_commit_sha": "a".repeat(40),
            "head_commit_sha": "c".repeat(40),
            "diffs": [
                {"old_path":"a.rs","new_path":"a.rs","diff":"@@ -1 +1 @@\n-x\n+y\n"},
                {"old_path":"b.rs","new_path":"b.rs","new_file":true,"diff":"@@ -0,0 +1 @@\n+z"},
                {"old_path":"big.rs","new_path":"big.rs","diff":""}
            ]
        })
    }

    #[test]
    fn the_diff_reads_like_git_and_refuses_an_unchanged_path() {
        let all = unified_diff(&version(), None).unwrap();
        assert!(all.contains("--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@"), "{all}");
        assert!(all.contains("--- /dev/null\n+++ b/b.rs"), "{all}");
        assert!(all.contains("+z\n"), "a missing final newline is supplied");
        assert!(all.contains("GitLab returned no diff"), "{all}");
        let one = unified_diff(&version(), Some("b.rs")).unwrap();
        assert!(!one.contains("a.rs"), "{one}");
        assert!(unified_diff(&version(), Some("nope.rs")).is_err());
    }

    #[test]
    fn the_opening_is_one_line_and_carries_the_users_words() {
        let mr =
            json!({"iid": 7, "title": "Fix\nit", "source_branch": "f", "target_branch": "main"});
        let line = opening_line(
            &mr,
            &version(),
            Path::new("/tmp/x"),
            true,
            Some(Path::new("/home/me/proj")),
            Some("focus on\nsecurity"),
        );
        assert!(!line.contains('\n'), "{line}");
        assert!(line.contains("!7") && line.contains("/tmp/x") && line.contains("/home/me/proj"));
        assert!(line.contains("focus on security"), "{line}");
    }

    #[test]
    fn the_brief_names_files_threads_and_the_checkout() {
        let discussions = vec![
            json!({"notes": [
                {"system": false, "author": {"username": "ann"}, "body": "why?", "resolvable": true, "resolved": false,
                 "position": {"new_path": "a.rs", "new_line": 1}}
            ]}),
            json!({"notes": [{"system": true, "body": "added 1 commit"}]}),
        ];
        let brief = brief_of(
            &json!({"iid": 1, "title": "t", "author": {"username": "bob"}}),
            &version(),
            &discussions,
            Path::new("/tmp/x"),
        );
        assert_eq!(brief["changed_files"][1]["status"], "added");
        assert_eq!(brief["changed_files"][2]["diff_unavailable"], true);
        assert_eq!(
            brief["existing_threads"].as_array().unwrap().len(),
            1,
            "system notes are not threads"
        );
        assert_eq!(brief["existing_threads"][0]["resolved"], false);
        assert_eq!(brief["checkout"], "/tmp/x");
    }

    #[test]
    fn a_claude_reviewer_may_read_and_draft_without_asking_and_nothing_more() {
        let tools = claude_allowed_tools();
        assert!(tools.contains(&"mcp__cide__cide_mr_draft_comment".to_string()));
        for write in ["Edit", "Write", "Bash", "MultiEdit"] {
            assert!(
                !tools.contains(&write.to_string()),
                "{write} must still ask"
            );
        }
    }
}
