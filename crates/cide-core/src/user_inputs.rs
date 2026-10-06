//! Read-only prompt history. The CLI transcript is the durable source; this cache holds only
//! complete JSONL records. In particular, an append stopped halfway through a Unicode string
//! must be retried from its beginning, rather than forgotten as a malformed message.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

use cide_ipc::sessions::{UserInput, UserInputPage};

#[derive(Default)]
pub struct TranscriptInputs {
    offset: u64,
    observed: Option<(u64, Option<SystemTime>, u64)>,
    generation: u32,
    inputs: Vec<UserInput>,
    /// Claude records a slash command before it knows whether it will call a model. Keep it
    /// pending until an assistant record confirms that it did; local command output cancels it.
    pending_command: Option<(u64, String)>,
    codex_source: bool,
    activated_at: Option<i64>,
}

impl TranscriptInputs {
    /// Codex applies thread settings immediately on `/resume`, before model-turn hooks fire.
    /// Only CLI/exec rollouts qualify; a spawned subagent must not replace its parent's identity.
    pub fn activated_at(&self) -> Option<i64> {
        self.codex_source.then_some(self.activated_at).flatten()
    }
    pub fn version(&self) -> (u32, u32) {
        (self.generation, self.inputs.len() as u32)
    }

    /// Returns whether the prompt list changed. Called off the UI thread; unchanged files
    /// cost a stat, and growing files cost only their new complete lines.
    pub fn refresh(&mut self, path: &Path) -> std::io::Result<bool> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        };
        #[cfg(not(unix))]
        let identity = 0;
        let stamp = (metadata.len(), metadata.modified().ok(), identity);
        if self.observed == Some(stamp) {
            return Ok(false);
        }
        let replaced = self.observed.is_some_and(|previous| {
            identity != previous.2 || stamp.0 < previous.0 || stamp.0 == previous.0
        });
        if replaced {
            self.inputs.clear();
            self.offset = 0;
            self.pending_command = None;
            self.codex_source = false;
            self.activated_at = None;
            self.generation = self.generation.wrapping_add(1);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file);
        let before = self.inputs.len();
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 || line.last() != Some(&b'\n') {
                break;
            }
            let start = self.offset;
            self.offset += read as u64;
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) {
                if value["type"] == "session_meta" {
                    self.codex_source =
                        matches!(value["payload"]["source"].as_str(), Some("cli" | "exec"));
                }
                if value["type"] == "event_msg"
                    && value["payload"]["type"] == "thread_settings_applied"
                {
                    self.activated_at = value["timestamp"]
                        .as_str()
                        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                        .map(|stamp| stamp.timestamp_millis());
                }
                if let Some(command) = model_command(&value) {
                    self.pending_command = Some((start, command));
                } else if let Some(text) = submitted_text(&value) {
                    self.pending_command = None;
                    self.push(start, text);
                } else if value["type"] == "assistant" {
                    if let Some((offset, command)) = self.pending_command.take() {
                        self.push(offset, command);
                    }
                } else if crate::sessions::user_texts(&value)
                    .iter()
                    .any(|text| text.contains("<local-command-"))
                {
                    self.pending_command = None;
                }
            }
        }
        self.observed = Some(stamp);
        Ok(replaced || before != self.inputs.len())
    }

    fn push(&mut self, offset: u64, text: String) {
        self.inputs.push(UserInput {
            id: offset.to_string(),
            ordinal: self.inputs.len() as u32 + 1,
            text,
        });
    }

    pub fn page(&self, before: Option<u32>) -> UserInputPage {
        let end = before
            .map(|ordinal| ordinal.saturating_sub(1) as usize)
            .unwrap_or(self.inputs.len())
            .min(self.inputs.len());
        let start = end.saturating_sub(50);
        UserInputPage {
            inputs: self.inputs[start..end].to_vec(),
            total: self.inputs.len() as u32,
            has_previous: start != 0,
            available: true,
            generation: self.generation,
        }
    }
}

fn model_command(value: &serde_json::Value) -> Option<String> {
    let text = crate::sessions::user_texts(value).join("\n");
    if !text.trim_start().starts_with("<command-name>") {
        return None;
    }
    let name = text
        .split_once("<command-name>")?
        .1
        .split_once("</command-name>")?
        .0
        .trim();
    if name.is_empty() {
        return None;
    }
    if matches!(
        name,
        "/clear"
            | "/compact"
            | "/model"
            | "/resume"
            | "/rename"
            | "/config"
            | "/permissions"
            | "/help"
            | "/cost"
            | "/status"
            | "/context"
            | "/exit"
            | "/quit"
    ) {
        return None;
    }
    let args = text
        .split_once("<command-args>")
        .and_then(|(_, tail)| tail.split_once("</command-args>"))
        .map(|(args, _)| args.trim())
        .unwrap_or("");
    Some(if args.is_empty() {
        name.into()
    } else {
        format!("{name} {args}")
    })
}

/// Known CLI wrappers are excluded explicitly. Rejecting every text beginning with `<`, as
/// the first-prompt preview does, would lose a real XML/HTML request from the user's history.
pub fn submitted_text(value: &serde_json::Value) -> Option<String> {
    if value["isCompactSummary"] == true {
        return None;
    }
    let texts = crate::sessions::user_texts(value);
    let text = texts
        .into_iter()
        .filter(|text| {
            let trimmed = text.trim_start();
            ![
                "<command-name>",
                "<local-command-caveat>",
                "<local-command-stdout>",
                "<local-command-stderr>",
                "<system-reminder>",
                "<environment_context>",
                "<permissions instructions>",
                "<task-notification>",
                "Caveat:",
            ]
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn only_submitted_text_is_history() {
        assert_eq!(
            submitted_text(&json!({"type":"user", "message":{"content":[
                {"type":"text","text":"Первая строка"},
                {"type":"image","source":{}},
                {"type":"text","text":"<div>пример</div>"}
            ]}})),
            Some("Первая строка\n<div>пример</div>".into())
        );
        for value in [
            json!({"type":"user","isMeta":true,"message":{"content":"context"}}),
            json!({"type":"user","isCompactSummary":true,"message":{"content":"summary"}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","content":"yes"}]}}),
            json!({"type":"user","message":{"content":"<command-name>/model</command-name>"}}),
            json!({"type":"assistant","message":{"content":"quote the request"}}),
            json!({"type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"AGENTS.md"}]}}),
        ] {
            assert_eq!(submitted_text(&value), None, "{value}");
        }
        assert_eq!(
            submitted_text(&json!({"type":"event_msg","payload":{
                "type":"user_message","message":"  hello\nworld  "
            }})),
            Some("  hello\nworld  ".into())
        );
    }

    #[test]
    fn incremental_history_retries_partial_lines_pages_and_resets() {
        let path = std::env::temp_dir().join(format!("cide-inputs-{}.jsonl", uuid::Uuid::new_v4()));
        let mut file = File::create(&path).unwrap();
        let mut cache = TranscriptInputs::default();
        for _ in 0..55 {
            writeln!(
                file,
                "{}",
                json!({"type":"event_msg","payload":{"type":"user_message","message":"same"}})
            )
            .unwrap();
        }
        file.write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"")
            .unwrap();
        file.flush().unwrap();
        assert!(cache.refresh(&path).unwrap());
        let page = cache.page(None);
        assert_eq!(page.total, 55);
        assert_eq!(page.inputs.len(), 50);
        assert_eq!(page.inputs[0].ordinal, 6);
        assert!(page.has_previous);
        assert_ne!(page.inputs[0].id, page.inputs[1].id);
        assert_eq!(cache.page(Some(6)).inputs.len(), 5);
        assert!(!cache.refresh(&path).unwrap());
        file.write_all("Привет\n".replace('\n', "\\n").as_bytes())
            .unwrap();
        file.write_all(b"\"}}\nnot json\n").unwrap();
        file.flush().unwrap();
        assert!(cache.refresh(&path).unwrap());
        assert_eq!(cache.page(None).total, 56);
        assert_eq!(cache.page(None).inputs.last().unwrap().text, "Привет\n");
        std::fs::write(&path, b"").unwrap();
        assert!(cache.refresh(&path).unwrap());
        assert_eq!(cache.page(None).total, 0);
        assert_eq!(cache.page(None).generation, 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn model_commands_need_an_assistant_record_and_local_commands_stay_out() {
        let path =
            std::env::temp_dir().join(format!("cide-commands-{}.jsonl", uuid::Uuid::new_v4()));
        let user = |text| json!({"type":"user","message":{"content":text}});
        let mut file = File::create(&path).unwrap();
        for value in [
            user("<command-name>/model</command-name>"),
            user("<local-command-stdout>Model changed</local-command-stdout>"),
            user("<command-name>/review</command-name><command-args>the diff</command-args>"),
            json!({"type":"user","isMeta":true,"message":{"content":"Expanded review instructions"}}),
        ] {
            writeln!(file, "{value}").unwrap();
        }
        file.flush().unwrap();
        let mut cache = TranscriptInputs::default();
        cache.refresh(&path).unwrap();
        assert_eq!(
            cache.page(None).total,
            0,
            "command alone is not evidence of a model request"
        );
        writeln!(
            file,
            "{}",
            json!({"type":"assistant","message":{"content":"Reviewing"}})
        )
        .unwrap();
        file.flush().unwrap();
        cache.refresh(&path).unwrap();
        assert_eq!(cache.page(None).inputs[0].text, "/review the diff");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn codex_resume_activation_is_independent_of_prompts_and_excludes_subagents() {
        let path =
            std::env::temp_dir().join(format!("cide-activation-{}.jsonl", uuid::Uuid::new_v4()));
        let activation = json!({"type":"event_msg","timestamp":"2026-10-06T20:00:00.000Z",
            "payload":{"type":"thread_settings_applied"}});
        std::fs::write(
            &path,
            format!(
                "{}\n{activation}\n",
                json!({"type":"session_meta","payload":{"source":"cli"}})
            ),
        )
        .unwrap();
        let mut cache = TranscriptInputs::default();
        cache.refresh(&path).unwrap();
        assert!(cache.activated_at().is_some());
        assert_eq!(cache.page(None).total, 0, "a local resume is not a prompt");
        let replacement = path.with_extension("replacement");
        std::fs::write(
            &replacement,
            format!(
                "{}\n{activation}\n",
                json!({"type":"session_meta","payload":{"source":{"subagent":"test"}}})
            ),
        )
        .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        cache.refresh(&path).unwrap();
        assert_eq!(cache.activated_at(), None);
        std::fs::remove_file(path).unwrap();
    }
}
