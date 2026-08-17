//! Fuzzy pickers (files, commands, symbols) behind a [`Matcher`] trait, so `nucleo` staying
//! unreleased since 2024 is a swap and not a rewrite. (M8)
//!
//! # Why the trait is the point
//!
//! `nucleo` 0.5.0 was published in 2024-04 and there has been no release since. It is the
//! right matcher today — it is what Helix ships, it scores paths well, and its injector is
//! lock-free — but a picker whose call sites are written against `Nucleo<T>` directly is a
//! picker that cannot be moved off it. Everything outside this crate talks to [`Matcher`],
//! which is five methods wide and says nothing about how matching happens.
//!
//! # Streaming, and why it shapes the API
//!
//! The file walk takes seconds on a large repository, and Ctrl+P has to work in the first
//! frame. So candidates are *injected* while queries run, and a query answers with a
//! [`PickerFrame`] rather than a result set: the best rows so far, the two counts behind the
//! mock's `6 of 2,418` readout, and a `running` flag that tells the overlay to ask again.
//!
//! Frames are capped at [`MAX_FRAME`] rows. A keystroke that matched 100k paths must not put
//! 100k rows on the IPC channel to draw twelve of them.

//! # Two different jobs behind one crate name (M11)
//!
//! Everything above is the fuzzy picker. [`content`] is a *grep* — the same word, a
//! different thing: it reads file contents rather than ranking paths, it returns every match
//! rather than the best ones, and it does not score at all. They share this crate because
//! they share nothing else and both are "search" to the rest of the app; they share no code,
//! deliberately, because a subsequence score is the wrong ranking for a grep and a line
//! number is meaningless to a picker.

pub mod content;

use std::sync::atomic::{AtomicBool, Ordering};

use cide_ipc::{PickerFrame, PickerRow};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use parking_lot::{Mutex, RwLock};

/// The most rows any frame will carry, whatever the caller asks for.
pub const MAX_FRAME: usize = 200;

/// One candidate the user can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What is matched and drawn — for files, the path relative to its root.
    pub text: String,
    /// What picking it means: an absolute path, a command id. Never shown.
    pub value: String,
    /// Provenance, for a candidate that did not come from the project. (M16)
    ///
    /// Carried on the candidate rather than derived from the path at frame time, because the
    /// answer is not in the path: `…/registry/src/index.crates.io-6f17/serde-1.0.229/src/de/
    /// mod.rs` yields `serde-1.0.229` by string surgery and never `serde 1.0.229`, and for the
    /// SDK row it yields nothing recognisable at all. The resolver already knows; this is where
    /// it says so once, at injection time, for the life of the candidate.
    ///
    /// **Deliberately not part of the matched column.** `NucleoMatcher::push` writes only
    /// `text` into `columns[0]`, so typing `1.0.229` finds nothing — which is right: a version
    /// number is a label the user reads to tell two rows apart, not a thing they search for, and
    /// putting it in the haystack would make every crate's files match every other crate's
    /// version digits.
    pub source: Option<String>,
}

impl Candidate {
    pub fn new(text: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            value: value.into(),
            source: None,
        }
    }

    /// The same, from a package whose name and version the row should carry.
    pub fn from_source(
        text: impl Into<String>,
        value: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            value: value.into(),
            source: Some(source.into()),
        }
    }
}

/// A frame and the score behind each of its rows. See [`NucleoMatcher::frame_scored`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scored {
    pub frame: PickerFrame,
    /// Parallel to `frame.items`, and the same length.
    pub scores: Vec<u32>,
}

/// Interleave two scored frames into one, highest score first, project rows winning ties.
///
/// # Why a merge rather than a second column on one matcher
///
/// Two matchers is forced, for the reason `ProjectFs::symbol_matcher` already documents —
/// `Matcher::query` holds one query per matcher, so sharing would make each keystroke in one
/// overlay reset the other — plus one more that is specific to this feature: **nucleo is
/// append-only**. There is no un-inject. A user who turns *Search libraries* off after turning
/// it on could not have 32,000 candidates taken back out, and post-filtering a 200-row frame
/// would break both the row limit and the `6 of 2,418` counter, which come from the snapshot.
///
/// # The tie-break is toward the project, and it is load-bearing
///
/// Scores collide constantly: an empty query gives every row the same score, and `lib.rs`
/// typed in full matches this project's and `serde`'s identically. Ties therefore decide the
/// common case rather than an edge one, and the answer that is right in both is *the user's own
/// files first* — a picker whose first row for an empty query is somebody else's crate is a
/// picker that has stopped being about this project.
///
/// `matched` and `total` are sums: they describe the candidate set the user is now searching,
/// which is the whole point of having turned libraries on. `running` is the OR — a frame is
/// still growing if either side is.
///
/// The rows are already sorted within each side, so this is a two-finger merge and not a sort.
pub fn merge(project: Scored, libraries: Scored, limit: usize) -> PickerFrame {
    let limit = limit.min(MAX_FRAME);
    let mut items =
        Vec::with_capacity(limit.min(project.frame.items.len() + libraries.frame.items.len()));

    let mut left = project
        .frame
        .items
        .into_iter()
        .zip(project.scores)
        .peekable();
    let mut right = libraries
        .frame
        .items
        .into_iter()
        .zip(libraries.scores)
        .peekable();

    while items.len() < limit {
        // `>=`, not `>`: the project side takes the row on a tie. See the note above — with an
        // empty query every score is equal, so this branch *is* the ordering most of the time.
        let take_left = match (left.peek(), right.peek()) {
            (Some((_, l)), Some((_, r))) => l >= r,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let next = if take_left { left.next() } else { right.next() };
        match next {
            Some((row, _)) => items.push(row),
            None => break,
        }
    }

    PickerFrame {
        items,
        // Saturating rather than wrapping: the counter is a readout and a wrapped `total` is a
        // number that reads as a bug. Neither side can realistically reach `u32::MAX` — the
        // whole cargo registry on this machine is 127,055 files — so this is a guard, not a case.
        matched: project
            .frame
            .matched
            .saturating_add(libraries.frame.matched),
        total: project.frame.total.saturating_add(libraries.frame.total),
        running: project.frame.running || libraries.frame.running,
        // The project's, not a concatenation: both were parsed from the same string, and the
        // overlay compares this against what the user has typed to drop a stale frame.
        query: project.frame.query,
    }
}

/// A streaming fuzzy matcher.
///
/// Every method takes `&self`: the injector is lock-free and the query side is behind a
/// mutex, so a walk on eight threads and a keystroke on the IPC thread do not queue behind
/// each other. That is the property the picker's whole design depends on, and it would be
/// lost the moment this trait asked for `&mut self`.
pub trait Matcher: Send + Sync {
    /// Add candidates. Callable at any time, including while a query is being answered.
    fn extend(&self, items: &mut dyn Iterator<Item = Candidate>);

    /// Set the query.
    ///
    /// Implementations may use the fact that a query which extends the previous one only has
    /// to re-score the current matches.
    fn query(&self, text: &str);

    /// Advance the worker and take a frame of at most `limit.min(MAX_FRAME)` rows.
    fn frame(&self, limit: usize) -> PickerFrame;

    /// Drop every candidate. Used when a project is re-indexed.
    fn clear(&self);

    /// Whether new candidates have arrived, or a query has finished scoring, since the last
    /// frame was taken. Lets a caller poll without paying for a frame.
    fn dirty(&self) -> bool;
}

/// The `nucleo`-backed implementation.
///
/// # Lock order
///
/// `query` → `inner` → `highlight` / `injector`, and never the other way round. Two Tauri
/// worker threads answering two keystrokes really do run [`Matcher::query`] and
/// [`Matcher::frame`] against the same matcher concurrently, and `frame` taking `inner`
/// before `query` while `query` held `query` and waited on `inner` was a permanent hang of
/// both threads — `parking_lot` guards have no timeout, so the picker never came back. Any
/// new method must take these in the order they are declared here.
pub struct NucleoMatcher {
    inner: Mutex<Nucleo<Candidate>>,
    /// Held separately from the worker so that injection never waits on a query. Behind an
    /// `RwLock` only because `clear` has to replace it: pushes take the read side.
    injector: RwLock<nucleo::Injector<Candidate>>,
    /// The query as last parsed, for the append fast path and to echo in the frame.
    query: Mutex<String>,
    /// Reused across frames for the highlight offsets. `nucleo::Matcher::new` allocates the
    /// scoring matrix up front, and the overlay polls this several times a second while a
    /// walk is running.
    highlight: Mutex<nucleo::Matcher>,
    dirty: std::sync::Arc<AtomicBool>,
}

impl std::fmt::Debug for NucleoMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NucleoMatcher")
            .field("query", &*self.query.lock())
            .finish_non_exhaustive()
    }
}

impl Default for NucleoMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl NucleoMatcher {
    pub fn new() -> Self {
        let dirty = std::sync::Arc::new(AtomicBool::new(false));
        let notify = {
            let dirty = std::sync::Arc::clone(&dirty);
            std::sync::Arc::new(move || dirty.store(true, Ordering::Release))
        };
        // `match_paths` is not cosmetic: it changes the bonus table so that a match after a
        // `/` scores like a match at the start of a word. Without it `mod.rs` ranks the same
        // wherever the characters land, and typing `srmn` does not find `src/main.rs`.
        let nucleo = Nucleo::new(Config::DEFAULT.match_paths(), notify, None, 1);
        let injector = RwLock::new(nucleo.injector());
        Self {
            inner: Mutex::new(nucleo),
            injector,
            query: Mutex::new(String::new()),
            highlight: Mutex::new(nucleo::Matcher::new(Config::DEFAULT.match_paths())),
            dirty,
        }
    }

    /// Push one candidate. The hot path during a walk.
    pub fn push(&self, item: Candidate) {
        let injector = self.injector.read();
        injector.push(item, |item, columns| {
            columns[0] = item.text.as_str().into();
        });
    }

    /// [`Matcher::frame`], with the score of every row it returned.
    ///
    /// # Why the scores come back out at all
    ///
    /// Ctrl+P can now answer from **two** matchers — the project's and, when the user asks for
    /// it, the resolved libraries' — and nucleo has no notion of a second source. Merging two
    /// frames means ordering rows that were scored independently, and the only honest key is
    /// the score itself. Rescoring on this side would mean a third implementation of the
    /// ranking (`frame`'s, `rank`'s, and a new one) that could disagree with both.
    ///
    /// The score is free: `Pattern::indices` computes and returns it in order to produce the
    /// highlight offsets, and this method is what stopped throwing it away. Nothing about the
    /// single-matcher path changed — [`Matcher::frame`] is now one line over this.
    ///
    /// The vector is parallel to `frame.items` and the same length. A struct rather than a
    /// tuple because `(PickerFrame, Vec<u32>)` at a call site reads as neither.
    pub fn frame_scored(&self, limit: usize) -> Scored {
        let limit = limit.min(MAX_FRAME);
        // `query` before `inner`, which is the order `query()` takes them in. Taking them the
        // other way round deadlocks against a concurrent keystroke; see the lock-order note
        // on the struct. Holding the guard for the whole frame is also what makes the echoed
        // query honest: it is the query the pattern below was actually parsed from, not one a
        // keystroke landing mid-frame has already replaced.
        let query = self.query.lock();
        let mut nucleo = self.inner.lock();
        // 10ms: long enough to finish a small query outright, short enough that the IPC
        // thread is never held for a frame's worth of time.
        let status = nucleo.tick(10);
        self.dirty.store(false, Ordering::Release);

        let snapshot = nucleo.snapshot();
        let matched = snapshot.matched_item_count();
        let total = snapshot.item_count();
        let take = (matched as usize).min(limit) as u32;

        let mut matcher = self.highlight.lock();
        let pattern = snapshot.pattern().column_pattern(0);
        let mut indices = Vec::new();
        let mut scores = Vec::with_capacity(take as usize);
        let items: Vec<PickerRow> = snapshot
            .matched_items(..take)
            .map(|item| {
                indices.clear();
                // The score `indices` has always returned and this call has always thrown away.
                // Free — the pattern is scored to produce the offsets either way — and it is
                // what [`merge`] needs to interleave two matchers' frames without rescoring.
                //
                // `None` cannot happen here: these items are the snapshot's *matched* ones, so
                // the pattern matched them a moment ago. Zero is the honest fallback if nucleo
                // ever disagrees with itself — it sorts the row last rather than dropping it.
                let score = pattern
                    .indices(
                        item.matcher_columns[0].slice(..),
                        &mut matcher,
                        &mut indices,
                    )
                    .unwrap_or(0);
                indices.sort_unstable();
                indices.dedup();
                scores.push(score);
                PickerRow {
                    text: item.data.text.clone(),
                    value: item.data.value.clone(),
                    indices: indices.clone(),
                    source: item.data.source.clone(),
                }
            })
            .collect();

        Scored {
            frame: PickerFrame {
                items,
                matched,
                total,
                running: status.running,
                query: query.clone(),
            },
            scores,
        }
    }
}

impl Matcher for NucleoMatcher {
    fn extend(&self, items: &mut dyn Iterator<Item = Candidate>) {
        let injector = self.injector.read();
        for item in items {
            injector.push(item, |item, columns| {
                columns[0] = item.text.as_str().into();
            });
        }
    }

    fn query(&self, text: &str) {
        let mut previous = self.query.lock();
        if *previous == text {
            // The overlay polls with an unchanged query while the walk is still injecting.
            // Newly injected items are matched against the standing pattern by `tick`, so
            // reparsing here would only re-run a filter pass that has already been done.
            return;
        }
        // `append` promises the old query is a prefix of the new one, which lets `nucleo`
        // narrow the current match set instead of rescoring every item. Getting it wrong
        // loses matches silently, so it is derived here rather than trusted from the caller.
        let append = !previous.is_empty() && text.starts_with(previous.as_str());
        self.inner.lock().pattern.reparse(
            0,
            text,
            CaseMatching::Smart,
            Normalization::Smart,
            append,
        );
        *previous = text.to_string();
        self.dirty.store(true, Ordering::Release);
    }

    fn frame(&self, limit: usize) -> PickerFrame {
        self.frame_scored(limit).frame
    }
    fn clear(&self) {
        let mut nucleo = self.inner.lock();
        // `restart(true)` invalidates every outstanding injector, so a fresh one has to be
        // taken under the same lock — a walk still pushing into the old one would otherwise
        // be filling a queue nobody reads.
        nucleo.restart(true);
        *self.injector.write() = nucleo.injector();
        self.dirty.store(true, Ordering::Release);
    }

    fn dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }
}

/// Rank a fixed list of candidates in one call.
///
/// For the command palette, which has fifty entries and no streaming to do. It goes through
/// the same scoring as the file picker on purpose: two overlays that rank the same query
/// differently is the sort of thing that reads as a bug even when both answers are defensible.
pub fn rank(query: &str, items: &[Candidate], limit: usize) -> PickerFrame {
    let limit = limit.min(MAX_FRAME);
    let mut matcher = nucleo::Matcher::new(Config::DEFAULT.match_paths());
    let pattern = nucleo::pattern::Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

    let mut scored: Vec<(u32, usize)> = Vec::new();
    let mut buf = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let haystack = nucleo::Utf32Str::new(&item.text, &mut buf);
        if let Some(score) = pattern.score(haystack, &mut matcher) {
            scored.push((score, i));
        }
    }
    // Highest score first; ties keep the caller's order, which for a command palette is the
    // order the commands were registered in rather than something arbitrary.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let matched = scored.len() as u32;

    let mut indices = Vec::new();
    let rows = scored
        .into_iter()
        .take(limit)
        .map(|(_, i)| {
            let item = &items[i];
            indices.clear();
            let haystack = nucleo::Utf32Str::new(&item.text, &mut buf);
            pattern.indices(haystack, &mut matcher, &mut indices);
            indices.sort_unstable();
            indices.dedup();
            PickerRow {
                text: item.text.clone(),
                value: item.value.clone(),
                indices: indices.clone(),
                // Carried through from the candidate rather than hardcoded `None`, so this path
                // says the same thing as the streaming one. In practice every caller of `rank`
                // is the command palette, whose candidates have no provenance — but a `None`
                // written here would be a silent second rule, and the day something else ranks a
                // fixed list of library files it would lose the chip with no symptom.
                source: item.source.clone(),
            }
        })
        .collect();

    PickerFrame {
        items: rows,
        matched,
        total: items.len() as u32,
        running: false,
        query: query.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Poll until the worker settles, so an assertion is about matching and not timing.
    fn settle(matcher: &NucleoMatcher, limit: usize) -> PickerFrame {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let frame = matcher.frame(limit);
            if !frame.running || Instant::now() > deadline {
                return frame;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn candidates(texts: &[&str]) -> Vec<Candidate> {
        texts
            .iter()
            .map(|t| Candidate::new(*t, format!("/root/{t}")))
            .collect()
    }

    #[test]
    fn a_path_query_finds_the_path() {
        let matcher = NucleoMatcher::new();
        matcher
            .extend(&mut candidates(&["src/main.rs", "src/lib.rs", "docs/readme.md"]).into_iter());
        matcher.query("srmn");
        let frame = settle(&matcher, 10);
        assert_eq!(frame.items[0].text, "src/main.rs");
        assert_eq!(frame.items[0].value, "/root/src/main.rs");
        assert!(!frame.items[0].indices.is_empty());
        assert_eq!(frame.total, 3);
        assert_eq!(frame.query, "srmn");
    }

    #[test]
    fn the_counts_are_the_ones_the_status_line_shows() {
        let matcher = NucleoMatcher::new();
        let mut items = candidates(&["a.rs", "b.rs", "c.txt"]).into_iter();
        matcher.extend(&mut items);
        matcher.query("rs");
        let frame = settle(&matcher, 10);
        assert_eq!(frame.matched, 2, "{:?}", frame.items);
        assert_eq!(frame.total, 3);
    }

    #[test]
    fn a_frame_never_carries_more_than_the_cap() {
        let matcher = NucleoMatcher::new();
        let items: Vec<Candidate> = (0..10_000)
            .map(|i| Candidate::new(format!("src/file{i}.rs"), format!("/root/{i}")))
            .collect();
        matcher.extend(&mut items.into_iter());
        matcher.query("rs");
        let frame = settle(&matcher, 100_000);
        assert_eq!(frame.matched, 10_000);
        assert_eq!(frame.items.len(), MAX_FRAME);
    }

    #[test]
    fn extending_a_query_keeps_the_matches_it_should() {
        let matcher = NucleoMatcher::new();
        matcher.extend(&mut candidates(&["src/main.rs", "src/mode.rs", "x/other"]).into_iter());
        matcher.query("m");
        let first = settle(&matcher, 10);
        assert_eq!(first.matched, 2, "only the two paths containing an `m`");
        // `main` is a prefix-extension of `m`, which takes the append fast path.
        matcher.query("main");
        let second = settle(&matcher, 10);
        assert_eq!(second.matched, 1);
        assert_eq!(second.items[0].text, "src/main.rs");
        // And shrinking the query rescores from scratch rather than staying narrowed — the
        // append promise is only made when the old query really is a prefix of the new one.
        matcher.query("m");
        assert_eq!(settle(&matcher, 10).matched, 2);
        matcher.query("");
        assert_eq!(settle(&matcher, 10).matched, 3);
    }

    #[test]
    fn clearing_drops_every_candidate_and_leaves_the_matcher_usable() {
        let matcher = NucleoMatcher::new();
        matcher.extend(&mut candidates(&["a.rs"]).into_iter());
        matcher.query("a");
        assert_eq!(settle(&matcher, 10).total, 1);

        matcher.clear();
        assert_eq!(settle(&matcher, 10).total, 0);

        matcher.extend(&mut candidates(&["b.rs"]).into_iter());
        let frame = settle(&matcher, 10);
        assert_eq!(frame.total, 1, "the injector must survive a clear");
    }

    #[test]
    fn ranking_a_fixed_list_needs_no_worker() {
        let items = candidates(&["Split Pane Right", "Split Pane Down", "Close Tab"]);
        let frame = rank("spd", &items, 10);
        assert_eq!(frame.items[0].text, "Split Pane Down");
        assert_eq!(frame.matched, 1);
        assert_eq!(frame.total, 3);
        assert!(!frame.running);

        let empty = rank("", &items, 10);
        assert_eq!(empty.matched, 3, "an empty query matches everything");
        assert_eq!(empty.items.len(), 3);
    }

    /// Two threads doing what two Tauri workers do when the user types quickly.
    ///
    /// `picker_query` calls `query` then `frame`, and Tauri runs commands on a pool, so two
    /// keystrokes overlap as a matter of course. Before the lock order was fixed this wedged
    /// both threads permanently within a few hundred iterations: `frame` held `inner` and
    /// waited for `query`, `query` held `query` and waited for `inner`.
    ///
    /// The threads are deliberately not joined — a regression deadlocks them for ever, and a
    /// `join` would hang the whole test binary instead of failing this one test.
    #[test]
    fn a_query_and_a_frame_on_two_threads_do_not_wedge_each_other() {
        use std::sync::mpsc;

        let matcher = std::sync::Arc::new(NucleoMatcher::new());
        for i in 0..2_000 {
            matcher.push(Candidate::new(
                format!("src/file{i}.rs"),
                format!("/root/{i}"),
            ));
        }

        let (tx, rx) = mpsc::channel();
        for worker in 0..2 {
            let matcher = std::sync::Arc::clone(&matcher);
            let tx = tx.clone();
            std::thread::spawn(move || {
                for i in 0..300 {
                    if worker == 0 {
                        matcher.query(&format!("file{i}"));
                    } else {
                        let _ = matcher.frame(20);
                    }
                }
                let _ = tx.send(worker);
            });
        }
        drop(tx);

        let mut finished = 0;
        while finished < 2 {
            match rx.recv_timeout(Duration::from_secs(30)) {
                Ok(_) => finished += 1,
                Err(_) => panic!("deadlock: {finished} of 2 workers finished"),
            }
        }
    }

    /// The milestone's headline property: the picker answers before indexing finishes.
    #[test]
    fn a_hundred_thousand_paths_are_matchable_before_injection_finishes() {
        let matcher = std::sync::Arc::new(NucleoMatcher::new());
        let injector = std::sync::Arc::clone(&matcher);
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let finished = std::sync::Arc::clone(&done);

        let walk = std::thread::spawn(move || {
            for i in 0..100_000u32 {
                injector.push(Candidate::new(
                    format!("crates/cide-fs/src/module{i}/needle{i}.rs"),
                    format!("/root/{i}"),
                ));
                // Stand in for a walk's syscalls: without this the injection finishes so
                // fast that the test proves nothing about ordering.
                if i % 1_000 == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            finished.store(true, Ordering::Release);
        });

        matcher.query("needle");
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut early = None;
        while Instant::now() < deadline {
            let frame = matcher.frame(20);
            if frame.matched > 0 && !done.load(Ordering::Acquire) {
                early = Some(frame);
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        let frame = early.expect("the picker must match before the walk finishes");
        assert!(frame.items.len() <= 20);
        assert!(
            frame.total < 100_000,
            "matched against the finished set, not a partial one"
        );
        assert!(frame.items[0].text.contains("needle"));

        walk.join().unwrap();
        let complete = settle(&matcher, 20);
        assert_eq!(complete.total, 100_000);
        assert_eq!(complete.matched, 100_000);
    }

    // --- M16: two matchers, one frame ---------------------------------------------------

    /// Settle a matcher and hand back the scored frame the merge is built from.
    fn scored(matcher: &NucleoMatcher, limit: usize) -> Scored {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let scored = matcher.frame_scored(limit);
            if !scored.frame.running || Instant::now() > deadline {
                return scored;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn library(texts: &[&str], source: &str) -> NucleoMatcher {
        let matcher = NucleoMatcher::new();
        matcher.extend(
            &mut texts
                .iter()
                .map(|t| Candidate::from_source(*t, format!("/reg/{t}"), source)),
        );
        matcher
    }

    /// The score `Pattern::indices` returns and `frame` used to throw away.
    ///
    /// Asserted as a *property* rather than against a number: nucleo's bonus table is its own
    /// business and pinning a literal here would fail on the next release of a crate this
    /// project cannot upgrade anyway. What has to be true is that the vector is parallel to the
    /// rows and ordered the way the rows are, because [`merge`] is a two-finger merge and reads
    /// it as already-sorted.
    #[test]
    fn a_scored_frame_carries_one_ordered_score_per_row() {
        let matcher = NucleoMatcher::new();
        matcher
            .extend(&mut candidates(&["src/main.rs", "src/lib.rs", "docs/readme.md"]).into_iter());
        matcher.query("srmn");
        let scored = scored(&matcher, 10);
        assert_eq!(scored.scores.len(), scored.frame.items.len());
        assert!(!scored.scores.is_empty());
        assert!(
            scored.scores.windows(2).all(|w| w[0] >= w[1]),
            "nucleo hands back its matches best-first, and `merge` reads them that way: {:?}",
            scored.scores
        );
    }

    #[test]
    fn a_library_candidate_carries_its_package_and_a_project_one_does_not() {
        let project = NucleoMatcher::new();
        project.extend(&mut candidates(&["src/lib.rs"]).into_iter());
        project.query("lib");
        let libs = library(&["serde-1.0.229/src/lib.rs"], "serde 1.0.229");
        libs.query("lib");

        let frame = merge(scored(&project, 10), scored(&libs, 10), 10);
        let sources: Vec<Option<String>> = frame.items.iter().map(|r| r.source.clone()).collect();
        assert_eq!(
            sources,
            vec![None, Some("serde 1.0.229".to_string())],
            "one nullable field carries both the flag and the label, so a row can never be \
             marked as a library with nothing to show for it"
        );
    }

    /// The tie-break, which decides the *common* case rather than an edge one.
    ///
    /// An empty query scores every candidate identically, and `lib.rs` typed in full matches
    /// this project's and serde's identically too. A picker whose first row is somebody else's
    /// crate has stopped being about the project the user has open.
    #[test]
    fn the_project_wins_a_tie() {
        let project = NucleoMatcher::new();
        project.extend(&mut candidates(&["src/lib.rs"]).into_iter());
        let libs = library(&["serde-1.0.229/src/lib.rs"], "serde 1.0.229");

        for query in ["", "lib.rs"] {
            project.query(query);
            libs.query(query);
            let frame = merge(scored(&project, 10), scored(&libs, 10), 10);
            assert_eq!(
                frame.items.first().map(|r| r.source.clone()),
                Some(None),
                "the user's own file comes first for query {query:?}"
            );
        }
    }

    /// Ordering across the seam, when the scores really do differ.
    #[test]
    fn a_better_library_match_outranks_a_worse_project_one() {
        let project = NucleoMatcher::new();
        project.extend(&mut candidates(&["a/b/c/deserialize_something_else.rs"]).into_iter());
        let libs = library(&["serde-1.0.229/src/de.rs"], "serde 1.0.229");
        project.query("de.rs");
        libs.query("de.rs");

        let frame = merge(scored(&project, 10), scored(&libs, 10), 10);
        assert_eq!(
            frame.items[0].text, "serde-1.0.229/src/de.rs",
            "the merge is by score and not by side; a side-first order would bury an exact \
             match under every fuzzy one the project happens to have"
        );
    }

    #[test]
    fn the_counts_are_sums_and_running_is_the_or() {
        let project = NucleoMatcher::new();
        project.extend(&mut candidates(&["a.rs", "b.rs"]).into_iter());
        let libs = library(&["serde-1.0.229/src/lib.rs"], "serde 1.0.229");
        project.query("rs");
        libs.query("rs");

        let mut left = scored(&project, 10);
        let right = scored(&libs, 10);
        assert!(!left.frame.running);
        assert!(!right.frame.running);
        let settled = merge(left.clone(), right.clone(), 10);
        assert_eq!(
            settled.total, 3,
            "the user is searching both sets, so both are counted"
        );
        assert_eq!(settled.matched, 3);
        assert!(!settled.running);

        // One side still filling keeps the overlay polling. Believing a `false` here is how the
        // picker once left a user an empty list over a repository that was mid-walk.
        left.frame.running = true;
        assert!(merge(left, right, 10).running);
    }

    /// The limit is the *merged* limit, not the limit per side.
    #[test]
    fn the_merge_truncates_to_one_limit_rather_than_two() {
        let project = NucleoMatcher::new();
        project.extend(&mut candidates(&["a1.rs", "a2.rs", "a3.rs"]).into_iter());
        let libs = library(
            &["serde-1.0.229/src/a4.rs", "serde-1.0.229/src/a5.rs"],
            "serde 1.0.229",
        );
        project.query("a");
        libs.query("a");

        let frame = merge(scored(&project, 50), scored(&libs, 50), 4);
        assert_eq!(frame.items.len(), 4);
        assert_eq!(
            frame.matched, 5,
            "and the counter still names everything that matched, not what fitted"
        );
    }

    /// An empty other side is the ordinary case — libraries off, or a project with no
    /// dependencies — and must not cost a row.
    #[test]
    fn merging_with_an_empty_side_changes_nothing_about_the_rows() {
        let project = NucleoMatcher::new();
        project.extend(&mut candidates(&["src/main.rs", "src/lib.rs"]).into_iter());
        project.query("rs");
        let empty = NucleoMatcher::new();
        empty.query("rs");

        let alone = scored(&project, 10);
        let merged = merge(scored(&project, 10), scored(&empty, 10), 10);
        assert_eq!(
            merged.items, alone.frame.items,
            "same rows, same order, whichever way round the feature is switched"
        );
        assert_eq!(merged.total, alone.frame.total);
    }
}
