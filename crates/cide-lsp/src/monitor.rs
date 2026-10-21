//! Bounded, per-project/server observations. Shared across explicit restarts.
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use cide_ipc::diagnostics::{LanguageServerLog, LanguageServerLogs};
use parking_lot::Mutex;

const LIMIT: usize = 1024 * 1024;

#[derive(Default)]
struct Buffer {
    entries: VecDeque<LanguageServerLog>,
    bytes: usize,
    sequence: u64,
}

#[derive(Default)]
pub struct Monitor {
    pub version: Mutex<Option<String>>,
    buffer: Mutex<Buffer>,
}

impl Monitor {
    pub fn record(&self, level: &str, message: &str) {
        let mut buffer = self.buffer.lock();
        let start = message.len().saturating_sub(LIMIT / 2);
        let start = (start..=message.len())
            .find(|i| message.is_char_boundary(*i))
            .unwrap_or(start);
        let message = message[start..].to_string();
        // Include record overhead so even a stream of empty messages remains bounded.
        let bytes = message.len() + level.len() + 64;
        buffer.sequence += 1;
        let sequence = buffer.sequence;
        buffer.entries.push_back(LanguageServerLog {
            sequence,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            level: level.into(),
            message,
        });
        buffer.bytes += bytes;
        while buffer.bytes > LIMIT {
            if let Some(old) = buffer.entries.pop_front() {
                buffer.bytes -= old.message.len() + old.level.len() + 64;
            }
        }
    }

    pub fn read(&self, after: u64) -> LanguageServerLogs {
        let buffer = self.buffer.lock();
        LanguageServerLogs {
            entries: buffer
                .entries
                .iter()
                .filter(|e| e.sequence > after)
                .cloned()
                .collect(),
            cursor: buffer.sequence,
            truncated: buffer
                .entries
                .front()
                .is_some_and(|e| e.sequence > after.saturating_add(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_are_bounded_incremental_and_utf8_safe() {
        let monitor = Monitor::default();
        monitor.record("info", "first");
        let first = monitor.read(0);
        assert_eq!(first.entries.len(), 1);
        assert!(monitor.read(first.cursor).entries.is_empty());
        for _ in 0..5 {
            monitor.record("stderr", &"界".repeat(LIMIT));
        }
        let logs = monitor.read(first.cursor);
        assert!(logs.truncated);
        assert!(
            logs.entries
                .iter()
                .map(|e| e.message.len() + e.level.len() + 64)
                .sum::<usize>()
                <= LIMIT
        );
        assert_eq!(logs.cursor, 6);
    }
}
