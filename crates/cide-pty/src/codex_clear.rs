//! Recognize a submitted Codex `/clear` followed by a live terminal reset.
//! Neither text in the transcript nor an unarmed redraw can invalidate a conversation.

use std::time::{Duration, Instant};

/// Read only the composer row holding the visible cursor. Slash-command suggestions below
/// it are allowed, while transcript rows, hidden cursors and modal answers are excluded.
pub fn composer(capture: &cide_ipc::screen::ScreenCapture) -> Option<String> {
    let cursor = capture.cursor?;
    let row = capture.lines.get(usize::from(cursor.row))?;
    let line: String = row.runs.iter().map(|r| r.text.as_str()).collect();
    let text = line
        .trim_start()
        .strip_prefix('›')
        .or_else(|| line.trim_start().strip_prefix('»'))?;
    let text: String = text
        .chars()
        .filter(|c| !('\u{2800}'..='\u{28ff}').contains(c))
        .collect();
    let text = text.trim().to_owned();
    Some(if text == "Ask Codex to do anything" {
        String::new()
    } else {
        text
    })
}

#[derive(Default)]
pub struct ClearObserver {
    input_parser: vte::Parser,
    input: Input,
    output_parser: vte::Parser,
    output: Output,
    submitted: Option<Instant>,
    escape_pending: bool,
}

#[derive(Default)]
struct Input {
    text: Vec<char>,
    cursor: usize,
    known: bool,
    live: bool,
    paste: bool,
    clear: bool,
}

impl Input {
    fn reset(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.known = true;
    }

    fn submit(&mut self) {
        let text: String = self.text.iter().collect();
        self.clear |= self.live && self.known && is_clear(&text);
        self.reset();
    }
}

fn is_clear(text: &str) -> bool {
    !text.contains(['\n', '\r']) && text.split_whitespace().next() == Some("/clear")
}

impl vte::Perform for Input {
    fn print(&mut self, c: char) {
        if self.text.len() >= 1024 {
            self.known = false;
            return;
        }
        self.text.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' | b'\n' if !self.paste => self.submit(),
            b'\r' | b'\n' => self.print('\n'),
            8 | 127 if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            1 => self.cursor = 0,                  // Ctrl+A
            5 => self.cursor = self.text.len(),    // Ctrl+E
            3 | 21 => self.reset(),                // Ctrl+C / Ctrl+U
            11 => self.text.truncate(self.cursor), // Ctrl+K
            23 => {
                // Ctrl+W
                while self.cursor > 0 && self.text[self.cursor - 1].is_whitespace() {
                    self.cursor -= 1;
                    self.text.remove(self.cursor);
                }
                while self.cursor > 0 && !self.text[self.cursor - 1].is_whitespace() {
                    self.cursor -= 1;
                    self.text.remove(self.cursor);
                }
            }
            _ => self.known = false,
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, _: &[u8], _: bool, action: char) {
        let first = params
            .iter()
            .next()
            .and_then(|p| p.first())
            .copied()
            .unwrap_or(0);
        match (action, first) {
            ('~', 200) => self.paste = true,
            ('~', 201) => self.paste = false,
            ('u', 13) if !self.paste => {
                let modifiers = params.iter().nth(1).unwrap_or(&[1]);
                if modifiers.first() == Some(&1) && modifiers.get(1).is_none_or(|event| *event != 3)
                {
                    self.submit(); // enhanced-keyboard Enter, excluding newline and release
                } else {
                    self.known = false;
                }
            }
            ('D', _) => self.cursor = self.cursor.saturating_sub(1),
            ('C', _) => self.cursor = (self.cursor + 1).min(self.text.len()),
            ('H', _) => self.cursor = 0,
            ('F', _) => self.cursor = self.text.len(),
            ('~', 3) if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            _ => self.known = false, // history, completion, or a terminal reply
        }
    }
}

#[derive(Default)]
struct Output {
    reset: bool,
}

impl vte::Perform for Output {
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        let first = params
            .iter()
            .next()
            .and_then(|p| p.first())
            .copied()
            .unwrap_or(0);
        if !ignore && intermediates.is_empty() && action == 'J' && matches!(first, 2 | 3) {
            // Inline Codex purges scrollback (ED3); fullscreen Codex clears the screen (ED2).
            self.reset = true;
        }
    }
}

impl ClearObserver {
    /// `visible` is the live composer, never historical transcript text. It recovers input
    /// after history/completion shortcuts, whose edits are owned by Codex rather than cide.
    pub fn input(&mut self, bytes: &[u8], visible: Option<&str>) {
        if !self.input.known
            && let Some(visible) = visible
        {
            self.input.reset();
            self.input.text = visible.chars().take(1024).collect();
            self.input.cursor = self.input.text.len();
        }
        self.input.live = visible.is_some();
        self.input.clear = false;
        for &byte in bytes {
            if self.escape_pending && !matches!(byte, b'[' | b'O') {
                self.input.reset();
                self.submitted = None;
            }
            self.escape_pending = byte == 27;
            // VTE ignores DEL; terminals use it for Backspace in the outbound stream.
            if byte == 127 {
                vte::Perform::execute(&mut self.input, byte);
            } else {
                self.input_parser.advance(&mut self.input, &[byte]);
            }
        }
        if self.input.clear {
            self.submitted = Some(Instant::now());
        } else if bytes.contains(&3) {
            self.submitted = None;
        }
    }

    /// Called only on bytes read from the live child, never a reattach or scrollback replay.
    pub fn output(&mut self, bytes: &[u8]) -> bool {
        self.output.reset = false;
        self.output_parser.advance(&mut self.output, bytes);
        if self.output.reset {
            return self
                .submitted
                .take()
                .is_some_and(|at| at.elapsed() < Duration::from_secs(5));
        }
        false
    }

    pub fn forget_input(&mut self) {
        self.input = Input::default();
        self.input_parser = vte::Parser::new();
        self.submitted = None;
        self.escape_pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESET: &[u8] = b"\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H";

    #[test]
    fn composer_is_the_cursor_row_not_history_or_a_dialog() {
        let mut vt = vt100::Parser::new(8, 80, 100);
        vt.process("› /clear from earlier\r\nAllow this command?\r\n  1. Yes".as_bytes());
        assert_eq!(composer(&crate::screen::capture(vt.screen())), None);
        vt.process("\x1b[6;1H› /clear\x1b[7;1H  /clear suggestion\x1b[6;9H".as_bytes());
        assert_eq!(
            composer(&crate::screen::capture(vt.screen())),
            Some("/clear".into())
        );
        vt.process(b"\x1b[?25l");
        assert_eq!(composer(&crate::screen::capture(vt.screen())), None);
    }

    #[test]
    fn clear_survives_every_input_and_output_chunk_boundary() {
        for command in [
            &b"/clear\r"[..],
            &b"\x1b[200~/clear named\x1b[201~\r"[..],
            &b"/clear\x1b[13u"[..],
        ] {
            for i in 0..=command.len() {
                for j in 0..=RESET.len() {
                    let mut observer = ClearObserver::default();
                    observer.input(&command[..i], Some(""));
                    observer.input(&command[i..], Some(""));
                    let count = usize::from(observer.output(&RESET[..j]))
                        + usize::from(observer.output(&RESET[j..]));
                    assert_eq!(count, 1, "input {i}, output {j}, {command:?}");
                    assert!(!observer.output(RESET), "a redraw is not another clear");
                }
            }
        }
    }

    #[test]
    fn unsubmitted_cancelled_prose_multiline_paste_and_modals_never_clear() {
        for input in [
            &b"/clear"[..],
            &b"/clear\x03\r"[..],
            &b"mention /clear\r"[..],
            &b"/clearly\r"[..],
            &b"\x1b[200~/clear\ntext\x1b[201~\r"[..],
        ] {
            let mut observer = ClearObserver::default();
            observer.input(input, Some(""));
            assert!(!observer.output(RESET), "{input:?}");
        }
        let mut observer = ClearObserver::default();
        observer.input(b"/clear\r", None);
        assert!(!observer.output(RESET));
        observer.input(b"/clear", Some(""));
        observer.input(b"\x1b", Some("/clear"));
        observer.input(b"\r", Some(""));
        assert!(!observer.output(RESET));
    }

    #[test]
    fn edits_completion_and_fullscreen_reset_work() {
        for input in [
            &b"/cleax\x7fr\r"[..],
            &b"discard\x15/clear\r"[..],
            &b"x/clear\x01\x1b[3~\x05\r"[..],
        ] {
            let mut observer = ClearObserver::default();
            observer.input(input, Some(""));
            assert!(observer.output(b"\x1b[1;1H\x1b[2J"));
        }
        let mut observer = ClearObserver::default();
        observer.input(b"/cl\t", Some(""));
        observer.input(b"\r", Some("/clear"));
        assert!(observer.output(RESET));
    }

    #[test]
    fn output_text_and_stale_or_unarmed_resets_do_not_clear() {
        let mut observer = ClearObserver::default();
        assert!(!observer.output(RESET));
        for key in [b"\x1b[13;2u".as_slice(), b"\x1b[13;1:3u".as_slice()] {
            let mut observer = ClearObserver::default();
            observer.input(b"/clear", Some(""));
            observer.input(key, Some("/clear"));
            assert!(!observer.output(RESET), "newline and release do not submit");
        }
        observer.input(b"/clear\r", Some(""));
        assert!(!observer.output(b"documentation: /clear resets a session"));
        assert!(!observer.output(b"\x1b]0;/clear\x07\x1b[0J"));
        observer.submitted = Some(Instant::now() - Duration::from_secs(6));
        assert!(!observer.output(RESET));
    }
}
