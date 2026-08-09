//! The Rust half of the keystroke budget, at 100 000 candidates.
//!
//! # What this measures, and what it cannot
//!
//! The milestone asks for keystroke-to-paint under 16 ms. That interval has two halves:
//!
//! * **Here.** What `cide_app::cmd::picker::picker_query` does — [`Matcher::query`] then
//!   [`Matcher::frame`] — against a matcher holding a large repository. Everything the
//!   command adds on top is a `DashMap` lookup and serde, so this is the whole of the Rust
//!   cost.
//! * **Not here.** Serialising the frame over IPC, React rendering it, and the compositor
//!   putting it on the glass. That half needs a window, and this crate must not link one.
//!   **This file therefore does not establish the 16 ms criterion.** It establishes that the
//!   backend leaves nearly all of the budget to the frontend, which is the part that can be
//!   verified without a GUI.
//!
//! # Why the cost is size-independent by construction
//!
//! [`Matcher::frame`] advances the worker with `tick(10)` and then copies at most `limit`
//! rows. Neither term is a function of how many candidates exist or how many matched: a
//! query that hits 100 000 paths does the same bounded work as one that hits three, and
//! reports `running: true` so the overlay asks again. The measurements are taken at 100 000
//! candidates precisely because that is where a design with an unbounded term would show.
//!
//! # Wall clock on a shared machine
//!
//! A timing assertion that fails when somebody else starts a build is a bad test. Measured:
//! every percentile here — including the fastest of 78 samples — inflates by more than 20x
//! between an idle box and one at twice its core count, which is the scheduler being
//! measured and not this crate. So the split is:
//!
//! * the default-suite test carries every property that has no clock in it — the counts, the
//!   caps, the `running` flag's honesty — and one deliberately loose timing check that a
//!   heavily loaded machine still passes;
//! * [`the_keystroke_budget_holds_on_an_idle_machine`] asserts the median, p90 and worst case
//!   against the real budget. It is `#[ignore]`d because it needs a machine that is not doing
//!   anything else, and it prints its distribution so the numbers can be read rather than
//!   inferred from a pass.
//!
//! Both run on a dev-profile build, so release only has more headroom.

use std::time::{Duration, Instant};

use cide_search::{Candidate, MAX_FRAME, Matcher, NucleoMatcher};

/// A repository's worth of candidates. 100 000 is the milestone's figure.
const CANDIDATES: usize = 100_000;

/// Rows `picker_query` asks for by default. The overlay draws about twelve; the rest is
/// scroll headroom.
const LIMIT: usize = 50;

/// The whole keystroke-to-paint budget. Nothing in the Rust half may spend all of it.
const FRAME_BUDGET: Duration = Duration::from_millis(16);

/// The `tick` budget [`Matcher::frame`] hands the worker, which is what puts a ceiling on a
/// single call. A query too big to score in this long yields a *partial* frame with
/// `running: true` rather than blocking, so reaching the ceiling is the design working: the
/// overlay paints what there is and asks again. That is why the strict test bounds the
/// median and the p90 — what a typist experiences — and only asks of the worst case that it
/// be a tick and not a scoring pass.
const TICK_BUDGET: Duration = Duration::from_millis(10);

/// Paths shaped like the ones a real project produces, so scoring does real work: several
/// components, repeated component names, and a common extension.
fn corpus() -> Vec<Candidate> {
    (0..CANDIDATES)
        .map(|i| {
            let text = format!(
                "crates/cide-{}/src/module{}/handler{i}.rs",
                ["fs", "search", "ipc", "core", "app"][i % 5],
                i % 997,
            );
            let value = format!("/repo/{text}");
            Candidate::new(text, value)
        })
        .collect()
}

/// A matcher holding the whole corpus, scored and settled.
///
/// Settling first is deliberate: the measurements are of querying a filled matcher, which is
/// where a picker on a large repo spends nearly all of its life. The streaming case — a
/// query answered while the walk is still injecting — is `cide-fs`'s `large_repo` test.
fn filled() -> NucleoMatcher {
    let matcher = NucleoMatcher::new();
    matcher.extend(&mut corpus().into_iter());
    let deadline = Instant::now() + Duration::from_secs(120);
    matcher.query("");
    loop {
        let frame = matcher.frame(LIMIT);
        if frame.total as usize == CANDIDATES && !frame.running {
            return matcher;
        }
        assert!(
            Instant::now() < deadline,
            "matcher never finished ingesting"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// One `query` + `frame` pair — exactly the body of `picker_query`.
fn keystroke(matcher: &NucleoMatcher, query: &str) -> (Duration, cide_ipc::PickerFrame) {
    let started = Instant::now();
    matcher.query(query);
    let frame = matcher.frame(LIMIT);
    (started.elapsed(), frame)
}

/// The literal path a user is imagined to be typing. It is a prefix of real candidates, so
/// the scoring is the kind that happens in use rather than a query that matches nothing.
const TYPED: &str = "crates/cide-search/src/module42/handler";

/// Type [`TYPED`] one character at a time, twice, checking each frame and timing each
/// keystroke.
///
/// Two passes because the first character of each is the expensive one: [`Matcher::query`]
/// only takes `nucleo`'s append fast path when the new query extends the old, so returning
/// to a one-character query forces a full rescore of all 100 000 candidates — the worst case
/// a user can produce, by holding backspace.
fn type_a_path(matcher: &NucleoMatcher) -> Vec<Duration> {
    let mut times = Vec::new();
    for _pass in 0..2 {
        for end in 1..=TYPED.len() {
            if !TYPED.is_char_boundary(end) {
                continue;
            }
            let (elapsed, frame) = keystroke(matcher, &TYPED[..end]);
            times.push(elapsed);
            assert_eq!(frame.query, TYPED[..end], "the frame echoes its own query");
            assert_eq!(frame.total as usize, CANDIDATES, "`total` never shrinks");
            assert!(frame.items.len() <= LIMIT, "a frame respects its limit");
        }
        // Back to empty, so the next pass starts from a full rescore, not a narrowing.
        matcher.query("");
    }
    times
}

struct Timings {
    best: Duration,
    p10: Duration,
    median: Duration,
    p90: Duration,
    worst: Duration,
}

fn report(label: &str, mut times: Vec<Duration>) -> Timings {
    times.sort_unstable();
    let t = Timings {
        best: times[0],
        p10: times[times.len() / 10],
        median: times[times.len() / 2],
        p90: times[times.len() * 9 / 10],
        worst: *times.last().unwrap(),
    };
    println!(
        "{label}: n={} best={:?} p10={:?} median={:?} p90={:?} max={:?}",
        times.len(),
        t.best,
        t.p10,
        t.median,
        t.p90,
        t.worst
    );
    t
}

/// The whole of the default-suite check: one matcher, one sequence, no clock where a
/// structural assertion will do.
///
/// It is one test rather than three because each would otherwise build its own
/// 100 000-candidate matcher with its own `nucleo` thread pool, and the harness runs them
/// side by side: three matchers contending in one binary changed the numbers the timing one
/// was measuring by an order of magnitude. Sharing a single matcher between parallel tests
/// was the alternative and is worse — the query is matcher-wide state, so interleaved tests
/// would be reading each other's.
#[test]
fn the_rust_half_of_a_keystroke_is_bounded_and_correct() {
    let matcher = filled();

    let t = report("typing a path", type_a_path(&matcher));
    // The single fastest of the ~78 keystrokes, against a budget-plus-a-tick. Both choices
    // were arrived at by measurement rather than taste: with 64 spinners on a 32-core box
    // (load average 76) this run reported best 2–13 ms, p10 12–27 ms, median 57–78 ms, so
    // preemption inflates even the fastest sample by 20x or more and every tighter or
    // higher percentile was flaky by construction.
    //
    // Be clear about how little this catches. It is a floor check, and a floor is where the
    // cheap keystrokes live: a `frame` mutated to loop on `tick` until the worker settled —
    // i.e. every keystroke waiting on the whole 100 000-candidate corpus — still reported
    // best 134 µs, median 2.2 ms, max 15.4 ms here and passed this line. What it does catch
    // is a per-keystroke cost that no longer has a fast case at all. The properties with
    // real teeth in this test are the structural ones below, which have no clock in them,
    // and the distribution bounds are in the `#[ignore]`d test.
    assert!(
        t.best < FRAME_BUDGET + TICK_BUDGET,
        "the fastest keystroke of the run cost {:?} in the Rust half alone — that is a \
         scoring pass over the corpus, not a tick of one",
        t.best
    );

    // --- the query shapes that cost the most ------------------------------------------

    // Every candidate matches: the state the overlay opens in, and the one where a picker
    // that returned its whole result set would put 100 000 rows on the IPC channel.
    let (_, all) = keystroke(&matcher, "s");
    assert_eq!(all.total as usize, CANDIDATES);
    assert!(all.items.len() <= LIMIT);
    // A frame cut short by the tick has to admit it. `running` is what makes a bounded call
    // honest rather than merely fast: it is the overlay's instruction to ask again, and a
    // frame that claimed to be settled while carrying a partial count would freeze the
    // readout on a wrong number.
    assert!(
        all.running || all.matched as usize == CANDIDATES,
        "a frame claiming to be settled reported {} of {CANDIDATES} matched",
        all.matched
    );

    // Nothing matches, which `nucleo` can only discover by scoring all 100 000. The frame
    // taken mid-rescore may still be showing the previous query's rows — that is what
    // `running` is for — so the count is asserted on the settled frame below.
    let (_, none) = keystroke(&matcher, "zzzzqqqq");
    assert_eq!(none.total as usize, CANDIDATES);
    assert_eq!(none.query, "zzzzqqqq");

    // --- the counts, once the worker has settled --------------------------------------

    let hit = settle(&matcher, TYPED);
    assert!(hit.matched > 0);
    // Matching is fuzzy, so the tail of a 50-row frame is legitimately made of subsequence
    // hits; what has to hold is that a path literally containing what was typed outranks
    // them.
    assert!(
        hit.items[0].text.starts_with(TYPED),
        "the top hit for a literal path prefix was {:?}",
        hit.items[0].text
    );

    let everything = settle(&matcher, "");
    assert_eq!(everything.matched as usize, CANDIDATES);
    assert_eq!(everything.total as usize, CANDIDATES);
    assert_eq!(everything.items.len(), LIMIT);

    let nothing = settle(&matcher, "zzzzqqqq");
    assert_eq!(nothing.matched, 0);
    assert_eq!(
        nothing.total as usize, CANDIDATES,
        "`total` is the corpus even when nothing matches"
    );
    assert!(nothing.items.is_empty());

    // --- what crosses the IPC boundary ------------------------------------------------

    // The Rust-side half of "JS heap flat regardless of repo size": the frontend cannot
    // retain rows it was never sent. Whether it then retains the ones it was is a frontend
    // property and is not tested here.
    settle(&matcher, "");
    let greedy = matcher.frame(CANDIDATES);
    assert_eq!(
        greedy.matched as usize, CANDIDATES,
        "all 100 000 matched, and a caller asked for all of them"
    );
    assert_eq!(greedy.items.len(), MAX_FRAME, "and got the cap");
    let bytes: usize = greedy
        .items
        .iter()
        .map(|r| r.text.len() + r.value.len() + r.indices.len() * 4)
        .sum();
    assert!(
        bytes < 64 * 1024,
        "the largest frame a caller can ask for carried {bytes} bytes"
    );
}

/// The budget itself, measured rather than inferred.
///
/// `#[ignore]`d because it is a wall-clock measurement: on a box under load the numbers are
/// the scheduler's and not this crate's, and they move by more than 20x. Run it on an
/// otherwise idle machine with
/// `cargo test -p cide-search --test latency -- --ignored --nocapture`.
///
/// Observed idle, dev profile, 100 000 candidates: best 0.12 ms, p10 0.14 ms,
/// median 1.25 ms, p90 4.7 ms, max 10.1 ms — the max being `tick` spending its whole budget
/// on the full-rescore keystrokes. So the Rust half of a keystroke typically costs under a
/// tenth of the 16 ms, and in the worst case hands back a partial frame rather than
/// overrunning it.
#[test]
#[ignore = "wall-clock measurement: meaningful only on an otherwise idle machine"]
fn the_keystroke_budget_holds_on_an_idle_machine() {
    let matcher = filled();
    let t = report("typing a path (idle)", type_a_path(&matcher));

    assert!(
        t.median < FRAME_BUDGET / 4,
        "the median keystroke cost {:?}; the 16 ms criterion assumes the backend is nearly \
         free and the paint gets the rest",
        t.median
    );
    assert!(
        t.p90 < FRAME_BUDGET,
        "the 90th-percentile keystroke cost {:?} in the Rust half alone, leaving nothing \
         for the paint",
        t.p90
    );
    assert!(
        t.worst < FRAME_BUDGET + TICK_BUDGET,
        "the worst keystroke cost {:?}, more than a tick's overrun — something waited on \
         the whole corpus",
        t.worst
    );
}

/// Poll until the worker settles, so a count assertion is about matching and not timing.
fn settle(matcher: &NucleoMatcher, query: &str) -> cide_ipc::PickerFrame {
    matcher.query(query);
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let frame = matcher.frame(LIMIT);
        if !frame.running {
            return frame;
        }
        assert!(
            Instant::now() < deadline,
            "worker never settled on {query:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
