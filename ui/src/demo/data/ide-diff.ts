/**
 * The ide-diff scene's proposal: Claude's `openDiff` on `crates/cide-pty/src/coalesce.rs`.
 *
 * The branch's story (`data/git.ts`) is a coalescer split out of `session.rs` that then learns
 * backpressure. `ORIGINAL` is the file just after the split — a plain byte-or-interval flush —
 * and `PROPOSED` is the agent's next turn: frames held past the ack window instead of sent to a
 * sink that is not keeping up, and counted, so the pressure shows. Three hunks (an import, a new
 * field and its initialiser, a rewritten `flush`) so the diff reads as a real edit, not a paste.
 */
import { HOME } from '../world'

export const COALESCE_PATH = `${HOME}/crates/cide-pty/src/coalesce.rs`

/** The request id the broker minted; the tab and the answer both name it. */
export const REQUEST_ID = 'b7e3c2a1-openDiff-coalesce'

export const ORIGINAL = `//! The coalescer: turns the reader's 20-200 byte reads into frames worth sending.
//!
//! Split out of \`session.rs\` so the flush rule can be tested without a PTY. The rule itself
//! is unchanged: flush on [\`FLUSH_BYTES\`] **or** [\`FLUSH_INTERVAL\`], whichever comes first,
//! capped at [\`MAX_FRAME\`]. Coalescing is a correctness requirement, not a tuning knob — see
//! the crate header for what an unbatched frame costs on the GTK main loop.

use std::time::{Duration, Instant};

use crate::{FLUSH_BYTES, FLUSH_INTERVAL, MAX_FRAME};

/// One batch of output, ready for the sinks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    bytes: Vec<u8>,
}

impl Frame {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// Accumulates reads until one of the flush rules fires.
#[derive(Debug)]
pub struct Coalescer {
    pending: Vec<u8>,
    since: Option<Instant>,
}

impl Coalescer {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(FLUSH_BYTES),
            since: None,
        }
    }

    /// Take one read from the reader thread.
    pub fn push(&mut self, chunk: &[u8], now: Instant) -> Option<Frame> {
        self.since.get_or_insert(now);
        self.pending.extend_from_slice(chunk);
        self.flush(now)
    }

    /// Cut a frame if the buffer is big enough or old enough.
    pub fn flush(&mut self, now: Instant) -> Option<Frame> {
        let due = self.since.is_some_and(|t| now.duration_since(t) >= FLUSH_INTERVAL);
        if self.pending.len() < FLUSH_BYTES && !due {
            return None;
        }
        let take = self.pending.len().min(MAX_FRAME);
        let bytes: Vec<u8> = self.pending.drain(..take).collect();
        self.since = if self.pending.is_empty() { None } else { Some(now) };
        Some(Frame { bytes })
    }

    /// How long until [\`Self::flush\`] would fire on time alone.
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        self.since.map(|t| FLUSH_INTERVAL.saturating_sub(now.duration_since(t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_read_waits_for_the_interval() {
        let mut c = Coalescer::new();
        let t = Instant::now();
        assert_eq!(c.push(b"$ ", t), None);
        assert!(c.flush(t + FLUSH_INTERVAL).is_some());
    }
}
`

export const PROPOSED = `//! The coalescer: turns the reader's 20-200 byte reads into frames worth sending.
//!
//! Split out of \`session.rs\` so the flush rule can be tested without a PTY. The rule itself
//! is unchanged: flush on [\`FLUSH_BYTES\`] **or** [\`FLUSH_INTERVAL\`], whichever comes first,
//! capped at [\`MAX_FRAME\`]. Coalescing is a correctness requirement, not a tuning knob — see
//! the crate header for what an unbatched frame costs on the GTK main loop.

use std::time::{Duration, Instant};

use crate::{CreditPolicy, FLUSH_BYTES, FLUSH_INTERVAL, MAX_FRAME};

/// One batch of output, ready for the sinks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    bytes: Vec<u8>,
}

impl Frame {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// Accumulates reads until one of the flush rules fires.
#[derive(Debug)]
pub struct Coalescer {
    pending: Vec<u8>,
    since: Option<Instant>,
    /// Bytes sent and not yet acked by the slowest sink.
    in_flight: usize,
    /// Frames held back on a full ack window: backpressure, made visible.
    pub held: u64,
}

impl Coalescer {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(FLUSH_BYTES),
            since: None,
            in_flight: 0,
            held: 0,
        }
    }

    /// Take one read from the reader thread.
    pub fn push(
        &mut self,
        chunk: &[u8],
        now: Instant,
        policy: &CreditPolicy,
    ) -> Option<Frame> {
        self.since.get_or_insert(now);
        self.pending.extend_from_slice(chunk);
        self.flush(now, policy)
    }

    /// Cut a frame if the buffer is big enough or old enough, and the sinks
    /// have room for it.
    ///
    /// A frame past the ack window is held, not dropped: the bytes stay in
    /// \`pending\`, the bounded channel behind us fills, and the child blocks in
    /// \`write()\` — the reader's structural backpressure, one hop closer to
    /// the webview.
    pub fn flush(&mut self, now: Instant, policy: &CreditPolicy) -> Option<Frame> {
        let due = self.since.is_some_and(|t| now.duration_since(t) >= FLUSH_INTERVAL);
        if self.pending.len() < FLUSH_BYTES && !due {
            return None;
        }
        if self.in_flight >= policy.high {
            self.held += 1;
            return None;
        }
        let room = policy.high - self.in_flight;
        let take = self.pending.len().min(MAX_FRAME).min(room);
        let bytes: Vec<u8> = self.pending.drain(..take).collect();
        self.in_flight += bytes.len();
        self.since = if self.pending.is_empty() { None } else { Some(now) };
        Some(Frame { bytes })
    }

    /// The slowest sink acked \`bytes\`; reopen the window.
    pub fn ack(&mut self, bytes: usize) {
        self.in_flight = self.in_flight.saturating_sub(bytes);
    }

    /// How long until [\`Self::flush\`] would fire on time alone.
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        self.since.map(|t| FLUSH_INTERVAL.saturating_sub(now.duration_since(t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_read_waits_for_the_interval() {
        let mut c = Coalescer::new();
        let p = CreditPolicy::default();
        let t = Instant::now();
        assert_eq!(c.push(b"$ ", t, &p), None);
        assert!(c.flush(t + FLUSH_INTERVAL, &p).is_some());
    }

    #[test]
    fn a_full_window_holds_the_frame() {
        let mut c = Coalescer::new();
        let p = CreditPolicy { high: 16, ..CreditPolicy::default() };
        let t = Instant::now();
        assert!(c.push(&[b'x'; 16], t + FLUSH_INTERVAL, &p).is_some());
        assert_eq!(c.push(b"more", t + FLUSH_INTERVAL * 2, &p), None);
        assert_eq!(c.held, 1);
    }
}
`
