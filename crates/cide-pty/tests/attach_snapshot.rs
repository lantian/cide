//! The attach cut point: a pane must not be sent bytes its own snapshot already held.
//!
//! # Why this is an integration test and not a unit test
//!
//! The defect lives in the gap between two *threads* — the coalescer, which feeds the mirror
//! per chunk and the sinks per flush, and whichever thread services the attach. A unit test
//! that drove `broadcast` directly would be testing a function that was never wrong. The only
//! way to observe the window is to open a real PTY, let a real child write into it, and attach
//! while the two are out of step.
//!
//! # Why the window is *provable* here and not merely hoped for
//!
//! Every case below attaches a witness sink first, then polls the mirror until the child's
//! output has reached it, then checks that the witness has **not** yet been given those bytes.
//! That last check is what says "the flush has not happened yet, so `pending` is holding
//! them" — i.e. that this run genuinely landed inside the window. A run that misses the
//! window (the flush beat the poll) is not a pass and is not a failure: it is discarded and
//! retried, and the test fails only if it can never land there. Without that, a fix that did
//! nothing at all would pass on any machine where the 8 ms flush happened to win.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cide_pty::{PtySession, Sink, SpawnSpec};

/// Unlikely to appear in a shell's own output, and one token so a partial frame cannot
/// half-match it.
const SENTINEL: &str = "CIDESENTINEL";

/// How many times a case may miss the flush window before it gives up.
///
/// Each attempt spawns a child and races an 8 ms timer, so missing every one of these would
/// mean the poll loop below is slower than the flush interval on this machine — a real
/// finding about the environment, not a flake, and worth failing for.
const ATTEMPTS: usize = 40;

fn recording_sink() -> (Arc<dyn Sink>, mpsc::Receiver<Vec<u8>>) {
    let (tx, rx) = mpsc::channel();
    let sink: Arc<dyn Sink> = Arc::new(move |bytes: &[u8]| tx.send(bytes.to_vec()).is_ok());
    (sink, rx)
}

/// Everything this sink has been given so far, without blocking.
fn drained(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        out.extend_from_slice(&chunk);
    }
    out
}

/// Everything this sink is given over the next `dur`.
fn collected_for(rx: &mpsc::Receiver<Vec<u8>>, dur: Duration) -> Vec<u8> {
    let deadline = Instant::now() + dur;
    let mut out = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(chunk) => out.extend_from_slice(&chunk),
            Err(_) => break,
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

fn occurrences(haystack: &[u8], needle: &str) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle.as_bytes())
        .count()
}

/// A child that prints the sentinel once and then stays alive, so nothing else lands in the
/// stream to confuse the assertions.
fn sentinel_child() -> Arc<PtySession> {
    let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
        .arg("-c")
        .arg(format!("printf '{SENTINEL}'; sleep 30"));
    PtySession::spawn(spec).expect("spawn sh")
}

/// Poll until the mirror shows the sentinel. Returns false if it never does.
fn wait_for_mirror(session: &PtySession) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if contains(&session.screen_state(), SENTINEL) {
            return true;
        }
        // No sleep. The whole point is to observe the mirror as soon as it advances, which is
        // several milliseconds before the coalescer's flush deadline.
        std::hint::spin_loop();
    }
    false
}

/// **The regression.**
///
/// The snapshot the attaching pane is given already contains the sentinel, and the sink it
/// registered must therefore never be sent the sentinel as live output. Before
/// `attach_with_snapshot` this failed: `screen_state()` read the mirror, `attach()` registered
/// the sink, and the coalescer's next flush handed the still-pending bytes to a sink that had
/// just been shown them — a duplicated prompt on every re-dock and every rehydration.
#[test]
fn attaching_inside_the_flush_window_does_not_replay_the_snapshot() {
    let mut landed = 0;

    for _ in 0..ATTEMPTS {
        let session = sentinel_child();
        let (witness, witness_rx) = recording_sink();
        session.attach(witness);

        if !wait_for_mirror(&session) {
            session.kill();
            continue;
        }
        if contains(&drained(&witness_rx), SENTINEL) {
            // The flush beat the poll; this run says nothing either way.
            session.kill();
            continue;
        }
        landed += 1;

        let (sink, rx) = recording_sink();
        let (_id, snapshot) = session.attach_with_snapshot(sink);

        assert!(
            contains(&snapshot, SENTINEL),
            "the snapshot must carry the screen the pane is about to paint"
        );

        // Three flush intervals: long enough that a replay would certainly have arrived.
        let live = collected_for(&rx, Duration::from_millis(200));
        assert!(
            !contains(&live, SENTINEL),
            "the sink was replayed bytes its own snapshot already held: {:?}",
            String::from_utf8_lossy(&live)
        );

        // And the sink that was there all along is still owed them exactly once — the fix
        // must not have swallowed the flush to achieve the above.
        let witnessed = drained(&witness_rx);
        assert_eq!(
            occurrences(&witnessed, SENTINEL),
            1,
            "the already-attached sink lost or doubled the flush: {:?}",
            String::from_utf8_lossy(&witnessed)
        );

        session.kill();
    }

    assert!(
        landed > 0,
        "never observed the mirror ahead of the sinks in {ATTEMPTS} attempts — \
         this test cannot see the window it exists to close"
    );
}

/// Output produced *after* the cut point still reaches the new sink.
///
/// The cheapest way to pass the test above is to attach a sink that never receives anything,
/// so this is the other half of the claim.
#[test]
fn a_sink_attached_with_a_snapshot_still_receives_what_comes_next() {
    let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
        .arg("-c")
        .arg("cat");
    let session = PtySession::spawn(spec).expect("spawn sh");

    let (sink, rx) = recording_sink();
    let (_id, _snapshot) = session.attach_with_snapshot(sink);

    session.write(b"hello\n".to_vec());

    let live = collected_for(&rx, Duration::from_secs(5));
    assert!(
        contains(&live, "hello"),
        "a freshly attached sink received nothing: {:?}",
        String::from_utf8_lossy(&live)
    );

    session.kill();
}

/// Attaching to a child that has already exited answers with the screen rather than hanging.
///
/// `attach_with_snapshot` takes the fallback on the `exited` flag alone, without ever asking
/// the coalescer, so that is the path this exercises — and the thing that must not happen is
/// the two-second `ATTACH_TIMEOUT` being paid on every rehydration of a dead pane, of which a
/// restored workspace has one per pane. (The coalescer having *ended* is a likely consequence
/// and not the trigger; see the mirror wait in the body, which is there because the two are
/// genuinely separate events.)
#[test]
fn attaching_to_a_dead_session_answers_from_the_mirror_without_waiting() {
    let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
        .arg("-c")
        .arg(format!("printf '{SENTINEL}'"));
    let session = PtySession::spawn(spec).expect("spawn sh");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !session.has_exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(session.has_exited(), "child never finished");

    // …and then for the mirror, which is a different event. `has_exited()` is deliberately the
    // *earliest* honest answer: it is set by whichever of two observers gets there first, the
    // coalescer seeing EOF on the master or the reaper seeing `wait()` return. When the reaper
    // wins, the child's bytes are still on their way into the mirror and the snapshot below is
    // legitimately empty — so this test was asserting something `has_exited()` does not promise
    // and its own header assumes ("the coalescer thread has ended by then"). It held on every
    // macOS run measured here — 200 spawns, the coalescer first every time, under load too — and
    // did not hold on a Linux runner, which is where it failed.
    //
    // `full_state`, not `screen_state`: the exited arm of `attach_with_snapshot` answers with
    // the whole transcript, so that is the buffer whose readiness this is about.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !contains(&session.full_state(), SENTINEL) && Instant::now() < deadline {
        std::hint::spin_loop();
    }
    assert!(
        contains(&session.full_state(), SENTINEL),
        "the child's output never reached the mirror at all"
    );

    // Measured from here, so the wait above cannot pay for the fallback the assertion below is
    // about.
    let started = Instant::now();
    let (sink, _rx) = recording_sink();
    let (_id, snapshot) = session.attach_with_snapshot(sink);
    let took = started.elapsed();

    assert!(
        contains(&snapshot, SENTINEL),
        "a dead session still has a screen worth painting"
    );
    assert!(
        took < Duration::from_millis(500),
        "attaching to a dead session waited {took:?} — the fallback is not being taken"
    );
    assert_eq!(session.sink_count(), 1, "the sink must still be registered");
}
