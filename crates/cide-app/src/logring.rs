//! The raw text of the log lines a shell pane has rendered, so one can be looked at whole.
//!
//! # Why anything is kept at all
//!
//! `cide_core::jsonlog` turns a structured log line into a one-line summary, and the summary
//! is lossy in one direction that matters: a nested object arrives as compact JSON and a wide
//! event wraps. The obvious answer — scroll back and read the original — does not exist here,
//! because the rewrite happens *above* the vt100 mirror (see `cide_pty::LineRender`), so the
//! bytes the child wrote are in no buffer anywhere. Either the raw line is kept here or it is
//! gone.
//!
//! # The shape, and what it costs
//!
//! One ring per session, [`CAP`] lines deep, oldest dropped. The renderer runs on the
//! coalescer thread — the thread every byte of every session flows through — so recording is
//! one short lock and a push, taken only for lines that actually rendered; a pane printing
//! prose or running a TUI never touches this. A session's ring dies with the session, in
//! `SessionRegistry::remove`, not at child exit: a dead pane's scrollback is still on screen
//! and still worth clicking.
//!
//! The handle handed to the terminal is a sequence number, not a hash of the rendering. Two
//! events that render identically are still two entries, and the one clicked is the one that
//! is shown — which text matching could not promise.
//!
//! # The clock beside each line
//!
//! Every entry also carries the moment it reached cide, because the line itself often does
//! not. A structured log line has its own timestamp and a codex `item.completed` has none at
//! all — nor a date, nor a clock, nor a duration — so the card that opens a run's tool call
//! had nothing to say about *when* the command ran, and a person reading a run back after the
//! fact asks exactly that. The wall clock is read here, on the coalescer thread, at the only
//! moment the line is in hand; a card that asked `Date.now()` on open would stamp every call
//! with the time it was read rather than the time it happened.

use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use cide_ipc::SessionId;
use parking_lot::Mutex;

/// How many rendered lines one session can have looked at again.
///
/// A screenful is 50; a person chasing something in a busy service scrolls back a few hundred.
/// Past a couple of thousand the answer is a search tool, not a scrollback. At roughly 200
/// bytes a line this is ~400 KiB for a session that has been logging hard, and nothing for
/// every other pane, which holds an empty ring.
const CAP: usize = 2_000;

/// And how many bytes, which since M62 is the binding cap rather than the generous one.
///
/// The "roughly 200 bytes a line" above was true while every kept line was a log record or a
/// tool call. An agent run now keeps its *reasoning* too — the row draws none of it, so this
/// ring is the only copy — and a single block of thinking is two to twenty kilobytes. At
/// [`CAP`] entries that is tens of megabytes a session, held until `SessionRegistry::remove`
/// and not the child's exit, across every finished-but-attached run pane at once.
///
/// A second cap rather than a smaller [`CAP`], because the two answer different questions: the
/// entry count is how far back a person may click, and this is what the process will spend. The
/// count is what a run of ordinary tool calls hits; this is what a run that thinks hard hits.
const BYTES_CAP: usize = 8 * 1024 * 1024;

/// One kept line: the bytes as the child wrote them, and the moment they arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub raw: String,
    /// Milliseconds since the Unix epoch, read as the line reached the renderer — see the
    /// module header for why the clock is taken here and not where the line is shown.
    pub recorded_unix_ms: u64,
}

#[derive(Default)]
struct Ring {
    /// The next handle to hand out. Never reused, never restarted: a stale handle from a line
    /// that has since been evicted must answer *gone*, not somebody else's event.
    next: u64,
    /// The raw bytes currently held, kept alongside rather than summed on every push — the
    /// renderer calls this on the coalescer thread, the thread every byte of every session
    /// flows through, and a walk of two thousand entries per line does not belong there.
    bytes: usize,
    entries: VecDeque<(u64, Kept)>,
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Every session's ring. Managed state, one per process.
#[derive(Default)]
pub struct JsonLogRing {
    sessions: Mutex<HashMap<SessionId, Ring>>,
}

impl JsonLogRing {
    /// Keep one raw line, stamped with the wall clock now, and answer the handle that finds it
    /// again.
    pub fn record(&self, session: SessionId, raw: &str) -> u64 {
        self.record_at(session, raw, now_unix_ms())
    }

    /// [`Self::record`] with the clock injected — so a test can assert on the stamp, and so the
    /// agent stream hook can read the wall clock **once** and give the same moment to both this
    /// ring and the renderer's own `RenderState`. Two reads of `SystemTime::now()` for one line
    /// would be two different answers to "when did this arrive". (M62)
    pub(crate) fn record_at(&self, session: SessionId, raw: &str, recorded_unix_ms: u64) -> u64 {
        let mut sessions = self.sessions.lock();
        let ring = sessions.entry(session).or_default();
        let handle = ring.next;
        ring.next += 1;
        ring.entries.push_back((
            handle,
            Kept {
                raw: raw.to_string(),
                recorded_unix_ms,
            },
        ));
        ring.bytes += raw.len();
        // Oldest first, on whichever cap bites. The eviction is the cost of keeping reasoning at
        // all: a thinking-heavy run ages its own earlier tool-call handles out faster, and a
        // click on a `#7` still on screen then says the line is no longer kept. Bounded memory
        // is worth that; an unbounded ring is not.
        while ring.entries.len() > CAP || (ring.bytes > BYTES_CAP && ring.entries.len() > 1) {
            if let Some((_, kept)) = ring.entries.pop_front() {
                ring.bytes = ring.bytes.saturating_sub(kept.raw.len());
            }
        }
        handle
    }

    /// The line behind a handle, or `None` once it has aged out of the ring.
    pub fn get(&self, session: SessionId, handle: u64) -> Option<Kept> {
        let sessions = self.sessions.lock();
        let ring = sessions.get(&session)?;
        ring.entries
            .iter()
            .find(|(id, _)| *id == handle)
            .map(|(_, kept)| kept.clone())
    }

    /// Drop a session's ring. Called from `SessionRegistry::remove` — see the module header
    /// for why that is the moment rather than the child's exit.
    pub fn forget(&self, session: SessionId) {
        self.sessions.lock().remove(&session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionId {
        SessionId::new()
    }

    fn raw(ring: &JsonLogRing, s: SessionId, handle: u64) -> Option<String> {
        ring.get(s, handle).map(|kept| kept.raw)
    }

    #[test]
    fn a_handle_finds_the_line_it_was_minted_for() {
        let ring = JsonLogRing::default();
        let s = session();
        let first = ring.record(s, r#"{"level":"info","msg":"a"}"#);
        let second = ring.record(s, r#"{"level":"info","msg":"b"}"#);
        assert_ne!(first, second, "two events are two entries, however alike");
        assert_eq!(
            raw(&ring, s, first).as_deref(),
            Some(r#"{"level":"info","msg":"a"}"#)
        );
        assert_eq!(
            raw(&ring, s, second).as_deref(),
            Some(r#"{"level":"info","msg":"b"}"#)
        );
        // Another session's handle is not this one's.
        assert_eq!(ring.get(session(), first), None);
    }

    /// The stamp is the clock as the line arrived, and it stays with the line: a card opened
    /// an hour later must say when the command ran, not when somebody looked.
    #[test]
    fn a_line_keeps_the_moment_it_arrived() {
        let ring = JsonLogRing::default();
        let s = session();
        let handle = ring.record_at(s, "x", 1_758_200_000_123);
        let kept = ring.get(s, handle).unwrap();
        assert_eq!(kept.recorded_unix_ms, 1_758_200_000_123);
        assert_eq!(kept.raw, "x");
        // And the ordinary road stamps something plausible rather than zero — the
        // `unwrap_or_default` in `now_unix_ms` is for a clock before 1970, not for every day.
        let live = ring.record(s, "y");
        assert!(ring.get(s, live).unwrap().recorded_unix_ms > 1_700_000_000_000);
    }

    /// Identical renderings stay distinguishable, which is the whole argument for a sequence
    /// number over a hash of the rendered text: the two lines below render to the same
    /// characters and carry different data.
    #[test]
    fn two_events_that_render_alike_are_still_two_entries() {
        let ring = JsonLogRing::default();
        let s = session();
        let a = ring.record(s, r#"{"level":"info","msg":"tick","pid":1}"#);
        let b = ring.record(s, r#"{"level":"info","msg":"tick","pid":2}"#);
        assert!(raw(&ring, s, a).unwrap().contains("\"pid\":1"));
        assert!(raw(&ring, s, b).unwrap().contains("\"pid\":2"));
    }

    /// Past the cap the oldest go, and their handles answer *gone* rather than a neighbour's
    /// line — which is what a handle reused after eviction would have done.
    #[test]
    fn an_evicted_handle_answers_nothing_at_all() {
        let ring = JsonLogRing::default();
        let s = session();
        let first = ring.record(s, "oldest");
        for i in 0..CAP {
            ring.record(s, &format!("line {i}"));
        }
        assert_eq!(ring.get(s, first), None);
        let newest = ring.record(s, "newest");
        assert_eq!(raw(&ring, s, newest).as_deref(), Some("newest"));
    }

    /// A run that thinks hard must not cost the process unboundedly, and the entry cap alone
    /// does not stop it: since M62 a kept line may be a whole block of reasoning, kilobytes of
    /// it, and [`CAP`] of those is tens of megabytes held until the *pane* closes. (M62)
    ///
    /// The other half of the claim is the one nothing could see before: eviction is now
    /// reachable long before two thousand lines, so a tool-call handle still on screen can
    /// answer *gone*. That is the price of keeping reasoning at all, and it is asserted here
    /// rather than discovered by a click.
    #[test]
    fn a_ring_of_long_lines_is_bounded_by_bytes_as_well_as_entries() {
        let ring = JsonLogRing::default();
        let s = session();
        let block = "x".repeat(64 * 1024);
        let first = ring.record(s, &block);
        for _ in 0..(BYTES_CAP / block.len() + 2) {
            ring.record(s, &block);
        }
        assert_eq!(ring.get(s, first), None, "the oldest went first");
        let held: usize = ring.sessions.lock()[&s].bytes;
        assert!(held <= BYTES_CAP, "{held}");
        assert!(
            ring.sessions.lock()[&s].entries.len() < CAP,
            "the byte cap bit long before the entry cap"
        );
        // And the ring still answers for what it holds.
        let newest = ring.record(s, "newest");
        assert_eq!(raw(&ring, s, newest).as_deref(), Some("newest"));
    }

    /// One line longer than the whole budget is still kept: a ring that evicted it would answer
    /// *gone* for a handle it minted one statement earlier, which is the one thing a handle
    /// promises not to do.
    #[test]
    fn a_single_line_over_the_budget_is_still_reachable() {
        let ring = JsonLogRing::default();
        let s = session();
        let huge = "y".repeat(BYTES_CAP + 4_096);
        let handle = ring.record(s, &huge);
        assert_eq!(raw(&ring, s, handle).map(|raw| raw.len()), Some(huge.len()));
    }

    #[test]
    fn forgetting_a_session_drops_everything_it_kept() {
        let ring = JsonLogRing::default();
        let s = session();
        let handle = ring.record(s, "x");
        ring.forget(s);
        assert_eq!(ring.get(s, handle), None);
    }
}
