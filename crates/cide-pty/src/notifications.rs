//! Live OSC 9 terminal notifications. No screen scraping or replayed-screen side effects.

#[derive(Default)]
pub struct Osc9 {
    parser: vte::Parser,
    notifications: Notifications,
}

#[derive(Default)]
struct Notifications(usize);

impl vte::Perform for Notifications {
    fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
        // OSC 9;4;… is a progress report, not a notification. Codex's notification
        // body can contain semicolons, so do not require exactly two parameters.
        if matches!(params, [b"9", body, ..] if !body.is_empty() && *body != b"4") {
            self.0 += 1;
        }
    }
}

impl Osc9 {
    /// Count complete notifications, retaining unfinished sequences across reads.
    pub fn process(&mut self, bytes: &[u8]) -> usize {
        self.notifications.0 = 0;
        self.parser.advance(&mut self.notifications, bytes);
        self.notifications.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_terminators_work_at_every_chunk_boundary() {
        for sequence in [
            &b"\x1b]9;Approval requested: command; arguments\x07"[..],
            &b"\x1b]9;Approval requested: command\x1b\\"[..],
        ] {
            for split in 0..=sequence.len() {
                let mut parser = Osc9::default();
                let count = parser.process(&sequence[..split]) + parser.process(&sequence[split..]);
                assert_eq!(count, 1, "split {split}");
                assert_eq!(parser.process(b"ordinary output"), 0);
            }
        }
    }

    #[test]
    fn ordinary_output_other_osc_and_progress_are_not_notifications() {
        let mut parser = Osc9::default();
        assert_eq!(parser.process(b"Approval requested: text\x07\x1b]0;title\x07\x1b]9;4;1;50\x07\x1b]9;\x07\x1b]52;c;YWJj\x07"), 0);
        assert_eq!(parser.process(b"\x1b]9;first\x07\x1b]9;second\x1b\\"), 2);
    }
}
