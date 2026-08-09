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
}
