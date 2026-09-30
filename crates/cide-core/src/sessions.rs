//! The session journal's rules: what a pane in the tree says about a conversation, and how a
//! sighting merges into what the journal already holds. (M134)
//!
//! `cide_ipc::sessions` says why the journal exists and why it is keyed by conversation. This
//! module is the pure half — no disk, no clock, no registry — so every rule below is a unit test
//! away. `cide-app`'s `sessions_state` feeds it sightings from two places (the workspace tree
//! after every accepted mutation, the agent registry on every roster flush) and owns the file.
//!
//! # The merge, and the two things it must not do
//!
//! * **List one conversation twice.** A run's mirror tab, a console resumed from the Sessions tab
//!   and a codex console that has just learned its thread id are all a *second sighting* of a
//!   conversation the journal already has. They merge by id, by run, or — for a row whose id was
//!   only cide's handle standing in — by that handle, and the row is re-keyed in place.
//! * **Lose what the first sighting knew.** A run knows its role, task and prompt; its mirror pane
//!   knows none of that. A later sighting fills what is missing and never blanks what is there,
//!   and a pane never demotes a run's kind — see `SessionKind`'s doc.
//!
//! `/clear` is deliberately *not* a merge: the CLI starts a new conversation under a new id, the
//! old one stays resumable, and both are rows — which is what the user would expect to find.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cide_ipc::agents::Harness;
use cide_ipc::sessions::{PROMPT_CLIP, SessionKind, SessionRecord};
use cide_ipc::{Pane, PaneOrigin, SessionId, Tab, TabKind, Workspace};
use cide_ipc::{PaneKind, PaneRole};
use serde::{Deserialize, Serialize};

/// How many conversations the journal keeps per project. The oldest by `last_seen` go first.
///
/// Five hundred rows of a few hundred bytes each is a small file, and far past what anyone
/// scrolls; a cap at all is so that a project used daily for years does not grow a file that is
/// read whole on every list.
pub const PER_PROJECT: usize = 500;

/// How far `last_seen` may advance before it counts as a change worth writing. The tree mutates on
/// every focus change; a journal write per click would be the debounce doing all the work.
const SEEN_GRANULARITY_MS: u64 = 60_000;

/// The file, whole. Keyed by the project's primary root rather than its `ProjectId`, the way the
/// proposal queue is: a project closed and re-opened is the same project to the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    #[serde(default)]
    pub projects: BTreeMap<String, Vec<SessionRecord>>,
}

fn key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

impl Journal {
    /// A project's rows, newest-seen first.
    pub fn list(&self, root: &Path) -> Vec<SessionRecord> {
        let mut rows = self.projects.get(&key(root)).cloned().unwrap_or_default();
        rows.sort_by(|a, b| {
            b.last_seen_unix_ms
                .cmp(&a.last_seen_unix_ms)
                .then(b.started_unix_ms.cmp(&a.started_unix_ms))
        });
        rows
    }

    /// Merge one sighting. Answers whether anything worth writing changed.
    pub fn upsert(&mut self, root: &Path, seen: SessionRecord) -> bool {
        let rows = self.projects.entry(key(root)).or_default();
        let found = rows
            .iter()
            .position(|r| r.id == seen.id)
            .or_else(|| {
                seen.run
                    .and_then(|run| rows.iter().position(|r| r.run == Some(run)))
            })
            .or_else(|| {
                // A stand-in row meeting its real id: the same handle, now named.
                seen.session.and_then(|session| {
                    rows.iter()
                        .position(|r| !r.known && r.session == Some(session))
                })
            });
        let Some(index) = found else {
            rows.push(seen);
            prune(rows);
            return true;
        };
        merge(&mut rows[index], seen)
    }

    /// Fill in what a row's transcript says: its first prompt, when the row has none yet, and its
    /// `/rename` name, which can change for as long as the conversation is live. `false` when
    /// there was no row or nothing moved.
    pub fn set_head(&mut self, root: &Path, id: &str, head: TranscriptHead) -> bool {
        let Some(row) = self
            .projects
            .get_mut(&key(root))
            .and_then(|rows| rows.iter_mut().find(|r| r.id == id))
        else {
            return false;
        };
        let before = (row.prompt.clone(), row.name.clone());
        if row.prompt.is_none() {
            row.prompt = head.prompt;
        }
        if head.title.is_some() {
            row.name = head.title;
        }
        before != (row.prompt.clone(), row.name.clone())
    }
}

fn merge(row: &mut SessionRecord, seen: SessionRecord) -> bool {
    let before = row.clone();
    if row.kind.is_run() && !seen.kind.is_run() {
        // A run's mirror, or a pane continuing a run: the pane knows which handle is live and
        // nothing else the run did not know better.
        row.session = seen.session.or(row.session);
    } else {
        if seen.known || !row.known {
            row.id = seen.id;
            row.known = seen.known;
        }
        if seen.kind.is_run() && !row.kind.is_run() {
            row.kind = seen.kind;
        }
        row.harness = seen.harness;
        if !seen.title.is_empty() {
            row.title = seen.title;
        }
        row.cwd = seen.cwd;
        row.session = seen.session.or(row.session);
        row.run = seen.run.or(row.run);
        row.agent = seen.agent.or(row.agent.take());
        row.task = seen.task.or(row.task.take());
        row.task_title = seen.task_title.or(row.task_title.take());
        row.branch = seen.branch.or(row.branch.take());
        if row.prompt.is_none() {
            row.prompt = seen.prompt;
        }
        row.started_unix_ms = row.started_unix_ms.min(seen.started_unix_ms);
    }
    let seen_at = seen.last_seen_unix_ms.max(row.last_seen_unix_ms);
    let stale = seen_at.saturating_sub(before.last_seen_unix_ms) >= SEEN_GRANULARITY_MS;
    row.last_seen_unix_ms = seen_at;
    let mut compare = row.clone();
    compare.last_seen_unix_ms = before.last_seen_unix_ms;
    compare != before || stale
}

fn prune(rows: &mut Vec<SessionRecord>) {
    if rows.len() <= PER_PROJECT {
        return;
    }
    rows.sort_by(|a, b| b.last_seen_unix_ms.cmp(&a.last_seen_unix_ms));
    rows.truncate(PER_PROJECT);
}

/// One line of what was asked: whitespace collapsed, at most [`PROMPT_CLIP`] characters.
pub fn clip_prompt(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PROMPT_CLIP {
        return flat;
    }
    let mut out: String = flat.chars().take(PROMPT_CLIP - 1).collect();
    out.push('…');
    out
}

/// Every agent conversation the tree shows, with the project root each belongs under.
///
/// Agent panes only (`PaneKind::Claude`, which covers every harness through `Pane::harness`):
/// a shell's scrollback is not a conversation anything can reopen. `cwd_of` is where the caller
/// knows a session's spawn directory — a worker tab stands in its worktree, and only the running
/// child knows which — and `None` falls back to the directory the pane records, then the root.
pub fn observe(
    ws: &Workspace,
    now_ms: u64,
    cwd_of: impl Fn(SessionId) -> Option<PathBuf>,
) -> Vec<(PathBuf, SessionRecord)> {
    let mut out = Vec::new();
    for project in ws.projects.values() {
        let Some(root) = project.roots.first().map(|r| r.path.clone()) else {
            continue;
        };
        for tab in &project.tabs {
            for pane in tab.tree.panes.values() {
                if let Some(record) = of_pane(pane, Some(tab), &root, now_ms, &cwd_of) {
                    out.push((root.clone(), record));
                }
            }
        }
        for pane in project.detached.values() {
            if let Some(record) = of_pane(pane, None, &root, now_ms, &cwd_of) {
                out.push((root.clone(), record));
            }
        }
    }
    out
}

fn of_pane(
    pane: &Pane,
    tab: Option<&Tab>,
    root: &Path,
    now_ms: u64,
    cwd_of: &impl Fn(SessionId) -> Option<PathBuf>,
) -> Option<SessionRecord> {
    if pane.kind != PaneKind::Claude {
        return None;
    }
    let session = pane.session?;
    let harness = pane.harness.unwrap_or(Harness::Claude);
    let (id, known) = if let Some(conversation) = pane.conversation {
        (conversation.to_string(), true)
    } else if let Some(continues) = &pane.continues {
        (continues.id.clone(), true)
    } else {
        // Claude and Qwen take cide's handle as their `--session-id`, so the handle *is* the
        // conversation. The others name their own, and until they have, this is a stand-in.
        (
            session.to_string(),
            matches!(harness, Harness::Claude | Harness::Qwen),
        )
    };
    let cwd = pane
        .continues
        .as_ref()
        .map(|c| c.cwd.clone())
        .or_else(|| cwd_of(session))
        .unwrap_or_else(|| root.to_path_buf());
    let kind = match pane.origin {
        Some(PaneOrigin::Worker) => SessionKind::Worker,
        Some(PaneOrigin::Planner) => SessionKind::Planner,
        Some(PaneOrigin::Reviewer) => SessionKind::Reviewer,
        None => match tab.map(|t| &t.kind) {
            Some(TabKind::ClaudeHome) | None if pane.role == PaneRole::Primary => {
                SessionKind::Console
            }
            Some(TabKind::ClaudeHome) => SessionKind::ConsoleSplit,
            _ => SessionKind::Tab,
        },
    };
    let title = match tab.map(|t| &t.kind) {
        Some(TabKind::ClaudeFull { title, .. }) if !title.is_empty() => title.clone(),
        _ => pane.title.clone(),
    };
    let branch = worktree_name(root, &cwd);
    Some(SessionRecord {
        id,
        known,
        kind,
        harness,
        title,
        name: None,
        cwd,
        session: Some(session),
        run: None,
        agent: None,
        task: None,
        task_title: None,
        prompt: None,
        branch,
        started_unix_ms: now_ms,
        last_seen_unix_ms: now_ms,
    })
}

/// The worktree `cwd` is, when it is one of the project's `.cide/worktrees/<name>`.
pub fn worktree_name(root: &Path, cwd: &Path) -> Option<String> {
    let rest = cwd.strip_prefix(root).ok()?;
    let mut parts = rest.components();
    let (a, b, name) = (parts.next()?, parts.next()?, parts.next()?);
    (a.as_os_str() == ".cide" && b.as_os_str() == "worktrees")
        .then(|| name.as_os_str().to_string_lossy().into_owned())
}

// --- reading a transcript ----------------------------------------------------------------
//
// `cide-app`'s `lifecycle::transcript_exists` says cide never opens a transcript, and until M134
// that was true. The Sessions tab needs two things only a transcript holds — what a console
// conversation was first asked, and (when the user asks for it) whether a conversation ever said
// some word — so these two readers are the exception, and they keep the old rule's spirit:
// read-only, bounded, and every way of being wrong (a format change, a line that does not parse,
// a file that is not there) answers *nothing*, never an error. A wrong "nothing" costs the user
// one row without its prompt line or one missed search hit; a transcript layout cide misread into
// a failure would cost them the tab.
//
// The three CLIs file one JSON object per line and name the speaker differently:
//
// * claude and qwen: `{"type":"user","message":{"content": "…" | [{"type":"text","text":"…"}]}}`,
//   with `isMeta: true` on lines the CLI wrote itself, and qwen's `message.parts[].text`;
// * codex: `{"type":"event_msg","payload":{"type":"user_message","message":"…"}}` for what the
//   user typed — its `response_item` user messages also carry AGENTS.md and approval-review
//   preambles, which are not what anybody would call the first prompt.

/// How much of a transcript the first-prompt reader looks at. The first user message is near the
/// top; a transcript whose first megabyte holds none has nothing a row could show.
const FIRST_PROMPT_BYTES: u64 = 1024 * 1024;

/// What a transcript's opening says about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptHead {
    /// The first thing the user typed that was not a command or a wrapper.
    pub prompt: Option<String>,
    /// A `/rename` title, when the CLI filed one (claude's `custom-title` line).
    pub title: Option<String>,
}

/// Read the head of a transcript. Never fails: a reader error ends the read with what it had.
pub fn transcript_head(reader: impl std::io::BufRead) -> TranscriptHead {
    use std::io::BufRead as _;
    let mut head = TranscriptHead::default();
    for line in reader.take(FIRST_PROMPT_BYTES).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value["type"] == "custom-title"
            && let Some(title) = value["customTitle"].as_str()
            && !title.trim().is_empty()
        {
            head.title = Some(title.trim().to_owned());
        }
        if head.prompt.is_none() {
            head.prompt = user_texts(&value)
                .into_iter()
                .find(|text| is_typed(text))
                .map(|text| clip_prompt(&text));
        }
    }
    head
}

/// Whether a user message is something a person typed rather than a wrapper the CLI or cide put
/// in the user's turn: `<command-name>`, `<local-command-caveat>`, `<environment_context>`, …
fn is_typed(text: &str) -> bool {
    let text = text.trim_start();
    !text.is_empty() && !text.starts_with('<') && !text.starts_with("Caveat:")
}

/// The texts of a user message line, in any of the three shapes above; empty for anything else.
fn user_texts(value: &serde_json::Value) -> Vec<String> {
    if value["type"] == "user" && value["isMeta"] != true {
        return texts_of(&value["message"]);
    }
    if value["type"] == "event_msg" && value["payload"]["type"] == "user_message" {
        return value["payload"]["message"]
            .as_str()
            .map(|s| vec![s.to_owned()])
            .unwrap_or_default();
    }
    Vec::new()
}

fn texts_of(message: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(text) = message["content"].as_str() {
        out.push(text.to_owned());
    }
    for key in ["content", "parts"] {
        if let Some(items) = message[key].as_array() {
            for item in items {
                let kind = item["type"].as_str().unwrap_or("text");
                if matches!(kind, "text" | "input_text")
                    && let Some(text) = item["text"].as_str()
                {
                    out.push(text.to_owned());
                }
            }
        }
    }
    out
}

/// Every string a line carries, for the search: messages, tool inputs and results alike —
/// "anything that was said in it" is the question the user asked. Except the bookkeeping: a
/// search for `user` that matched every line's `"type":"user"` would find every transcript.
fn strings_of(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| strings_of(v, out)),
        serde_json::Value::Object(map) => map
            .iter()
            .filter(|(key, _)| !BOOKKEEPING.contains(&key.as_str()))
            .for_each(|(_, v)| strings_of(v, out)),
        _ => {}
    }
}

/// Keys whose values are the CLI's own records of a line rather than anything said in it.
const BOOKKEEPING: &[&str] = &[
    "type",
    "role",
    "uuid",
    "parentUuid",
    "sessionId",
    "session_id",
    "id",
    "tool_use_id",
    "toolUseID",
    "requestId",
    "timestamp",
    "cwd",
    "gitBranch",
    "version",
    "model",
    "userType",
    "signature",
    "stop_reason",
];

/// How much text a snippet shows before and after the match, in characters.
const SNIPPET_BEFORE: usize = 40;
const SNIPPET_AFTER: usize = 90;

/// The first place a transcript says `needle`, case-insensitively, as a one-line snippet.
///
/// `needle` must already be lowercase. The raw line is tested first and parsed only when it
/// matches: most lines do not, and parsing every line of a long transcript is the whole cost of
/// the search. The raw test is sound only for a needle JSON does not escape — no quote, no
/// backslash, no control character — so any other needle parses every line.
pub fn transcript_find(reader: impl std::io::BufRead, needle: &str) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    let raw_ok = !needle
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_control());
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if raw_ok && !line.to_lowercase().contains(needle) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let mut strings = Vec::new();
        strings_of(&value, &mut strings);
        for text in strings {
            if let Some(snippet) = snippet(&text, needle) {
                return Some(snippet);
            }
        }
    }
    None
}

fn snippet(text: &str, needle: &str) -> Option<String> {
    // Searched char by char over the lowercased text, so the offset found is a *character*
    // index that means the same thing in the original — a byte offset would not, for the few
    // characters whose lowercase is a different length.
    let lower: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let original: Vec<char> = text.chars().collect();
    let chars: &[char] = if lower.len() == original.len() {
        &original
    } else {
        &lower
    };
    let needle: Vec<char> = needle.chars().collect();
    let at = lower
        .windows(needle.len())
        .position(|w| w == needle.as_slice())?;
    let start = at.saturating_sub(SNIPPET_BEFORE);
    let end = (at + needle.len() + SNIPPET_AFTER).min(chars.len());
    let body: String = chars[start..end].iter().collect();
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!(
        "{}{flat}{}",
        if start > 0 { "…" } else { "" },
        if end < chars.len() { "…" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_head_skips_wrappers_and_meta_lines_and_finds_the_title() {
        let transcript = [
            r#"{"type":"custom-title","customTitle":"design pass"}"#,
            r#"{"type":"user","isMeta":true,"message":{"content":"meta line"}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"x"}]}}"#,
            r#"not json"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"Add a  sessions\ntab"}]}}"#,
            r#"{"type":"user","message":{"content":"later"}}"#,
        ]
        .join("\n");
        let head = transcript_head(transcript.as_bytes());
        assert_eq!(head.prompt.as_deref(), Some("Add a sessions tab"));
        assert_eq!(head.title.as_deref(), Some("design pass"));
    }

    #[test]
    fn a_codex_head_is_what_the_user_typed_not_the_preamble() {
        let transcript = [
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"AGENTS.md instructions"}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"fix the build"}}"#,
        ]
        .join("\n");
        assert_eq!(
            transcript_head(transcript.as_bytes()).prompt.as_deref(),
            Some("fix the build")
        );
    }

    #[test]
    fn a_search_finds_text_the_json_escaped_and_ignores_keys() {
        let transcript = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The Gate\nruns \"check\" first"}]}}"#,
        ]
        .join("\n");
        let hit = transcript_find(transcript.as_bytes(), "\"check\"").expect("quoted needle");
        assert!(hit.contains("\"check\""), "{hit}");
        let hit = transcript_find(transcript.as_bytes(), "the gate").expect("case-insensitive");
        assert!(hit.starts_with("The Gate"), "{hit}");
        assert!(
            transcript_find(transcript.as_bytes(), "assistant").is_none(),
            "a key or a type is not something said"
        );
    }

    #[test]
    fn a_snippet_is_trimmed_around_the_match() {
        let long = format!("{} needle {}", "a ".repeat(100), "b ".repeat(100));
        let s = snippet(&long, "needle").expect("found");
        assert!(
            s.starts_with('…') && s.ends_with('…') && s.contains("needle"),
            "{s}"
        );
    }

    fn record(id: &str, kind: SessionKind) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            known: true,
            kind,
            harness: Harness::Claude,
            title: "t".into(),
            name: None,
            cwd: PathBuf::from("/p"),
            session: None,
            run: None,
            agent: None,
            task: None,
            task_title: None,
            prompt: None,
            branch: None,
            started_unix_ms: 1_000,
            last_seen_unix_ms: 1_000,
        }
    }

    const ROOT: &str = "/p";

    #[test]
    fn a_second_sighting_keeps_the_first_start_and_changes_nothing() {
        let mut j = Journal::default();
        assert!(j.upsert(Path::new(ROOT), record("a", SessionKind::Console)));
        let mut again = record("a", SessionKind::Tab);
        again.started_unix_ms = 5_000;
        again.last_seen_unix_ms = 5_000;
        assert!(
            !j.upsert(Path::new(ROOT), again),
            "a focus change is not news"
        );
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].started_unix_ms, 1_000);
        assert_eq!(
            rows[0].kind,
            SessionKind::Console,
            "first seen wins between pane kinds"
        );
    }

    #[test]
    fn a_pane_never_demotes_a_run_nor_blanks_what_it_knew() {
        let mut j = Journal::default();
        let mut run = record("c", SessionKind::Subagent);
        run.agent = Some("coder".into());
        run.prompt = Some("do it".into());
        j.upsert(Path::new(ROOT), run);
        let mut mirror = record("c", SessionKind::Tab);
        mirror.session = Some(SessionId(uuid::Uuid::from_u128(7)));
        assert!(j.upsert(Path::new(ROOT), mirror));
        let row = &j.list(Path::new(ROOT))[0];
        assert_eq!(row.kind, SessionKind::Subagent);
        assert_eq!(row.agent.as_deref(), Some("coder"));
        assert_eq!(row.prompt.as_deref(), Some("do it"));
        assert_eq!(row.session, Some(SessionId(uuid::Uuid::from_u128(7))));
    }

    #[test]
    fn a_stand_in_is_re_keyed_when_the_harness_names_its_conversation() {
        let mut j = Journal::default();
        let handle = SessionId(uuid::Uuid::from_u128(1));
        let mut stand_in = record(&handle.to_string(), SessionKind::Console);
        stand_in.known = false;
        stand_in.harness = Harness::Codex;
        stand_in.session = Some(handle);
        j.upsert(Path::new(ROOT), stand_in);
        let mut named = record("thread-9", SessionKind::Console);
        named.harness = Harness::Codex;
        named.session = Some(handle);
        assert!(j.upsert(Path::new(ROOT), named));
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "thread-9");
        assert!(rows[0].known);
    }

    #[test]
    fn a_clear_is_a_second_conversation_not_a_rename() {
        let mut j = Journal::default();
        let handle = SessionId(uuid::Uuid::from_u128(2));
        let mut before = record(&handle.to_string(), SessionKind::Console);
        before.session = Some(handle);
        j.upsert(Path::new(ROOT), before);
        let mut after = record("after-clear", SessionKind::Console);
        after.session = Some(handle);
        j.upsert(Path::new(ROOT), after);
        assert_eq!(j.list(Path::new(ROOT)).len(), 2);
    }

    #[test]
    fn a_run_is_found_by_its_run_id_before_its_conversation_is_named() {
        let mut j = Journal::default();
        let run = cide_ipc::RunId(uuid::Uuid::from_u128(3));
        let mut queued = record("run:x", SessionKind::Subagent);
        queued.known = false;
        queued.run = Some(run);
        j.upsert(Path::new(ROOT), queued);
        let mut named = record("ses_1", SessionKind::Subagent);
        named.run = Some(run);
        j.upsert(Path::new(ROOT), named);
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "ses_1");
    }

    #[test]
    fn the_oldest_rows_go_past_the_cap() {
        let mut j = Journal::default();
        for i in 0..(PER_PROJECT as u64 + 3) {
            let mut r = record(&format!("s{i}"), SessionKind::Tab);
            r.last_seen_unix_ms = i;
            j.upsert(Path::new(ROOT), r);
        }
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), PER_PROJECT);
        assert!(rows.iter().all(|r| r.id != "s0" && r.id != "s2"));
    }

    #[test]
    fn a_prompt_is_one_clipped_line() {
        assert_eq!(clip_prompt("  fix\n\nthe  bug "), "fix the bug");
        let long = "x".repeat(PROMPT_CLIP + 10);
        assert_eq!(clip_prompt(&long).chars().count(), PROMPT_CLIP);
    }

    #[test]
    fn a_worktree_is_named_from_its_path() {
        let root = Path::new("/p");
        assert_eq!(
            worktree_name(root, Path::new("/p/.cide/worktrees/coder-t-1")).as_deref(),
            Some("coder-t-1")
        );
        assert_eq!(worktree_name(root, Path::new("/p")), None);
        assert_eq!(worktree_name(root, Path::new("/elsewhere")), None);
    }
}
