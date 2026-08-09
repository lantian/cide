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
}

impl Candidate {
    pub fn new(text: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            value: value.into(),
        }
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
        let limit = limit.min(MAX_FRAME);
        let mut nucleo = self.inner.lock();
        // 10ms: long enough to finish a small query outright, short enough that the IPC
        // thread is never held for a frame's worth of time.
        let status = nucleo.tick(10);
        self.dirty.store(false, Ordering::Release);

        let query = self.query.lock().clone();
        let snapshot = nucleo.snapshot();
        let matched = snapshot.matched_item_count();
        let total = snapshot.item_count();
        let take = (matched as usize).min(limit) as u32;

        let mut matcher = self.highlight.lock();
        let pattern = snapshot.pattern().column_pattern(0);
        let mut indices = Vec::new();
        let items = snapshot
            .matched_items(..take)
            .map(|item| {
                indices.clear();
                pattern.indices(
                    item.matcher_columns[0].slice(..),
                    &mut matcher,
                    &mut indices,
                );
                indices.sort_unstable();
                indices.dedup();
                PickerRow {
                    text: item.data.text.clone(),
                    value: item.data.value.clone(),
                    indices: indices.clone(),
                }
            })
            .collect();

        PickerFrame {
            items,
            matched,
            total,
            running: status.running,
            query,
        }
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
}
