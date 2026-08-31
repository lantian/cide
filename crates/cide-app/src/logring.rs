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

use std::collections::{HashMap, VecDeque};

use cide_ipc::SessionId;
use parking_lot::Mutex;

/// How many rendered lines one session can have looked at again.
///
/// A screenful is 50; a person chasing something in a busy service scrolls back a few hundred.
/// Past a couple of thousand the answer is a search tool, not a scrollback. At roughly 200
/// bytes a line this is ~400 KiB for a session that has been logging hard, and nothing for
/// every other pane, which holds an empty ring.
const CAP: usize = 2_000;

#[derive(Default)]
struct Ring {
    /// The next handle to hand out. Never reused, never restarted: a stale handle from a line
    /// that has since been evicted must answer *gone*, not somebody else's event.
    next: u64,
    entries: VecDeque<(u64, String)>,
}

/// Every session's ring. Managed state, one per process.
#[derive(Default)]
pub struct JsonLogRing {
    sessions: Mutex<HashMap<SessionId, Ring>>,
}

impl JsonLogRing {
    /// Keep one raw line and answer the handle that finds it again.
    pub fn record(&self, session: SessionId, raw: &str) -> u64 {
        let mut sessions = self.sessions.lock();
        let ring = sessions.entry(session).or_default();
        let handle = ring.next;
        ring.next += 1;
        ring.entries.push_back((handle, raw.to_string()));
        while ring.entries.len() > CAP {
            ring.entries.pop_front();
        }
        handle
    }

    /// The raw line behind a handle, or `None` once it has aged out of the ring.
    pub fn get(&self, session: SessionId, handle: u64) -> Option<String> {
        let sessions = self.sessions.lock();
        let ring = sessions.get(&session)?;
        ring.entries
            .iter()
            .find(|(id, _)| *id == handle)
            .map(|(_, raw)| raw.clone())
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

    #[test]
    fn a_handle_finds_the_line_it_was_minted_for() {
        let ring = JsonLogRing::default();
        let s = session();
        let first = ring.record(s, r#"{"level":"info","msg":"a"}"#);
        let second = ring.record(s, r#"{"level":"info","msg":"b"}"#);
        assert_ne!(first, second, "two events are two entries, however alike");
        assert_eq!(
            ring.get(s, first).as_deref(),
            Some(r#"{"level":"info","msg":"a"}"#)
        );
        assert_eq!(
            ring.get(s, second).as_deref(),
            Some(r#"{"level":"info","msg":"b"}"#)
        );
        // Another session's handle is not this one's.
        assert_eq!(ring.get(session(), first), None);
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
        assert!(ring.get(s, a).unwrap().contains("\"pid\":1"));
        assert!(ring.get(s, b).unwrap().contains("\"pid\":2"));
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
        assert_eq!(ring.get(s, newest).as_deref(), Some("newest"));
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
