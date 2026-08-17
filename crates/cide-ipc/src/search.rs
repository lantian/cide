//! Wire types for the fuzzy pickers: Ctrl+P, the command palette, and anything else that
//! needs a ranked list. (M8)

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One frame of picker results.
///
/// A frame, not "the results": the file index streams in while the user is already typing,
/// so the honest answer to a query is always "here is the best 200 of what I have seen so
/// far, and I am still reading". `matched`/`total` are the two numbers behind the mock's
/// `6 of 2,418` counter, and they keep climbing during the walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PickerFrame {
    /// At most the `limit` the caller asked for, capped in Rust at 200. A keystroke must
    /// never ship 100k rows over IPC — the overlay draws about twelve.
    pub items: Vec<PickerRow>,
    /// `matched_item_count()`: how many candidates the query matches.
    pub matched: u32,
    /// `item_count()`: how many candidates exist so far.
    pub total: u32,
    /// The matcher is still working — either the walk is still injecting, or the query is
    /// still being scored. The overlay polls again while this is true.
    pub running: bool,
    /// Echoed so a frame that arrives after the user has typed on can be discarded rather
    /// than painted under the new query.
    pub query: String,
}

/// A candidate supplied by the caller, for the one-shot ranking the command palette uses.
///
/// The file picker never sends these — its candidates come from the walk and stay in Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct PickerItem {
    pub text: String,
    pub value: String,
}

/// One ranked candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PickerRow {
    /// What was matched and what is drawn — for files, the path relative to its root.
    pub text: String,
    /// What selecting the row means: an absolute path, or a command id. Carried separately
    /// because the row the user reads is never the thing the caller needs to act on.
    pub value: String,
    /// Offsets in `text` that the query matched, for the highlight in the overlay.
    ///
    /// These count **chars**, which is what the matcher works in — not bytes, and not the
    /// UTF-16 units a JS string is indexed by. Those three agree for every path made of BMP
    /// characters and diverge for an emoji, so highlight against `Array.from(text)` rather
    /// than `text[i]`.
    pub indices: Vec<u32>,
    /// Where this row came from, when it did not come from the project. (M16)
    ///
    /// `Some("serde 1.0.229")` for a file inside an *External Libraries* package, `None` for a
    /// project file and for every row `picker_rank` produces (the command palette, which has no
    /// provenance to speak of).
    ///
    /// # One nullable field rather than a flag and a label
    ///
    /// The failure this exists for is two rows reading `RS  lib.rs  src/lib.rs`, one of them
    /// this project's and one of them `serde`'s — the request's own words were "open the wrong
    /// `lib.rs`". A boolean `library: bool` would let the frontend draw a row *marked* as a
    /// library with nothing to say about which one, and a separate `package: Option<String>`
    /// beside it would let the two disagree. One field carries both facts and cannot.
    ///
    /// It is the package's *display* string — name and version, exactly as the tree's own row
    /// draws it — and not a path. A path here would be the registry directory, which is what
    /// `value` already carries and what nobody can read at 620px.
    #[ts(optional)]
    pub source: Option<String>,
}

// --- M11: content search ----------------------------------------------------------------
//
// Deliberately separate types from the picker's, in the same file because both are "search"
// on the wire and neither is more than a screenful. A content search is not a fuzzy match:
// the picker ranks *paths* by a subsequence score, this one reports every *line* that
// matches a pattern, in walk order, with no ranking at all. Sharing `PickerFrame` between
// them would have meant a `Vec<PickerRow>` whose `text` was sometimes a path and sometimes a
// line of source, and a `matched` count that meant two different things.

/// How the pattern is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SearchMode {
    /// The pattern is text. Escaped before it reaches the engine, so `a.b` finds `a.b` and
    /// not `axb`.
    Literal,
    /// The pattern is a regular expression in the `regex` crate's syntax — no backtracking,
    /// so no lookaround and no backreferences.
    Regex,
}

/// What the user typed, and the toggles beside the input.
///
/// Echoed back on every [`SearchFrame`] whole, rather than only the pattern string: toggling
/// case-sensitivity without touching the text changes the answer, and a frontend that
/// compared pattern strings alone would paint the old results under the new toggle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SearchQuery {
    pub pattern: String,
    pub mode: SearchMode,
    pub case_sensitive: bool,
    /// Wrap the pattern in `\b…\b`. On a literal made only of punctuation this can never
    /// match, which is what `grep -w` does with the same input.
    pub whole_word: bool,
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchHit {
    /// Absolute. What opening the hit acts on.
    pub path: String,
    /// Relative to its root, prefixed with the root label in a multi-root project — the same
    /// string the file picker draws, so one file is named the same in both surfaces.
    pub rel: String,
    /// 1-based, as every editor and every compiler diagnostic counts lines.
    pub line: u32,
    /// The line's text, without its terminator, clipped to a few hundred bytes. A minified
    /// bundle has single lines of megabytes and none of them belongs on the IPC channel.
    pub text: String,
    /// Byte offset of the match **within `text`**.
    ///
    /// Bytes, not chars and not the UTF-16 units a JS string is indexed by: bytes are what
    /// the engine works in, and the three agree only for ASCII. `ui/src/sidebar/SearchModel.ts`
    /// splits the line with a `TextEncoder` rather than `String.prototype.slice`, which is
    /// what makes the highlight land in the right place on a line with an emoji in it.
    pub start: u32,
    /// Exclusive end of the match, in the same units as [`Self::start`].
    pub end: u32,
}

/// One page of a running or finished search.
///
/// A page, not a result set. The walk is still running while the user reads the first hits,
/// so the panel asks repeatedly from an advancing `offset` and appends — the same poll shape
/// as [`PickerFrame`], and for the same reason: pushing an event per batch would put an
/// unbounded number of them on the IPC channel during a walk of a large repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchFrame {
    /// The query these hits answer. A frame whose query is not the one on screen is dropped
    /// rather than painted.
    pub query: SearchQuery,
    /// Hits `[offset, offset + hits.len())` of the full list, in walk order. Every hit from
    /// one file is contiguous, which is what lets the panel group by file without sorting.
    pub hits: Vec<SearchHit>,
    /// Where `hits[0]` sits in the full list. Echoed so a frame that arrives out of order
    /// cannot be appended twice.
    pub offset: u32,
    /// Hits found so far. Keeps climbing while `running`.
    pub total: u32,
    /// Files with at least one hit so far — the second figure in the panel's readout.
    pub files: u32,
    /// Files opened and scanned so far. The only honest progress figure during a walk whose
    /// size is not known in advance.
    pub scanned: u32,
    /// The walk is still going. The panel polls again while this is true.
    pub running: bool,
    /// The hit cap stopped the search early, so the counts are floors rather than totals.
    pub truncated: bool,
    /// An unusable pattern — an unbalanced `(` in regex mode, almost always.
    ///
    /// Carried on the frame instead of rejecting the command: half of every regex is
    /// unparsable while it is being typed, and a rejected promise per keystroke would turn
    /// ordinary typing into a stream of errors in the log.
    pub error: Option<String>,
}
