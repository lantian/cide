//! Conservative recognizers for peer delivery into an interactive composer.
//! Unknown CLI layouts fail closed. These functions never inspect arbitrary historical prompts.
use cide_ipc::{ConsoleHarness, screen::ScreenCapture};

pub const CHUNK_BYTES: usize = 197;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

pub fn normalize(message: &str) -> Result<String, String> {
    if message.len() > MAX_MESSAGE_BYTES {
        return Err("Peer message exceeds 64 KiB".into());
    }
    if message
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("Peer message contains a control character".into());
    }
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return Err("Peer message is empty".into());
    }
    Ok(line)
}

pub fn chunks(text: &str, bytes: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(bytes.max(4));
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        if end < rest.len()
            && let Some(space) = rest[..end].rfind(' ')
            && space > 0
        {
            end = space + 1;
        }
        result.push(rest[..end].into());
        rest = &rest[end..];
    }
    result
}

/// Keep the witness on screen between chunks. A chunk normally ends in a space, which
/// a terminal capture trims when it is at the end of the composer. Removing the witness
/// after every chunk therefore made a correct delivery look truncated. Replace it only
/// as part of the next write, and remove it once the complete normalized body is verified.
pub fn marked_chunks<'a>(
    text: &str,
    marker: &'a str,
) -> impl Iterator<Item = (Vec<u8>, String, usize)> + 'a {
    assert!(!marker.is_empty() && marker.is_ascii() && marker.len() * 2 + 4 <= CHUNK_BYTES);
    let mut expected = String::new();
    let mut replace = false;
    chunks(text, CHUNK_BYTES - marker.len() * 2)
        .into_iter()
        .map(move |chunk| {
            let mut input = Vec::new();
            if replace {
                input.extend(std::iter::repeat_n(127, marker.len()));
            }
            input.extend_from_slice(chunk.as_bytes());
            input.extend_from_slice(marker.as_bytes());
            expected.push_str(&chunk);
            replace = true;
            (
                input,
                format!("{expected}{marker}"),
                chunk.len() + marker.len(),
            )
        })
}

fn rule(s: &str) -> bool {
    let s = s.trim();
    s.chars()
        .take_while(|c| matches!(c, '─' | '—' | '-'))
        .count()
        >= 10
}
fn particles(s: &str) -> String {
    s.chars()
        .filter(|c| !('\u{2800}'..='\u{28ff}').contains(c))
        .collect()
}

/// The live input only, plus cursor column. Refuse modals and shell mode, even at column two.
pub fn composer(harness: ConsoleHarness, capture: &ScreenCapture) -> Option<(String, u16)> {
    read_composer(harness, capture, false)
}

/// During a gated delivery, exact content verification distinguishes a wrapped numeric
/// message (including a wrapped UUID witness) from numbered permission entries. Readiness
/// detection still rejects numbered composers, and never writes into an existing draft.
pub fn matches_capture(
    harness: ConsoleHarness,
    capture: &ScreenCapture,
    expected: &str,
    witness_bytes: usize,
) -> bool {
    read_composer(harness, capture, true)
        .is_some_and(|(text, _)| matches_tail(&text, expected, witness_bytes))
}

fn read_composer(
    harness: ConsoleHarness,
    capture: &ScreenCapture,
    verifying: bool,
) -> Option<(String, u16)> {
    let cursor = capture.cursor?;
    let mut rows: Vec<String> = capture
        .lines
        .iter()
        .map(|l| {
            l.runs
                .iter()
                .map(|r| r.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    while rows.last().is_some_and(|s| s.trim().is_empty()) {
        rows.pop();
    }
    let row = usize::from(cursor.row);
    if row >= rows.len() {
        return None;
    }
    match harness {
        ConsoleHarness::Claude => {
            let bottom = rows.iter().rposition(|s| rule(s))?;
            // A dialog below the input box is not a live composer.
            if rows[bottom + 1..]
                .iter()
                .any(|s| !s.is_empty() && !s.starts_with("  "))
            {
                return None;
            }
            if rows[bottom + 1..].iter().any(|s| {
                s.trim_start()
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            }) {
                return None;
            }
            let prompt = rows[..bottom]
                .iter()
                .rposition(|s| s.trim_start().starts_with('❯'))?;
            if row < prompt || row >= bottom {
                return None;
            }
            let top = rows[..prompt].iter().rposition(|s| rule(s))?;
            if prompt != top + 1 {
                return None;
            }
            let mut body = rows[prompt]
                .trim_start()
                .strip_prefix('❯')?
                .trim_start()
                .to_string();
            for s in &rows[prompt + 1..bottom] {
                if !s.is_empty() {
                    body.push('\n');
                    body.push_str(s.trim_start());
                }
            }
            Some((body, cursor.col))
        }
        ConsoleHarness::Codex => {
            let prompt = rows
                .iter()
                .rposition(|s| s.starts_with('›') || s.starts_with('»'))?;
            if row < prompt {
                return None;
            }
            let first = particles(&rows[prompt]);
            let mut body = first
                .chars()
                .skip(1)
                .collect::<String>()
                .trim_start()
                .to_string();
            if !verifying && body.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return None;
            }
            // At most two footer rows, separated from the composer. Never remove arbitrary
            // numbered or indented modal entries as though they were shortcut hints.
            let footer = rows.last()?;
            if !footer.starts_with("  ")
                || footer
                    .trim_start()
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            {
                return None;
            }
            let mut end = rows.len() - 1;
            if end > prompt + 1 && rows[end - 1].starts_with("  ") && rows[end - 2].is_empty() {
                end -= 1;
            }
            if row >= end {
                return None;
            }
            for s in &rows[prompt + 1..end] {
                let s = particles(s);
                if s.trim().is_empty() {
                    continue;
                }
                if !s.starts_with("  ")
                    || (!verifying
                        && s.trim_start()
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_digit()))
                {
                    return None;
                }
                body.push('\n');
                body.push_str(s.trim_start());
            }
            Some((body, cursor.col))
        }
        ConsoleHarness::Opencode => None,
    }
}

pub fn empty(harness: ConsoleHarness, text: &str) -> bool {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match harness {
        ConsoleHarness::Claude => {
            matches!(
                text.as_str(),
                "" | "Press up to edit queued messages"
                    | "Press up to edit queued messages, Enter to send them immediately"
            ) || (text.starts_with("Try \"") && text.ends_with('"') && text.len() > 6)
        }
        ConsoleHarness::Codex => text == "Ask Codex to do anything",
        ConsoleHarness::Opencode => false,
    }
}

/// Visual wrapping may consume one space between rows, but never internal characters.
fn matching_offsets(observed: &str, expected: &str) -> Vec<usize> {
    let mut offsets = vec![expected.len()];
    let lines = observed.lines().collect::<Vec<_>>();
    for (i, line) in lines.iter().enumerate().rev() {
        let mut next = Vec::new();
        for end in offsets {
            if let Some(start) = end.checked_sub(line.len())
                && expected.get(start..end) == Some(*line)
            {
                next.push(start);
                if i > 0 && start > 0 && expected.as_bytes()[start - 1] == b' ' {
                    next.push(start - 1);
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        offsets = next;
    }
    offsets
}

pub fn matches_body(observed: &str, expected: &str) -> bool {
    matching_offsets(observed, expected).contains(&0)
}

/// Each chunk must be visible with an overlap from the already verified prefix. A clipped
/// repeated tail is ambiguous, so refuse it rather than hiding a deletion behind scrolling.
pub fn matches_tail(observed: &str, expected: &str, witness_bytes: usize) -> bool {
    if observed.is_empty() {
        return false;
    }
    let boundary = expected
        .len()
        .saturating_sub(witness_bytes.saturating_add(32));
    matching_offsets(observed, expected)
        .into_iter()
        .any(|start| {
            if start == 0 {
                return true;
            }
            if start > boundary {
                return false;
            }
            let tail = &expected[start..];
            expected.match_indices(tail).count() == 1
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::screen::{Cursor, ScreenInfo, ScreenLine, StyleRun};
    fn screen(text: &str, row: u16, col: u16) -> ScreenCapture {
        ScreenCapture {
            info: ScreenInfo {
                rows: 24,
                cols: 80,
                alt: true,
                app_cursor: false,
                bracketed_paste: true,
                mouse: Default::default(),
            },
            cursor: Some(Cursor { row, col }),
            lines: text
                .lines()
                .enumerate()
                .map(|(n, s)| ScreenLine {
                    row: n as u16,
                    wrapped: false,
                    runs: vec![StyleRun {
                        text: s.into(),
                        fg: None,
                        bg: None,
                        flags: 0,
                    }],
                })
                .collect(),
        }
    }
    #[test]
    fn safe_unicode_chunks_and_validation() {
        let text = "Привет 世界 😀 ".repeat(100);
        let c = chunks(&text, CHUNK_BYTES);
        assert_eq!(c.concat(), text);
        assert!(c.iter().all(|s| s.len() <= CHUNK_BYTES));
        assert!(normalize("hello\x1b[31m").is_err());
        assert!(normalize(" \n ").is_err());
        assert!(normalize(&"x".repeat(MAX_MESSAGE_BYTES + 1)).is_err());
        assert_eq!(normalize("a\nb").unwrap(), "a b");
    }
    #[test]
    fn chunk_witness_survives_trailing_space_capture_and_is_removed_only_at_the_end() {
        let marker = "[cide:00000000-0000-0000-0000-000000000000]";
        let body = normalize(&"Review Привет 世界 😀 with a unique finding. ".repeat(30)).unwrap();
        let mut input = Vec::new();
        let mut steps = 0;
        for (write, expected, witness) in marked_chunks(&body, marker) {
            assert!(write.len() <= CHUNK_BYTES);
            for byte in write {
                if byte == 127 {
                    input.pop();
                } else {
                    input.push(byte);
                }
            }
            let visible = std::str::from_utf8(&input).unwrap().trim_end();
            assert!(matches_tail(visible, &expected, witness));
            assert!(visible.ends_with(marker));
            assert_eq!(visible.matches(marker).count(), 1);
            steps += 1;
        }
        assert!(steps > 1);
        input.truncate(input.len() - marker.len());
        assert_eq!(String::from_utf8(input).unwrap(), body);
        // The old protocol failed here even though no input had been lost.
        let c = screen("› a chunk \n  ? for shortcuts", 0, 10);
        assert!(!matches_body(
            &composer(ConsoleHarness::Codex, &c).unwrap().0,
            "a chunk "
        ));
    }
    #[test]
    fn drafts_and_dialogs_are_not_empty() {
        let c = screen("────────────────\n❯ \n────────────────", 1, 2);
        assert!(empty(
            ConsoleHarness::Claude,
            &composer(ConsoleHarness::Claude, &c).unwrap().0
        ));
        let c = screen("────────────────\n❯ my draft\n────────────────", 1, 2);
        assert!(!empty(
            ConsoleHarness::Claude,
            &composer(ConsoleHarness::Claude, &c).unwrap().0
        ));
        let c = screen(
            "────────────────\n❯ \n────────────────\nAllow this command?\n 1. Yes",
            4,
            2,
        );
        assert!(composer(ConsoleHarness::Claude, &c).is_none());
        let c = screen("› 1. Yes\n 2. No\n  shortcuts", 0, 2);
        assert!(composer(ConsoleHarness::Codex, &c).is_none());
        let c = screen("! ls\n  shortcuts", 0, 2);
        assert!(composer(ConsoleHarness::Codex, &c).is_none());
        let c = screen("» Ask Codex to do anything\n  shortcuts", 0, 2);
        assert!(empty(
            ConsoleHarness::Codex,
            &composer(ConsoleHarness::Codex, &c).unwrap().0
        ));
        assert!(!matches_body("missing tail", "whole missing tail"));
    }
    #[test]
    fn wrapping_preserves_spaces_and_requires_a_unique_verified_tail() {
        assert!(matches_body(
            "hello world\nsecond line",
            "hello world second line"
        ));
        assert!(matches_body("hello wor\nld", "hello world"));
        assert!(!matches_body("helloworld", "hello world"));
        assert!(!matches_body("hello worldx", "hello world"));
        let prefix = "Already verified earlier text. ".repeat(10);
        let end =
            "Now a unique next chunk has arrived in the input composer with a private marker.";
        let expected = format!("{prefix}{end}");
        assert!(matches_tail(end, &expected, 20));
        assert!(!matches_tail("private marker.", &expected, 20));
        assert!(!matches_tail(
            "Now a unique next chunk has arrived in the input composer with a private marke.",
            &expected,
            20
        ));
        assert!(!matches_tail(
            &"repetition ".repeat(8),
            &"repetition ".repeat(30),
            20
        ));
        assert_eq!(chunks("😀世界", 1).concat(), "😀世界");
    }
    #[test]
    fn normal_composers_accept_wrapping_but_not_permission_footers() {
        let c = screen(
            "────────────────\n❯ hello wor\n  ld\n────────────────\n  ? for shortcuts",
            2,
            4,
        );
        assert!(matches_body(
            &composer(ConsoleHarness::Claude, &c).unwrap().0,
            "hello world"
        ));
        let c = screen(
            "› hello\n  world\n\n  ? for shortcuts\n    context left",
            1,
            7,
        );
        assert!(matches_body(
            &composer(ConsoleHarness::Codex, &c).unwrap().0,
            "hello world"
        ));
        let c = screen(
            "› a draft\n  1. Allow once\n  2. Deny\n  ? for shortcuts",
            0,
            2,
        );
        assert!(composer(ConsoleHarness::Codex, &c).is_none());
        assert!(!matches_capture(ConsoleHarness::Codex, &c, "a draft", 7));
        let c = screen(
            "Working (49s)\n› Peer review: [cide:00000000-0000-0000-0000-\n  000000000000]\n\n  gpt-5 default\n  tab to queue message",
            2,
            15,
        );
        assert!(composer(ConsoleHarness::Codex, &c).is_none());
        assert!(matches_capture(
            ConsoleHarness::Codex,
            &c,
            "Peer review: [cide:00000000-0000-0000-0000-000000000000]",
            43
        ));
        assert!(!matches_capture(
            ConsoleHarness::Codex,
            &c,
            "Peer review: [cide:00000000-0000-0000-0000-000000000001]",
            43
        ));
        let mut footer_cursor = c;
        footer_cursor.cursor.as_mut().unwrap().row = 5;
        assert!(!matches_capture(
            ConsoleHarness::Codex,
            &footer_cursor,
            "Peer review: [cide:00000000-0000-0000-0000-000000000000]",
            43
        ));
    }
}
