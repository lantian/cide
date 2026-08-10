//! Content search across a project's roots: literal and regex, streamed. (M11)
//!
//! # This is not the picker
//!
//! [`crate::NucleoMatcher`] is a fuzzy matcher over *paths*, and its ranking is exactly wrong
//! for a grep: it scores subsequences, so `fn spawn` would match every path that happens to
//! contain those letters in order. A content search answers a different question — which
//! *lines* contain this pattern — and it answers it in walk order with no ranking at all,
//! because a grep whose results move around as more of them arrive is unreadable.
//!
//! # One engine, two modes
//!
//! Literal mode is [`regex::escape`] and nothing else. Writing a separate substring searcher
//! would have meant implementing case-insensitivity and word boundaries twice, and the two
//! implementations disagreeing on `Straße` or on `_` being a word character is precisely the
//! kind of bug that never gets reported, only distrusted. The `regex` crate's literal
//! prefilters make an escaped pattern about as fast as a hand-rolled `memmem` anyway.
//!
//! # Streaming
//!
//! [`Search::run`] hands the sink one file's hits as soon as that file has been scanned,
//! while the walk is still running — the same contract as `cide_fs::Index::build`, and for
//! the same reason: the first hits of a search over a large repository have to be on screen
//! long before the last directory is read. `tests/content_walk.rs` asserts it structurally,
//! from inside the sink, rather than by timing.
//!
//! # What is skipped, and why it is skipped rather than reported
//!
//! Binary files, files past [`Limits::max_file_bytes`], and unreadable ones contribute
//! nothing and raise nothing. A search panel is not a linter: a `target/` that slipped
//! through, a 300 MB core dump and a `.so` are all noise, and a per-file error list would be
//! longer than the results.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;

use cide_ipc::{SearchHit, SearchMode, SearchQuery};
use ignore::{DirEntry, WalkBuilder, WalkState};
use regex::RegexBuilder;

/// What [`compile`] fails with. Re-exported for the same reason as [`Regex`].
pub use regex::Error as PatternError;
/// The compiled pattern, re-exported so that a caller holding one — `cide-app`'s handler
/// compiles it before dispatching the walk, so that a bad pattern is an answer rather than a
/// job — does not have to name `regex` in its own manifest. `cide-app` is glue by policy and
/// every dependency it does not need is one it cannot accidentally grow logic against.
pub use regex::Regex;

/// The ignore decision, supplied by the caller.
///
/// In the app this is `cide_fs::Filter::admits` — the same object the file tree and the
/// watcher consult, so a hit can never appear for a path the tree refuses to show. It is a
/// closure rather than a `&Filter` so that this crate does not depend on `cide-fs`: the walk
/// below already applies `.gitignore` through [`ignore::WalkBuilder`] with the settings
/// `cide_fs::index` spells out, and the filter is the *final* say on top of that, not a
/// reimplementation of it.
pub type Admits<'a> = &'a (dyn Fn(&Path, bool) -> bool + Sync);

/// A root to search, with the label a multi-root project shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoot {
    pub path: PathBuf,
    /// Prefixed onto [`SearchHit::rel`] when the project has more than one root, exactly as
    /// `cide_fs::index` prefixes the picker's rows.
    pub label: String,
}

impl SearchRoot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Self { path, label }
    }
}

/// The caps that keep one search bounded in memory, on the IPC channel and in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Files larger than this are not opened. 2 MiB covers every source file anyone greps
    /// and excludes the checked-in `.wasm` that is the usual reason a search takes a minute.
    pub max_file_bytes: u64,
    /// Bytes of a matching line that are carried. A minified bundle is one line of
    /// megabytes; the panel draws about 200 characters of it.
    pub max_line_bytes: usize,
    /// Hits after which the whole search stops and reports itself truncated. Nobody reads
    /// the ten-thousandth hit; they refine the query.
    pub max_hits: usize,
    /// Hits taken from any one file. A generated file that matches on all 50 000 of its
    /// lines must not fill the budget by itself and hide every other file.
    pub max_hits_per_file: usize,
    /// How much of a file's head is examined for a NUL byte before it is called binary.
    pub probe_bytes: usize,
    /// 0 lets `ignore` pick one walker thread per core.
    pub threads: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: 2 * 1024 * 1024,
            max_line_bytes: 512,
            max_hits: 5_000,
            max_hits_per_file: 200,
            probe_bytes: 8 * 1024,
            threads: 0,
        }
    }
}

/// What a finished (or cancelled, or truncated) search did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Files opened and scanned. Not files walked — a skipped binary is not scanned.
    pub scanned: u32,
    /// Hits handed to the sink.
    pub hits: u32,
    /// Files that produced at least one hit.
    pub files: u32,
    /// [`Limits::max_hits`] was reached, so the counts are floors.
    pub truncated: bool,
}

/// Turn a query into the one regex both modes run through.
///
/// # Whole word
///
/// `\b(?:…)\b` and not `\b…\b`: the pattern is untrusted in regex mode, and `\bfoo|bar\b`
/// parses as `(\bfoo)|(bar\b)` — the toggle would apply to the first alternative only, which
/// is a wrong answer rather than an error.
pub fn compile(query: &SearchQuery) -> Result<Regex, regex::Error> {
    let body = match query.mode {
        SearchMode::Literal => regex::escape(&query.pattern),
        SearchMode::Regex => query.pattern.clone(),
    };
    let body = if query.whole_word {
        format!(r"\b(?:{body})\b")
    } else {
        body
    };
    RegexBuilder::new(&body)
        .case_insensitive(!query.case_sensitive)
        // A line at a time is the haystack, so neither of these can reach across one. Spelled
        // out because they are the defaults and a reader should not have to know that: with
        // `multi_line` on, `^` would anchor somewhere the caller never sees.
        .multi_line(false)
        .dot_matches_new_line(false)
        // The default 10 MB compiled-program limit, stated. A user-typed regex is untrusted
        // input, and `(a|b|c){50}` is a small string that compiles into a very large program.
        .size_limit(10 * (1 << 20))
        .build()
}

/// Whether a query is worth running at all.
///
/// An empty pattern compiles into a regex that matches at every position of every line, so
/// "search for nothing" would otherwise mean "return the entire repository" — the most
/// expensive possible answer to the least interesting question. The panel's empty state
/// covers it; this is the backstop.
pub fn is_searchable(query: &SearchQuery) -> bool {
    !query.pattern.is_empty()
}

/// A binary file, by the usual test: a NUL byte anywhere in the head.
///
/// Cheap and wrong at the edges — a UTF-16 source file is called binary, a `.png` whose
/// first 8 KiB happen to be NUL-free is not — and both errors are the harmless direction.
/// The alternative, sniffing content types, is a dependency and a table of magic numbers to
/// answer a question this settles for every file anyone actually greps.
pub fn is_binary(head: &[u8]) -> bool {
    head.contains(&0)
}

/// One search, ready to run.
///
/// Borrowed rather than owned so the caller keeps the cancel flag it will set from another
/// thread, and so the regex is compiled once by whoever handles the parse error.
pub struct Search<'a> {
    pub roots: &'a [SearchRoot],
    pub regex: &'a Regex,
    pub limits: Limits,
    /// The shared ignore decision. `None` leaves `ignore`'s own gitignore handling as the
    /// only filter, which is what the crate's own tests use.
    pub admits: Option<Admits<'a>>,
    /// Set from any thread to stop the walk. Checked once per entry, so a cancelled search
    /// stops within one file rather than at the end of the tree.
    pub cancel: &'a AtomicBool,
    /// Incremented once per file actually opened and scanned.
    ///
    /// Supplied by the caller rather than returned in [`Outcome`] because the number is only
    /// interesting *while* the search runs: it is the progress figure the panel shows over a
    /// repository whose size is not known in advance, and by the time `run` returns nobody is
    /// waiting for it. `None` for callers that do not draw one.
    pub progress: Option<&'a AtomicU32>,
}

impl Search<'_> {
    /// Walk every root, handing each file's hits to `sink` as they are found.
    ///
    /// **Blocking**, for as long as the tree takes. `sink` is called on this thread — the
    /// walker threads send batches over a channel rather than calling it — so it can be an
    /// `FnMut` over the caller's buffer with no lock of its own.
    pub fn run(&self, sink: &mut dyn FnMut(&[SearchHit])) -> Outcome {
        let mut outcome = Outcome::default();
        if self.roots.is_empty() {
            return outcome;
        }
        // Counted in the visitors, because only they see a file that produced no hits. Local
        // when the caller wants no live figure, so the visitors have one code path — and one
        // counter across all the roots, so a two-root project's progress does not restart.
        let local = AtomicU32::new(0);
        let scanned = self.progress.unwrap_or(&local);

        let multi = self.roots.len() > 1;
        for root in self.roots {
            if self.cancel.load(Ordering::Acquire) || outcome.truncated {
                break;
            }
            self.walk_root(root, multi, scanned, &mut outcome, sink);
        }
        outcome.scanned = scanned.load(Ordering::Acquire);
        outcome
    }

    fn walk_root(
        &self,
        root: &SearchRoot,
        multi: bool,
        scanned: &AtomicU32,
        outcome: &mut Outcome,
        sink: &mut dyn FnMut(&[SearchHit]),
    ) {
        let mut builder = WalkBuilder::new(&root.path);
        builder
            // Every one of these mirrors `cide_fs::index::walk_root`. They are the defaults
            // apart from `parents` and `require_git`, and they are spelled out here for the
            // same reason they are spelled out there: the two walks have to be readable side
            // by side, because a search that descends where the file tree does not is a
            // result the user cannot open from the tree.
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .ignore(true)
            .parents(false)
            .require_git(false)
            .follow_links(false)
            .threads(self.limits.threads);

        let (tx, rx) = mpsc::channel::<Vec<SearchHit>>();

        std::thread::scope(|scope| {
            let walker = builder.build_parallel();
            let mut factory = Visitors {
                tx,
                search: self,
                root,
                multi,
                scanned,
            };
            scope.spawn(move || walker.visit(&mut factory));

            // The receiver is where the caps are enforced exactly. The visitors also stop
            // themselves once the total is reached, but they do it from a racing count on N
            // threads and can overshoot by a file each; trimming here is what makes
            // `max_hits` a number rather than an approximation.
            for batch in rx {
                let room = self.limits.max_hits - outcome.hits as usize;
                let batch = if batch.len() >= room {
                    outcome.truncated = true;
                    // Stops the walkers. Without it the loop would sit here while a walk of
                    // the whole repository produced results that are already over the cap.
                    self.cancel.store(true, Ordering::Release);
                    &batch[..room]
                } else {
                    &batch[..]
                };
                if !batch.is_empty() {
                    outcome.hits += batch.len() as u32;
                    outcome.files += 1;
                    sink(batch);
                }
                // Cancellation ends delivery, not just the walk — including when the sink
                // itself cancelled, which is how the panel stops a search the moment the
                // user types the next character. Hits already queued behind this one are
                // dropped: they answer a query nobody is looking at any more.
                if self.cancel.load(Ordering::Acquire) {
                    break;
                }
            }
        });
    }
}

/// `ParallelVisitorBuilder`: `ignore` asks it for one visitor per walker thread.
struct Visitors<'a, 'b> {
    tx: mpsc::Sender<Vec<SearchHit>>,
    search: &'a Search<'b>,
    root: &'a SearchRoot,
    multi: bool,
    scanned: &'a AtomicU32,
}

impl<'a, 'b, 's> ignore::ParallelVisitorBuilder<'s> for Visitors<'a, 'b>
where
    'a: 's,
    'b: 's,
{
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(Visitor {
            tx: self.tx.clone(),
            search: self.search,
            root: self.root,
            multi: self.multi,
            scanned: self.scanned,
            buf: Vec::new(),
        })
    }
}

struct Visitor<'a, 'b> {
    tx: mpsc::Sender<Vec<SearchHit>>,
    search: &'a Search<'b>,
    root: &'a SearchRoot,
    multi: bool,
    scanned: &'a AtomicU32,
    /// Reused across files. One file's hits are sent as one batch, which is what makes them
    /// contiguous in the result list and lets the panel group by file without sorting.
    buf: Vec<SearchHit>,
}

impl Visitor<'_, '_> {
    /// Scan one file, or decide not to. `None` when nothing was opened.
    fn scan(&mut self, entry: &DirEntry) -> Option<()> {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            return None;
        }
        // Non-UTF-8 paths are dropped rather than lossily converted, as they are in the file
        // index: the path is the identifier the frontend hands back to open the hit, and
        // `to_string_lossy` produces one that names no file.
        let path_str = path.to_str()?;
        let rel = path.strip_prefix(&self.root.path).ok()?.to_str()?;

        if let Some(admits) = self.search.admits
            && !admits(path, false)
        {
            return None;
        }

        // From the walk's own `stat`, so this costs no syscall of its own.
        let size = entry.metadata().ok()?.len();
        if size > self.search.limits.max_file_bytes {
            return None;
        }

        let bytes = std::fs::read(path).ok()?;
        let probe = bytes.len().min(self.search.limits.probe_bytes);
        if is_binary(&bytes[..probe]) {
            return None;
        }
        self.scanned.fetch_add(1, Ordering::Relaxed);

        // Lossy rather than a `from_utf8` that drops the file: a latin-1 `README` is still a
        // text file somebody wants to grep, and the replacement characters are in the text
        // that ships, so the byte offsets stay consistent with the string the panel draws.
        // `regex::bytes` over the raw file would keep the original bytes and then have no way
        // to put a line of them in a JS string, which is where they have to end up.
        let text = String::from_utf8_lossy(&bytes);
        let rel = if self.multi {
            format!("{}/{rel}", self.root.label)
        } else {
            rel.to_string()
        };

        self.buf.clear();
        hits_in_text(
            &text,
            self.search.regex,
            &self.search.limits,
            &mut |line, hit| {
                self.buf.push(SearchHit {
                    path: path_str.to_string(),
                    rel: rel.clone(),
                    line,
                    text: hit.text.to_string(),
                    start: hit.start,
                    end: hit.end,
                });
            },
        );
        if !self.buf.is_empty() {
            // A closed receiver means the search was abandoned; the walker winds down on the
            // cancel flag rather than on this.
            let _ = self.tx.send(std::mem::take(&mut self.buf));
        }
        Some(())
    }
}

impl ignore::ParallelVisitor for Visitor<'_, '_> {
    fn visit(&mut self, entry: Result<DirEntry, ignore::Error>) -> WalkState {
        if self.search.cancel.load(Ordering::Acquire) {
            return WalkState::Quit;
        }
        match entry {
            Ok(entry) => {
                // A directory the shared filter rejects is not descended into. `ignore` has
                // already applied `.gitignore`, so this only bites where the filter is
                // stricter — and pruning is the difference between skipping a `target/` and
                // walking it to reject every file individually.
                if entry.file_type().is_some_and(|t| t.is_dir())
                    && let Some(admits) = self.search.admits
                    && !admits(entry.path(), true)
                {
                    return WalkState::Skip;
                }
                self.scan(&entry);
                WalkState::Continue
            }
            Err(err) => {
                // One unreadable directory costs that subtree and nothing else, as in the
                // file walk.
                tracing::debug!(%err, "skipping an unreadable entry during a content search");
                WalkState::Continue
            }
        }
    }
}

/// One match, as [`hits_in_text`] reports it: a borrowed slice of the line and the offsets
/// inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineHit<'a> {
    /// The line, clipped to [`Limits::max_line_bytes`]. What ships.
    pub text: &'a str,
    /// Byte offsets of the match within [`Self::text`].
    pub start: u32,
    pub end: u32,
}

/// Every match in `text`, line by line, with 1-based line numbers.
///
/// Split out and public because it is the whole matching semantics — line numbering, the
/// clip, the per-file cap, `\r\n`, empty matches — with no walk and no IO around it, which is
/// what makes those testable one at a time rather than through a directory tree.
pub fn hits_in_text(
    text: &str,
    regex: &Regex,
    limits: &Limits,
    out: &mut dyn FnMut(u32, LineHit<'_>),
) -> usize {
    let mut found = 0usize;
    for (i, raw) in text.split('\n').enumerate() {
        if found >= limits.max_hits_per_file {
            break;
        }
        // `split('\n')` leaves the `\r` of a CRLF file on the end of every line. Left on, it
        // would be drawn as a stray glyph and would break a `foo$` regex on exactly the files
        // where the user least expects it.
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let line = clip(line, limits.max_line_bytes);
        for m in regex.find_iter(line) {
            if m.start() == m.end() {
                // A pattern that can match nothing — `a*`, or a bare `\b` — otherwise reports
                // a hit on every line in the repository, each highlighting no characters.
                continue;
            }
            out(
                (i + 1) as u32,
                LineHit {
                    text: line,
                    start: m.start() as u32,
                    end: m.end() as u32,
                },
            );
            found += 1;
            if found >= limits.max_hits_per_file {
                break;
            }
        }
    }
    found
}

/// The first `max` bytes of `line`, backed off to a char boundary.
///
/// Matches are searched in the *clipped* line rather than in the whole one, so an offset can
/// never point past the text that ships. The cost is that a match beyond the clip is not
/// reported at all — which is the honest outcome: a hit at column 40 000 of a minified bundle
/// cannot be shown, and reporting it with a range the panel cannot use would be worse.
fn clip(line: &str, max: usize) -> &str {
    if line.len() <= max {
        return line;
    }
    let mut end = max;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(pattern: &str, mode: SearchMode) -> SearchQuery {
        SearchQuery {
            pattern: pattern.to_string(),
            mode,
            case_sensitive: false,
            whole_word: false,
        }
    }

    /// Every hit in a string, as `(line, start, end, text)`.
    fn scan(text: &str, q: &SearchQuery, limits: &Limits) -> Vec<(u32, u32, u32, String)> {
        let re = compile(q).expect("the test's own pattern must compile");
        let mut out = Vec::new();
        hits_in_text(text, &re, limits, &mut |line, hit| {
            out.push((line, hit.start, hit.end, hit.text.to_string()))
        });
        out
    }

    const SAMPLE: &str = "fn spawn(x: u32) {\nlet spawned = SPAWN;\n// respawn\n}\n";

    #[test]
    fn a_literal_is_text_and_a_regex_is_a_pattern() {
        let limits = Limits::default();
        // `.` is a character in literal mode.
        let text = "a.b\naxb\n";
        assert_eq!(
            scan(text, &query("a.b", SearchMode::Literal), &limits)
                .into_iter()
                .map(|h| h.0)
                .collect::<Vec<_>>(),
            vec![1],
            "a literal dot matches only a dot"
        );
        assert_eq!(
            scan(text, &query("a.b", SearchMode::Regex), &limits)
                .into_iter()
                .map(|h| h.0)
                .collect::<Vec<_>>(),
            vec![1, 2],
            "a regex dot matches any character"
        );
    }

    #[test]
    fn line_numbers_are_one_based_and_the_range_is_within_the_line() {
        let hits = scan(
            SAMPLE,
            &query("spawn", SearchMode::Literal),
            &Limits::default(),
        );
        assert_eq!(
            hits.iter().map(|h| h.0).collect::<Vec<_>>(),
            vec![1, 2, 2, 3],
            "SPAWN on line 2 matches case-insensitively too"
        );
        let (line, start, end, text) = hits[0].clone();
        assert_eq!(line, 1);
        assert_eq!(&text[start as usize..end as usize], "spawn");
        assert_eq!(text, "fn spawn(x: u32) {");
    }

    #[test]
    fn case_sensitivity_is_a_toggle_in_both_modes() {
        let limits = Limits::default();
        for mode in [SearchMode::Literal, SearchMode::Regex] {
            let mut q = query("spawn", mode);
            assert_eq!(scan(SAMPLE, &q, &limits).len(), 4, "{mode:?}: insensitive");
            q.case_sensitive = true;
            let sensitive = scan(SAMPLE, &q, &limits);
            assert_eq!(
                sensitive.iter().map(|h| h.0).collect::<Vec<_>>(),
                vec![1, 2, 3],
                "{mode:?}: SPAWN is out once case matters"
            );
        }
    }

    #[test]
    fn whole_word_bounds_the_match_on_both_sides() {
        let limits = Limits::default();
        let text = "spawn\nrespawn\nspawned\nspawn_later\n(spawn)\n";
        let mut q = query("spawn", SearchMode::Literal);
        q.case_sensitive = true;
        assert_eq!(scan(text, &q, &limits).len(), 5, "every line contains it");

        q.whole_word = true;
        assert_eq!(
            scan(text, &q, &limits)
                .into_iter()
                .map(|h| h.0)
                .collect::<Vec<_>>(),
            vec![1, 5],
            "`_` is a word character, so `spawn_later` is one word and not a boundary"
        );
    }

    /// The reason the whole-word wrapper is `\b(?:…)\b`.
    #[test]
    fn whole_word_binds_to_the_whole_regex_and_not_to_its_first_branch() {
        let mut q = query("spawn|kill", SearchMode::Regex);
        q.whole_word = true;
        let hits = scan("killed\nkill\n", &q, &Limits::default());
        assert_eq!(
            hits.into_iter().map(|h| h.0).collect::<Vec<_>>(),
            vec![2],
            "`killed` is not the word `kill`"
        );
    }

    #[test]
    fn an_unparsable_pattern_is_an_error_and_not_a_panic() {
        assert!(compile(&query("fn (", SearchMode::Regex)).is_err());
        // The same characters are ordinary text in literal mode.
        assert!(compile(&query("fn (", SearchMode::Literal)).is_ok());
    }

    #[test]
    fn an_empty_pattern_is_refused_before_it_can_match_everything() {
        assert!(!is_searchable(&query("", SearchMode::Literal)));
        assert!(is_searchable(&query(" ", SearchMode::Literal)));
    }

    #[test]
    fn a_crlf_line_keeps_neither_terminator() {
        let hits = scan(
            "let x = 1;\r\nlet y = 2;\r\n",
            &query("let", SearchMode::Literal),
            &Limits::default(),
        );
        assert_eq!(
            hits[0].3, "let x = 1;",
            "no trailing \\r on the shipped line"
        );
        assert_eq!(hits[1].0, 2);
    }

    #[test]
    fn a_line_longer_than_the_cap_is_clipped_at_a_char_boundary() {
        let limits = Limits {
            max_line_bytes: 8,
            ..Limits::default()
        };
        // `é` is two bytes, at 7..9 — it straddles the cap, so clipping at 8 would split it
        // and `&line[..8]` would panic.
        let line = "abcdefgé9needle\n";
        let hits = scan(line, &query("abc", SearchMode::Literal), &limits);
        assert_eq!(hits[0].3, "abcdefg", "backed off to the boundary below 8");
        assert!(
            scan(line, &query("needle", SearchMode::Literal), &limits).is_empty(),
            "a match past the clip is not reported with an offset the panel cannot use"
        );
    }

    #[test]
    fn a_pattern_that_can_match_nothing_reports_nothing() {
        let hits = scan(
            "aaa\nbbb\n",
            &query("a*", SearchMode::Regex),
            &Limits::default(),
        );
        assert_eq!(
            hits.into_iter().map(|h| h.0).collect::<Vec<_>>(),
            vec![1],
            "the empty matches on every line are dropped; the real one on line 1 is not"
        );
    }

    #[test]
    fn the_per_file_cap_stops_a_generated_file_taking_the_whole_budget() {
        let limits = Limits {
            max_hits_per_file: 3,
            ..Limits::default()
        };
        let text = "hit\n".repeat(100);
        assert_eq!(
            scan(&text, &query("hit", SearchMode::Literal), &limits).len(),
            3
        );
    }

    #[test]
    fn offsets_are_bytes_and_the_line_is_sliceable_by_them() {
        // Four bytes before the match, one char. A char- or UTF-16-based offset would be 1.
        let hits = scan(
            "🙂 needle\n",
            &query("needle", SearchMode::Literal),
            &Limits::default(),
        );
        let (_, start, end, text) = hits[0].clone();
        assert_eq!(start, 5, "the emoji is four bytes plus a space");
        assert_eq!(&text[start as usize..end as usize], "needle");
    }

    #[test]
    fn a_nul_in_the_head_is_what_makes_a_file_binary() {
        assert!(is_binary(b"\x7fELF\0\0\0"));
        assert!(!is_binary(b"fn main() {}\n"));
        assert!(!is_binary(&[]));
    }
}
