//! Incremental, read-only conversation records. No terminal rendering or model calls.
use cide_ipc::conversations::ConversationItem;
use serde_json::Value;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    io::{BufRead, Seek},
    path::Path,
};

pub const PAGE_SIZE: usize = 50;
pub const PREVIEW_CHARS: usize = 8_000;
pub const DETAIL_CHARS: usize = 32_000;

pub fn digest(value: impl Hash) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hash);
    format!("{:x}", hash.finish())
}
pub fn clip(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

/// Input chrome wraps, clips and strips indentation; only validation is normalized.
/// The bytes pasted into the harness retain the user's original whitespace.
pub fn submitted_body_matches(observed: &str, expected: &str) -> bool {
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let observed = flat(observed);
    let expected = flat(expected);
    crate::peer_composer::matches_body(&observed, &expected)
        || crate::peer_composer::matches_tail(&observed, &expected, 64.min(expected.len()))
}

/// Native exporters are read-only helpers, but their output and lifetime still need bounds.
pub fn run_export(
    mut command: std::process::Command,
    deadline: std::time::Duration,
    limit: usize,
) -> Result<Vec<u8>, String> {
    use std::{io::Read, process::Stdio, time::Instant};
    crate::child_env::prepare_command(&mut command);
    crate::child_env::arm(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "The harness exporter could not be started.")?;
    let output = child
        .stdout
        .take()
        .ok_or("The harness exporter has no output.")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let until = Instant::now() + deadline;
    let result = (|| {
        let bytes = rx
            .recv_timeout(deadline)
            .map_err(|_| "The harness export timed out.")?
            .map_err(|_| "The harness export could not be read.")?;
        if bytes.len() > limit {
            return Err("The saved conversation exceeds the export size limit.".to_string());
        }
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(bytes)
                    } else {
                        Err("The harness could not export this saved conversation.".into())
                    };
                }
                Err(_) => return Err("The harness exporter could not be reaped.".into()),
                Ok(None) if Instant::now() < until => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Ok(None) => return Err("The harness export timed out.".into()),
            }
        }
    })();
    if result.is_err() {
        let tree = crate::process_tree::capture(child.id());
        let _ = child.kill();
        let _ = child.wait();
        tree.finish(
            std::time::Duration::from_millis(100),
            "conversation exporter".into(),
        );
    }
    let _ = reader.join();
    result
}

#[derive(Clone, Default)]
pub struct Entry {
    pub item: Option<ConversationItem>,
    pub detail: String,
    pub revision: u64,
}

#[derive(Clone, Default)]
pub struct Transcript {
    pub entries: Vec<Entry>,
    pub revision: u64,
    indexes: HashMap<String, usize>,
    codex_items: bool,
}

impl Transcript {
    fn put(
        &mut self,
        id: String,
        kind: &str,
        text: String,
        tool: Option<String>,
        status: Option<String>,
        detail: String,
    ) {
        if text.trim().is_empty() && kind != "tool" {
            return;
        }
        let preview = clip(&text, PREVIEW_CHARS);
        let detail = if detail.is_empty() {
            text.clone()
        } else {
            detail
        };
        let ordinal = self.indexes.get(&id).copied().unwrap_or(self.entries.len()) as u32;
        let item = ConversationItem {
            id: id.clone(),
            ordinal,
            kind: kind.into(),
            text: preview.clone(),
            tool,
            status,
            detail_available: kind == "tool" || preview != detail,
        };
        if let Some(&index) = self.indexes.get(&id) {
            let old = &self.entries[index];
            if old.item.as_ref() == Some(&item) && old.detail == detail {
                return;
            }
            self.revision += 1;
            self.entries[index] = Entry {
                item: Some(item),
                detail,
                revision: self.revision,
            };
        } else {
            self.revision += 1;
            self.indexes.insert(id, self.entries.len());
            self.entries.push(Entry {
                item: Some(item),
                detail,
                revision: self.revision,
            });
        }
    }
    fn tool(
        &mut self,
        id: &str,
        name: &str,
        input: &Value,
        output: Option<&Value>,
        error: bool,
        status: Option<&str>,
    ) {
        let key = format!("tool:{id}");
        let previous = self.indexes.get(&key).map(|i| &self.entries[*i]);
        let name = if name.is_empty() {
            previous
                .and_then(|e| e.item.as_ref()?.tool.as_deref())
                .unwrap_or("Tool")
        } else {
            name
        }
        .to_string();
        let input = if input.is_null() {
            String::new()
        } else {
            as_text(input)
        };
        let detail = if let Some(output) = output {
            let before = if input.is_empty() {
                previous
                    .map(|e| {
                        e.detail
                            .split("\n\nResult:\n")
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                    .unwrap_or_default()
            } else {
                input
            };
            format!("{before}\n\nResult:\n{}", as_text(output))
        } else {
            input
        };
        let state = if error {
            "error"
        } else {
            status.unwrap_or(if output.is_some() {
                "completed"
            } else {
                "running"
            })
        };
        self.put(
            key,
            "tool",
            name.clone(),
            Some(name),
            Some(state.into()),
            detail,
        );
    }

    pub fn feed(&mut self, v: &Value, fallback: &str) {
        let ty = v["type"].as_str().unwrap_or("");
        let id = v["uuid"]
            .as_str()
            .or_else(|| v["id"].as_str())
            .unwrap_or(fallback);
        if v["isSidechain"] == true {
            return;
        }
        if ty == "tool_result" {
            for (n, part) in v["message"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let result = &part["functionResponse"];
                let fallback = format!("{id}:{n}");
                let key = result["id"]
                    .as_str()
                    .or_else(|| v["toolCallResult"]["callId"].as_str())
                    .unwrap_or(&fallback);
                self.tool(
                    key,
                    result["name"].as_str().unwrap_or(""),
                    &Value::Null,
                    result
                        .get("response")
                        .or_else(|| v["toolCallResult"].get("resultDisplay")),
                    !matches!(
                        v["toolCallResult"]["error"],
                        Value::Null | Value::Bool(false)
                    ),
                    None,
                );
            }
            return;
        }
        if ty == "error" || (ty == "result" && v["is_error"] == true) {
            self.put(
                format!("error:{id}"),
                "error",
                v["message"]
                    .as_str()
                    .or_else(|| v["result"].as_str())
                    .unwrap_or("The harness reported an error.")
                    .into(),
                None,
                None,
                String::new(),
            );
            return;
        }
        if v["isCompactSummary"] == true
            || ty == "summary"
            || ty == "compact_boundary"
            || (ty == "system" && v["subtype"] == "compact_boundary")
        {
            self.put(
                format!("notice:{id}"),
                "notice",
                "Conversation compacted".into(),
                None,
                None,
                String::new(),
            );
            return;
        }
        if matches!(ty, "user" | "assistant") {
            let msg = &v["message"];
            let role = if ty == "assistant" {
                "assistant"
            } else {
                "user"
            };
            let mut texts = Vec::new();
            if let Some(t) = msg["content"].as_str() {
                texts.push(t.to_string());
            }
            for key in ["content", "parts"] {
                for (n, part) in msg[key].as_array().into_iter().flatten().enumerate() {
                    if part["thought"] == true {
                        continue;
                    }
                    let part_id = part["id"].as_str().unwrap_or(id).to_string();
                    if role == "assistant"
                        && !texts.is_empty()
                        && (part["type"] == "tool_use" || part.get("functionCall").is_some())
                    {
                        self.put(
                            format!("assistant:{}", msg["id"].as_str().unwrap_or(id)),
                            role,
                            texts.join("\n"),
                            None,
                            None,
                            String::new(),
                        );
                    }
                    match part["type"].as_str().unwrap_or("text") {
                        "text" | "input_text" | "output_text" => {
                            if let Some(t) = part["text"].as_str() {
                                texts.push(t.into());
                            }
                        }
                        "tool_use" => self.tool(
                            &part_id,
                            part["name"].as_str().unwrap_or("Tool"),
                            &part["input"],
                            None,
                            false,
                            None,
                        ),
                        "tool_result" => self.tool(
                            part["tool_use_id"].as_str().unwrap_or(&format!("{id}:{n}")),
                            "",
                            &Value::Null,
                            Some(&part["content"]),
                            part["is_error"] == true,
                            None,
                        ),
                        "image" | "image_url" => texts.push("[Image attachment]".into()),
                        _ => {}
                    }
                    if let Some(call) = part.get("functionCall") {
                        self.tool(
                            call["id"].as_str().unwrap_or(&part_id),
                            call["name"].as_str().unwrap_or("Tool"),
                            &call["args"],
                            None,
                            false,
                            None,
                        );
                    }
                    if let Some(result) = part.get("functionResponse") {
                        self.tool(
                            result["id"].as_str().unwrap_or(&part_id),
                            result["name"].as_str().unwrap_or("Tool"),
                            &Value::Null,
                            Some(&result["response"]),
                            false,
                            None,
                        );
                    }
                }
            }
            let text = if role == "user" && v["provenance"] == "system" {
                None
            } else if role == "user" {
                crate::user_inputs::submitted_text(v).or_else(|| {
                    (!texts.is_empty() && v["isMeta"] != true && !texts.iter().any(|t| internal(t)))
                        .then(|| texts.join("\n"))
                })
            } else {
                Some(texts.join("\n"))
            };
            if let Some(text) = text {
                self.put(
                    format!("{role}:{}", msg["id"].as_str().unwrap_or(id)),
                    role,
                    text,
                    None,
                    None,
                    String::new(),
                );
            }
            return;
        }
        if ty == "event_msg" {
            let p = &v["payload"];
            if p["type"] == "item_completed" {
                self.codex_items = true;
                let item = &p["item"];
                let key = item["id"].as_str().unwrap_or(id);
                match item["type"].as_str().unwrap_or("") {
                    "UserMessage" => {
                        if let Some(text) = crate::user_inputs::submitted_text(v) {
                            self.put(format!("user:{key}"), "user", text, None, None, String::new());
                        }
                    },
                    "AssistantMessage" | "AgentMessage" if item["phase"] != "analysis" => self.put(
                        format!("assistant:{key}"),
                        "assistant",
                        content_text(item),
                        None,
                        None,
                        String::new(),
                    ),
                    "CommandExecution" => self.tool(key, "Command", &item["command"], item.get("aggregated_output"), item["exit_code"].as_i64().is_some_and(|code| code != 0), item["status"].as_str()),
                    "FileChange" => self.tool(key, "File changes", &item["changes"], Some(&serde_json::json!({"stdout": item["stdout"], "stderr": item["stderr"]})), item["status"] == "failed", item["status"].as_str()),
                    "Plan" => self.put(format!("plan:{key}"), "notice", item["text"].as_str().unwrap_or("").into(), None, None, String::new()),
                    _ => {}
                }
            } else if p["type"] == "user_message" && !self.codex_items {
                if let Some(text) = p["message"].as_str() {
                    self.put(
                        format!("user:{id}"),
                        "user",
                        text.into(),
                        None,
                        None,
                        String::new(),
                    );
                }
            } else if p["type"] == "error" {
                self.put(
                    format!("error:{id}"),
                    "error",
                    as_text(&p["message"]),
                    None,
                    None,
                    String::new(),
                );
            }
            return;
        }
        if ty == "response_item" {
            let p = &v["payload"];
            let key = p["call_id"]
                .as_str()
                .or_else(|| p["id"].as_str())
                .unwrap_or(id);
            match p["type"].as_str().unwrap_or("") {
                "message"
                    if p["role"] == "assistant"
                        && p["phase"] != "analysis"
                        && !self.codex_items =>
                {
                    self.put(
                        format!("assistant:{key}"),
                        "assistant",
                        content_text(p),
                        None,
                        None,
                        String::new(),
                    )
                }
                "function_call" | "custom_tool_call" => self.tool(
                    key,
                    p["name"].as_str().unwrap_or("Tool"),
                    p.get("arguments")
                        .or_else(|| p.get("input"))
                        .unwrap_or(&Value::Null),
                    None,
                    false,
                    None,
                ),
                "function_call_output" | "custom_tool_call_output" => {
                    self.tool(key, "", &Value::Null, Some(&p["output"]), false, None)
                }
                _ => {}
            }
        }
    }

    /// Native OpenCode/MiMo export: message metadata and typed parts, never raw database rows.
    pub fn export(value: &Value) -> Self {
        let mut out = Self::default();
        let root = value.get("data").unwrap_or(value);
        for (i, m) in root["messages"]
            .as_array()
            .or_else(|| root.as_array())
            .into_iter()
            .flatten()
            .enumerate()
        {
            let info = m.get("info").or_else(|| m.get("message")).unwrap_or(m);
            let fallback = i.to_string();
            let id = info["id"].as_str().unwrap_or(&fallback);
            let role = info["role"]
                .as_str()
                .or_else(|| info["type"].as_str())
                .unwrap_or("assistant");
            if role == "compaction" {
                out.put(
                    id.into(),
                    "notice",
                    "Conversation compacted".into(),
                    None,
                    None,
                    String::new(),
                );
                continue;
            }
            if role == "shell" {
                let status = info["status"].as_str().unwrap_or("running");
                out.tool(
                    id,
                    "Shell",
                    &info["command"],
                    info["output"].get("output"),
                    info["exit"].as_i64().is_some_and(|code| code != 0),
                    Some(if status == "exited" {
                        "completed"
                    } else {
                        status
                    }),
                );
                continue;
            }
            if !matches!(role, "user" | "assistant") {
                continue;
            }
            if let Some(text) = info["text"].as_str() {
                out.put(id.into(), role, text.into(), None, None, String::new());
            }
            for (n, file) in info["files"].as_array().into_iter().flatten().enumerate() {
                out.put(
                    format!("{id}:file:{n}"),
                    role,
                    format!("[Attachment: {}]", file["name"].as_str().unwrap_or("file")),
                    None,
                    None,
                    String::new(),
                );
            }
            for (n, p) in m["parts"]
                .as_array()
                .or_else(|| m["content"].as_array())
                .into_iter()
                .flatten()
                .enumerate()
            {
                let fallback = format!("{id}:{n}");
                let key = p["id"].as_str().unwrap_or(&fallback);
                match p["type"].as_str().unwrap_or("") {
                    "text" if p["synthetic"] != true && p["ignored"] != true => out.put(
                        key.into(),
                        role,
                        p["text"].as_str().unwrap_or("").into(),
                        None,
                        None,
                        String::new(),
                    ),
                    "tool" => {
                        let state = &p["state"];
                        let status = state["status"].as_str().unwrap_or("running");
                        let failure = (status == "error").then(|| serde_json::json!({"error": state["error"], "output": state.get("output").or_else(|| state.get("content"))}));
                        out.tool(
                            key,
                            p["tool"]
                                .as_str()
                                .or_else(|| p["name"].as_str())
                                .unwrap_or("Tool"),
                            &state["input"],
                            failure.as_ref().or_else(|| {
                                state
                                    .get("output")
                                    .or_else(|| state.get("content"))
                                    .or_else(|| state.get("error"))
                            }),
                            status == "error",
                            Some(status),
                        );
                    }
                    "file" => out.put(
                        key.into(),
                        role,
                        format!("[Attachment: {}]", p["filename"].as_str().unwrap_or("file")),
                        None,
                        None,
                        String::new(),
                    ),
                    "compaction" => out.put(
                        key.into(),
                        "notice",
                        "Conversation compacted".into(),
                        None,
                        None,
                        String::new(),
                    ),
                    _ => {}
                }
            }
            if let Some(error) = info.get("error") {
                out.put(
                    format!("error:{id}"),
                    "error",
                    as_text(error),
                    None,
                    None,
                    String::new(),
                );
            }
        }
        out
    }

    /// Preserve revisions across complete native exports; changes to old tool cards must be delivered too.
    pub fn reconcile(&mut self, newer: Self) -> bool {
        let old_ids: Vec<_> = self
            .entries
            .iter()
            .filter_map(|e| e.item.as_ref().map(|i| &i.id))
            .collect();
        let new_ids: Vec<_> = newer
            .entries
            .iter()
            .filter_map(|e| e.item.as_ref().map(|i| &i.id))
            .collect();
        let reset = !old_ids.iter().zip(new_ids.iter()).all(|(a, b)| a == b)
            || new_ids.len() < old_ids.len();
        if reset {
            *self = newer;
            return true;
        }
        for entry in newer.entries {
            if let Some(item) = entry.item {
                self.put(
                    item.id,
                    &item.kind,
                    item.text,
                    item.tool,
                    item.status,
                    entry.detail,
                );
            }
        }
        false
    }
}

fn content_text(v: &Value) -> String {
    if let Some(text) = v["text"].as_str() {
        return text.into();
    }
    if let Some(text) = v["content"].as_str() {
        return text.into();
    }
    v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| {
            matches!(
                p["type"].as_str(),
                Some("text" | "Text" | "input_text" | "output_text") | None
            )
        })
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
fn internal(text: &str) -> bool {
    [
        "<system-reminder>",
        "<environment_context>",
        "<permissions instructions>",
        "<command-name>",
        "<local-command-",
        "<task-notification>",
    ]
    .iter()
    .any(|p| text.trim_start().starts_with(p))
}
fn as_text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.into();
    }
    if let Some(parts) = v.as_array() {
        return parts
            .iter()
            .map(|p| {
                p["text"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| p.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    serde_json::to_string_pretty(v).unwrap_or_default()
}

#[derive(Default)]
pub struct JsonlReader {
    pub transcript: Transcript,
    pub generation: u64,
    offset: u64,
    identity: Option<(u64, u64, Option<std::time::SystemTime>)>,
    nodes: HashMap<String, (Option<String>, Value, String)>,
    tip: Option<String>,
}
impl JsonlReader {
    pub fn refresh(&mut self, path: &Path) -> std::io::Result<()> {
        let mut file = std::fs::File::open(path)?;
        let meta = file.metadata()?;
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            meta.ino()
        };
        #[cfg(not(unix))]
        let inode = 0;
        let stamp = (inode, meta.len(), meta.modified().ok());
        if self.identity == Some(stamp) {
            return Ok(());
        }
        if self
            .identity
            .is_some_and(|old| old.0 != inode || stamp.1 < old.1 || stamp.1 == old.1)
        {
            self.offset = 0;
            self.transcript = Transcript::default();
            self.generation += 1;
            self.nodes.clear();
            self.tip = None;
        }
        file.seek(std::io::SeekFrom::Start(self.offset))?;
        let mut reader = std::io::BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = reader.read_until(b'\n', &mut line)?;
            if n == 0 || line.last() != Some(&b'\n') {
                break;
            }
            let key = self.offset.to_string();
            self.offset += n as u64;
            if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                if value["isSidechain"] == true {
                    continue;
                }
                if let Some(id) = value["uuid"].as_str() {
                    let parent = value["parentUuid"].as_str().map(str::to_string);
                    let mut ancestor = parent.as_deref();
                    let mut includes_tip = self.tip.is_none();
                    let mut count = 0;
                    while let Some(at) = ancestor {
                        if self.tip.as_deref() == Some(at) {
                            includes_tip = true;
                            break;
                        }
                        count += 1;
                        if count > self.nodes.len() {
                            break;
                        }
                        ancestor = self.nodes.get(at).and_then(|n| n.0.as_deref());
                    }
                    let branch = parent.as_ref().is_some_and(|p| self.nodes.contains_key(p))
                        && !includes_tip
                        && value["type"] != "system";
                    self.nodes
                        .insert(id.into(), (parent, value.clone(), key.clone()));
                    if branch {
                        let mut chain = Vec::new();
                        let mut at = Some(id);
                        while let Some(key) = at {
                            let Some(node) = self.nodes.get(key) else {
                                break;
                            };
                            chain.push(node);
                            if chain.len() > self.nodes.len() {
                                break;
                            }
                            at = node.0.as_deref();
                        }
                        self.transcript = Transcript::default();
                        self.generation += 1;
                        for (_, record, offset) in chain.into_iter().rev() {
                            self.transcript.feed(record, offset);
                        }
                    } else {
                        self.transcript.feed(&value, &key);
                    }
                    if value["type"] != "system" {
                        self.tip = Some(id.into());
                    }
                } else {
                    self.transcript.feed(&value, &key);
                }
            }
        }
        self.identity = Some(stamp);
        Ok(())
    }
}

/// Only the live input region is considered. Qwen's prompt is `> ` (`* ` in YOLO),
/// between rules; unlike Claude it has no ❯. Measured in Qwen's shipped OpenTUI renderer.
pub fn composer(
    harness: cide_ipc::Harness,
    capture: &cide_ipc::screen::ScreenCapture,
) -> Option<String> {
    use cide_ipc::{ConsoleHarness, Harness};
    if harness != Harness::Qwen {
        let console = match harness {
            Harness::Claude => ConsoleHarness::Claude,
            Harness::Codex => ConsoleHarness::Codex,
            _ => return None,
        };
        let (text, _) = crate::peer_composer::composer(console, capture)?;
        return Some(if crate::peer_composer::empty(console, &text) {
            String::new()
        } else {
            text
        });
    }
    // Older Qwen versions used Claude's input chrome.
    if let Some((text, _)) = crate::peer_composer::composer(ConsoleHarness::Claude, capture) {
        return Some(
            if crate::peer_composer::empty(ConsoleHarness::Claude, &text) {
                String::new()
            } else {
                text
            },
        );
    }
    let cursor = capture.cursor?;
    let rows: Vec<String> = capture
        .lines
        .iter()
        .map(|l| l.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect();
    let rule = |s: &str| {
        let s = s.trim();
        s.chars().count() >= 10 && s.chars().all(|c| matches!(c, '─' | '-' | '━'))
    };
    let row = usize::from(cursor.row);
    let bottom = rows
        .iter()
        .enumerate()
        .skip(row + 1)
        .find(|(_, s)| rule(s))
        .map(|(i, _)| i)?;
    let top = rows
        .iter()
        .enumerate()
        .take(row)
        .rev()
        .find(|(_, s)| rule(s))
        .map(|(i, _)| i)?;
    if bottom - top > 10 || row <= top || row >= bottom {
        return None;
    }
    let prompt = rows.get(top + 1)?.trim_start();
    let first = prompt
        .strip_prefix("> ")
        .or_else(|| prompt.strip_prefix("* "))?;
    let mut text = first.trim_end().to_string();
    for line in &rows[top + 2..bottom] {
        text.push('\n');
        text.push_str(line.get(3..).unwrap_or(line).trim_end());
    }
    if text.trim() == "Type your message or @path/to/file" && cursor.col <= 4 {
        text.clear();
    }
    if rows[bottom + 1..].iter().any(|r| {
        r.trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    }) {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_cli_exports_from_both_generations_are_readable() {
        // Imported synthetic transcripts, exported by the installed CLIs without a model turn.
        for raw in [
            include_str!("../tests/fixtures/conversations/mimo.json"),
            include_str!("../tests/fixtures/conversations/opencode-v2.json"),
        ] {
            let value: Value = serde_json::from_str(raw).unwrap();
            let transcript = Transcript::export(&value);
            let items: Vec<_> = transcript
                .entries
                .iter()
                .filter_map(|e| e.item.as_ref())
                .collect();
            assert!(
                items
                    .iter()
                    .any(|i| i.kind == "user" && i.text == "Fixture prompt")
            );
            assert!(
                items
                    .iter()
                    .any(|i| i.kind == "assistant" && i.text == "Fixture answer")
            );
        }
        let value: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/conversations/opencode-v2.json"
        ))
        .unwrap();
        let transcript = Transcript::export(&value);
        let tool = transcript
            .entries
            .iter()
            .find(|e| e.item.as_ref().is_some_and(|i| i.kind == "tool"))
            .unwrap();
        assert!(tool.detail.contains("pwd") && tool.detail.contains("/tmp/fixture"));
    }
    #[test]
    fn pasted_multiline_code_is_validated_without_rewriting_its_whitespace() {
        assert!(submitted_body_matches(
            "def f():\n  return 42",
            "def f():\n    return 42\n"
        ));
        assert!(!submitted_body_matches(
            "def f(): return 41",
            "def f():\n    return 42"
        ));
    }
    #[test]
    fn claude_tools_update_in_place_and_internal_text_stays_private() {
        let mut t = Transcript::default();
        t.feed(
            &json!({"type":"user","uuid":"u","message":{"content":"hello 世界"}}),
            "0",
        );
        t.feed(&json!({"type":"assistant","uuid":"a","message":{"content":[{"type":"thinking","thinking":"private"},{"type":"text","text":"Checking"},{"type":"tool_use","id":"c","name":"Bash","input":{"command":"pwd"}}]}}), "1");
        let before = t.revision;
        t.feed(&json!({"type":"user","uuid":"r","message":{"content":[{"type":"tool_result","tool_use_id":"c","content":"/work"}]}}), "2");
        t.feed(
            &json!({"type":"user","isMeta":true,"message":{"content":"secret"}}),
            "3",
        );
        assert_eq!(t.entries.len(), 3);
        let tool = t
            .entries
            .iter()
            .find(|e| e.item.as_ref().unwrap().kind == "tool")
            .unwrap();
        assert!(tool.revision > before);
        assert!(tool.detail.contains("pwd") && tool.detail.contains("/work"));
        assert_eq!(
            tool.item.as_ref().unwrap().status.as_deref(),
            Some("completed")
        );
        assert!(
            !format!(
                "{:?}",
                t.entries.iter().map(|e| &e.item).collect::<Vec<_>>()
            )
            .contains("private")
        );
    }
    #[test]
    fn codex_uses_human_event_and_public_assistant_and_pairs_tools() {
        let mut t = Transcript::default();
        for (i,v) in [
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"internal instructions"}]}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"fix it"}}),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"c","name":"exec","arguments":"pwd"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"/work"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}}),
        ].iter().enumerate() { t.feed(v, &i.to_string()); }
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.entries[0].item.as_ref().unwrap().text, "fix it");
        assert_eq!(t.entries[2].item.as_ref().unwrap().text, "Done");
    }
    #[test]
    fn codex_completed_items_use_the_native_text_casing_and_keep_command_activity() {
        let mut t = Transcript::default();
        for (i, value) in [
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u","content":[{"type":"text","text":"<environment_context>injected</environment_context>"},{"type":"text","text":"fix it","text_elements":[]}]}}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"model mirror"}]}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"a","phase":"commentary","content":[{"type":"Text","text":"Checking the build"}]}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"cmd","command":"cargo check","status":"completed","aggregated_output":"OK","exit_code":0}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","id":"patch","changes":[{"path":"app.rs","diff":"+fixed"}],"status":"completed","stdout":"Applied","stderr":""}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","id":"r","summary_text":["private"]}}}),
        ].iter().enumerate() { t.feed(value, &i.to_string()); }
        assert_eq!(t.entries.len(), 4);
        assert_eq!(t.entries[0].item.as_ref().unwrap().text, "fix it");
        assert_eq!(
            t.entries[1].item.as_ref().unwrap().text,
            "Checking the build"
        );
        assert!(t.entries[2].detail.contains("cargo check") && t.entries[2].detail.contains("OK"));
        assert!(t.entries[3].detail.contains("+fixed"));
    }
    #[test]
    fn qwen_parts_and_native_exports_are_readable() {
        let mut t = Transcript::default();
        t.feed(
            &json!({"type":"assistant","message":{"parts":[{"text":"Hello"}]}}),
            "0",
        );
        assert_eq!(t.entries[0].item.as_ref().unwrap().text, "Hello");
        let v = json!({"messages":[{"info":{"id":"m","role":"assistant"},"parts":[{"id":"p","type":"text","text":"Done"},{"id":"r","type":"reasoning","text":"private"},{"id":"t","type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"pwd"},"output":"/work"}}]}]});
        for export in [v.clone(), json!({"data":v})] {
            let t = Transcript::export(&export);
            assert_eq!(t.entries.len(), 2);
            assert!(t.entries[1].detail.contains("/work"));
        }
    }
    #[test]
    fn partial_unicode_records_retry_and_replacement_resets() {
        use std::io::Write;
        let p = std::env::temp_dir().join(format!("cide-conversation-{}", uuid::Uuid::new_v4()));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"")
            .unwrap();
        let mut r = JsonlReader::default();
        r.refresh(&p).unwrap();
        assert!(r.transcript.entries.is_empty());
        writeln!(f, "世界\"}}}}").unwrap();
        r.refresh(&p).unwrap();
        assert_eq!(r.transcript.entries.len(), 1);
        std::fs::write(&p, b"{}\n").unwrap();
        r.refresh(&p).unwrap();
        assert!(r.transcript.entries.is_empty());
        assert_eq!(r.generation, 1);
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn qwen_recorded_thoughts_and_system_notifications_are_excluded_and_calls_pair() {
        let mut t = Transcript::default();
        t.feed(&json!({"type":"assistant","uuid":"a","message":{"role":"model","parts":[{"thought":true,"text":"private"},{"text":"Checking"},{"functionCall":{"id":"c","name":"run_shell_command","args":{"command":"pwd"}}}]}}), "0");
        t.feed(&json!({"type":"tool_result","uuid":"r","toolCallResult":{"callId":"c","error":false},"message":{"role":"user","parts":[{"functionResponse":{"id":"c","name":"run_shell_command","response":{"output":"/work"}}}]}}), "1");
        t.feed(
            &json!({"type":"user","provenance":"system","message":{"parts":[{"text":"injected"}]}}),
            "2",
        );
        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.entries[0].item.as_ref().unwrap().text, "Checking");
        assert!(t.entries[1].detail.contains("/work"));
        assert_eq!(
            t.entries[1].item.as_ref().unwrap().status.as_deref(),
            Some("completed")
        );
    }
    #[test]
    fn native_export_updates_completed_tools_without_duplicate_cards() {
        let export = |status, output| {
            Transcript::export(
                &json!({"info":{},"messages":[{"info":{"id":"m","role":"assistant"},"parts":[{"id":"p","type":"tool","tool":"bash","state":{"status":status,"input":{"command":"pwd"},"output":output}}]}]}),
            )
        };
        let mut t = export("running", "");
        let revision = t.revision;
        assert!(!t.reconcile(export("completed", "/work")));
        assert_eq!(t.entries.len(), 1);
        assert!(t.revision > revision);
        assert!(t.entries[0].detail.contains("/work"));
    }
    #[test]
    fn selecting_an_older_branch_resets_the_history_generation() {
        use std::io::Write;
        let p = std::env::temp_dir().join(format!("cide-branch-{}", uuid::Uuid::new_v4()));
        let mut f = std::fs::File::create(&p).unwrap();
        let mut reader = JsonlReader::default();
        for (id, parent, text) in [
            ("u", None, "question"),
            ("a", Some("u"), "discarded answer"),
            ("b", Some("u"), "new answer"),
        ] {
            writeln!(f, "{}", json!({"type":if id=="u" {"user"} else {"assistant"},"uuid":id,"parentUuid":parent,"message":{"content":text}})).unwrap();
            reader.refresh(&p).unwrap();
        }
        assert_eq!(reader.generation, 1);
        assert_eq!(reader.transcript.entries.len(), 2);
        assert_eq!(
            reader.transcript.entries[1].item.as_ref().unwrap().text,
            "new answer"
        );
        std::fs::remove_file(p).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn exporter_limits_output_and_time_without_leaving_a_child_running() {
        let mut normal = std::process::Command::new("sh");
        normal.args(["-c", "printf '{\"messages\":[]}'"]);
        assert_eq!(
            run_export(normal, std::time::Duration::from_secs(2), 1024).unwrap(),
            br#"{"messages":[]}"#
        );
        let mut excessive = std::process::Command::new("sh");
        excessive.args(["-c", "while :; do printf '1234567890'; done"]);
        assert!(
            run_export(excessive, std::time::Duration::from_secs(2), 100)
                .unwrap_err()
                .contains("size limit")
        );
        let mut slow = std::process::Command::new("sh");
        slow.args(["-c", "sleep 10"]);
        let start = std::time::Instant::now();
        assert!(
            run_export(slow, std::time::Duration::from_millis(100), 1024)
                .unwrap_err()
                .contains("timed out")
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
    }

    #[test]
    fn qwen_composer_reads_only_the_input_and_preserves_desktop_drafts() {
        use cide_ipc::screen::*;
        let screen = |body: &str, row| ScreenCapture {
            info: ScreenInfo {
                rows: 24,
                cols: 80,
                alt: true,
                app_cursor: false,
                bracketed_paste: true,
                mouse: Default::default(),
            },
            cursor: Some(Cursor { row, col: 3 }),
            lines: body
                .lines()
                .enumerate()
                .map(|(row, text)| ScreenLine {
                    row: row as u16,
                    wrapped: false,
                    runs: vec![StyleRun {
                        text: text.into(),
                        fg: None,
                        bg: None,
                        flags: 0,
                    }],
                })
                .collect(),
        };
        assert_eq!(composer(cide_ipc::Harness::Qwen, &screen(" ────────────────────\n >   Type your message or @path/to/file\n ────────────────────", 1)).as_deref(), Some(""));
        assert_eq!(
            composer(
                cide_ipc::Harness::Qwen,
                &screen(
                    " ────────────────────\n > my draft\n ────────────────────",
                    1
                )
            )
            .as_deref(),
            Some("my draft")
        );
        assert!(
            composer(
                cide_ipc::Harness::Qwen,
                &screen("Approve this tool?\n > 1. Yes\n 2. No", 1)
            )
            .is_none()
        );
        assert_eq!(
            composer(
                cide_ipc::Harness::Qwen,
                &screen(
                    " ────────────────────\n > line one\n   line two\n ────────────────────",
                    2
                )
            )
            .as_deref(),
            Some("line one\nline two")
        );
    }
}
