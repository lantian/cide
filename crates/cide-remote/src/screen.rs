//! What a watching device is sent, and what it is spared. (M72)
//!
//! One [`Watch`] per (connection, session). It holds the grid's identity and a digest per line,
//! and turns a fresh [`ScreenCapture`] into either nothing at all or the few lines that moved.
//!
//! # Why the sender keeps the digests
//!
//! The alternative is asking the device what it has, which would put a round trip inside every
//! repaint and make the protocol stateful in the direction that breaks: a device whose reply is
//! lost would be stuck. Holding the digests here costs one `u64` per row per watcher — a few
//! hundred bytes — and makes a missed frame self-correcting, because the next tick diffs against
//! what was actually *sent*.
//!
//! # The tick, and why it is a tick
//!
//! The mirror is polled rather than subscribed to, and the whole argument is in
//! [`crate::host::RemoteHost::screen`]. What is here is the consequence: a repaint is whatever
//! changed between two reads, so the rate is a decision rather than the child's. It is fast while
//! anything is moving and slow when nothing is, because a terminal nobody is typing into is the
//! common case and a phone's battery is the thing being spent.

use std::time::Instant;

use cide_ipc::SessionId;
use cide_ipc::remote::ScreenUpdate;
use cide_ipc::screen::{Cursor, ScreenCapture, ScreenInfo};

/// One watcher's memory of a session's screen.
pub(crate) struct Watch {
    epoch: u64,
    /// `None` until the first capture: a watch that has sent nothing owes a full frame.
    seen: Option<Seen>,
    /// When something last changed, for the tick's own pacing.
    stirred: Instant,
    /// The digest of the prompt this watcher was last told about, if any.
    prompt: Option<String>,
}

/// What changed about a watched session's prompt.
pub(crate) enum PromptNews {
    Asking(Box<cide_ipc::remote::PermissionPrompt>),
    Gone(SessionId),
}

struct Seen {
    info: ScreenInfo,
    digests: Vec<u64>,
    cursor: Option<Cursor>,
}

impl Watch {
    pub(crate) fn new() -> Self {
        Self {
            epoch: 0,
            seen: None,
            stirred: Instant::now(),
            prompt: None,
        }
    }

    /// What to say about the prompt, if anything changed.
    ///
    /// Keyed on the digest rather than on the whole prompt: the digest already covers the
    /// question and every label, so two prompts that hash alike are the same question and
    /// re-sending one would redraw a card under somebody's thumb.
    pub(crate) fn prompt_news(
        &mut self,
        session: SessionId,
        prompt: Option<cide_ipc::remote::PermissionPrompt>,
    ) -> Option<PromptNews> {
        let now = prompt.as_ref().map(|p| p.digest.clone());
        if now == self.prompt {
            return None;
        }
        self.prompt = now;
        Some(match prompt {
            Some(prompt) => PromptNews::Asking(Box::new(prompt)),
            None => PromptNews::Gone(session),
        })
    }

    /// Whether this watch has been quiet long enough to be polled lazily.
    pub(crate) fn idle_for(&self, at: Instant) -> std::time::Duration {
        at.saturating_duration_since(self.stirred)
    }

    /// What to send, if anything.
    ///
    /// `None` is the ordinary answer for a terminal nobody is typing into, and sending nothing is
    /// the point — an idle pane must cost an idle socket.
    pub(crate) fn diff(
        &mut self,
        session: SessionId,
        capture: ScreenCapture,
    ) -> Option<ScreenUpdate> {
        let digests: Vec<u64> = capture.lines.iter().map(|line| line.digest()).collect();

        // A different grid is not a repaint of this one. A resize or an alternate-screen
        // transition re-numbers every row, so a line-by-line diff across it would be a list of
        // coincidences — hence a new epoch and the whole grid, always together.
        let changed_shape = self
            .seen
            .as_ref()
            .is_none_or(|seen| seen.info != capture.info);
        if changed_shape {
            self.epoch += 1;
            self.stirred = Instant::now();
            self.seen = Some(Seen {
                info: capture.info,
                digests,
                cursor: capture.cursor,
            });
            return Some(ScreenUpdate {
                session,
                epoch: self.epoch,
                full: true,
                info: capture.info,
                cursor: capture.cursor,
                lines: capture.lines,
            });
        }

        let seen = self.seen.as_mut().expect("just checked");
        let moved: Vec<_> = capture
            .lines
            .into_iter()
            .zip(&digests)
            .enumerate()
            .filter(|(row, (_, digest))| seen.digests.get(*row) != Some(*digest))
            .map(|(_, (line, _))| line)
            .collect();

        let cursor_moved = seen.cursor != capture.cursor;
        if moved.is_empty() && !cursor_moved {
            return None;
        }

        seen.digests = digests;
        seen.cursor = capture.cursor;
        self.stirred = Instant::now();
        Some(ScreenUpdate {
            session,
            epoch: self.epoch,
            full: false,
            info: capture.info,
            cursor: capture.cursor,
            lines: moved,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::screen::{ScreenLine, StyleRun};

    fn info() -> ScreenInfo {
        ScreenInfo {
            cols: 20,
            rows: 3,
            alt: false,
            app_cursor: false,
            bracketed_paste: false,
            mouse: cide_ipc::screen::MouseReporting::Off,
        }
    }

    fn line(row: u16, text: &str) -> ScreenLine {
        ScreenLine {
            row,
            wrapped: false,
            runs: vec![StyleRun {
                text: text.to_owned(),
                fg: None,
                bg: None,
                flags: 0,
            }],
        }
    }

    fn capture(rows: &[&str], cursor: Option<Cursor>) -> ScreenCapture {
        ScreenCapture {
            info: info(),
            cursor,
            lines: rows
                .iter()
                .enumerate()
                .map(|(i, t)| line(i as u16, t))
                .collect(),
        }
    }

    #[test]
    fn the_first_capture_is_always_whole() {
        let mut watch = Watch::new();
        let update = watch
            .diff(SessionId::new(), capture(&["a", "b", "c"], None))
            .expect("a first frame");
        assert!(update.full);
        assert_eq!(update.epoch, 1);
        assert_eq!(update.lines.len(), 3);
    }

    /// The property an idle pane depends on: nothing on the wire, so a terminal nobody is using
    /// costs a socket that says nothing.
    #[test]
    fn an_unchanged_screen_sends_nothing() {
        let mut watch = Watch::new();
        let session = SessionId::new();
        watch.diff(session, capture(&["a", "b", "c"], None));
        assert!(
            watch
                .diff(session, capture(&["a", "b", "c"], None))
                .is_none()
        );
    }

    #[test]
    fn only_the_lines_that_moved_are_sent() {
        let mut watch = Watch::new();
        let session = SessionId::new();
        watch.diff(session, capture(&["a", "b", "c"], None));

        let update = watch
            .diff(session, capture(&["a", "B", "c"], None))
            .expect("a repaint");
        assert!(!update.full);
        assert_eq!(update.epoch, 1, "the grid did not change identity");
        assert_eq!(update.lines.len(), 1);
        assert_eq!(update.lines[0].row, 1);
        assert_eq!(update.lines[0].runs[0].text, "B");
    }

    /// A cursor is its own reason to repaint, and on its own it costs no lines at all — which is
    /// most of what typing at a prompt looks like from here.
    #[test]
    fn a_cursor_that_moved_is_a_repaint_of_no_lines() {
        let mut watch = Watch::new();
        let session = SessionId::new();
        watch.diff(
            session,
            capture(&["a", "b", "c"], Some(Cursor { row: 0, col: 0 })),
        );

        let update = watch
            .diff(
                session,
                capture(&["a", "b", "c"], Some(Cursor { row: 0, col: 1 })),
            )
            .expect("a repaint");
        assert!(update.lines.is_empty());
        assert_eq!(update.cursor, Some(Cursor { row: 0, col: 1 }));
    }

    /// A resize re-numbers every row, so a line-by-line diff across it is a list of
    /// coincidences. The epoch is how a device knows to throw its cache away.
    #[test]
    fn a_resize_bumps_the_epoch_and_resends_everything() {
        let mut watch = Watch::new();
        let session = SessionId::new();
        watch.diff(session, capture(&["a", "b", "c"], None));

        let mut wider = capture(&["a", "b", "c"], None);
        wider.info.cols = 40;
        let update = watch.diff(session, wider).expect("a repaint");
        assert!(update.full);
        assert_eq!(update.epoch, 2);
        assert_eq!(update.lines.len(), 3, "an unchanged line was withheld");
    }

    #[test]
    fn entering_the_alternate_screen_is_a_new_grid() {
        let mut watch = Watch::new();
        let session = SessionId::new();
        watch.diff(session, capture(&["a", "b", "c"], None));

        let mut alt = capture(&["a", "b", "c"], None);
        alt.info.alt = true;
        let update = watch.diff(session, alt).expect("a repaint");
        assert!(update.full);
        assert_eq!(update.epoch, 2);
    }
}
