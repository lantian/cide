//! Where the user was in a file, so reopening it does not start at line 1. (M12)
//!
//! # Why this is not part of `Workspace`
//!
//! Two independent reasons, and either on its own would be enough.
//!
//! **Lifetime.** `workspace.json` records what is *open*; closing a tab removes the file from
//! it. The record a "put me back where I was" feature most needs is precisely the one that has
//! just been taken out of there — the user closed the file and is reopening it. That is the
//! same argument [`crate::RecentProject`] is built on, written out at length in
//! `cide_core::persist::recent_path`, and it lands the same way: a separate file.
//!
//! **Cost.** Every accepted mutation of the workspace bumps `rev`, clones the tree twice and
//! broadcasts it to *every* window (`cide_app::emit::workspace_changed`), where each webview
//! replaces its mirror. A scroll gesture is not a workspace mutation and must never become one:
//! at 60 Hz that is not a tuning problem, it is a different application. So this store has no
//! `rev`, emits nothing, and is written on its own slower debounce.
//!
//! # Why lines and not pixels
//!
//! `EditorView.lineWrapping` is on for every file under the highlight limit, and the code font
//! size is a runtime setting. A pixel offset is therefore wrong after a window resize, a
//! sidebar drag or a font change — not merely imprecise, but pointing at a different part of
//! the file. The complaint is "put me back on the same lines", and lines are what survive.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which of the three layouts a markdown file was last read in. (M20)
///
/// # Why this is a property of the *file* and not of the pane
///
/// The same argument the whole of this module is built on, and it lands harder here than it
/// does for a scroll position. A `README.md` is a document somebody reads and a `NOTES.md` is
/// one they write, and the layout each wants is a fact about the file rather than about the
/// tile it happens to be showing in. A per-pane mode would also be lost by the one gesture a
/// reader makes most — closing the tab and opening it again from `Ctrl+P`.
///
/// The cost is stated rather than hidden: a split showing one `.md` in two panes restores the
/// same layout into both, and switching in one does not switch the other until the next mount.
/// That is exactly the limitation [`ViewPosition::folds`] has, for exactly the same reason.
///
/// Every file has one of these, including `main.rs`, where it is [`MarkdownView::Text`] and is
/// never read. A second store keyed on "only the markdown ones" would be a second answer to
/// "where was I in this file", which is the thing this module exists to have one of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MarkdownView {
    /// The buffer, and nothing else. What every file has always shown, so it is the default.
    #[default]
    Text,
    /// Buffer on the left, rendering on the right, scroll-synchronised.
    Split,
    /// The rendering only. The buffer stays mounted and merely invisible — see
    /// `ui/src/editor/markdown/MarkdownFrame.tsx` for why it is never unmounted.
    Preview,
}

/// One file's remembered place.
///
/// # The units, once, for both features that use them
///
/// `line` and `column` are **1-based**, and `column` is a **UTF-16 code-unit** offset — the
/// convention already fixed by `RevealTarget`, `CaretPosition` and [`crate::SymbolSpan`], and
/// the one CodeMirror document positions are natively in. There is deliberately not a second
/// spelling of "a position in a file" anywhere in this app: `ui/src/editor/position.ts` is the
/// frontend half of this type and the navigation history uses the same one, because two subtly
/// different notions of one thing is how this repository once ended up with two counts for one
/// number in the git panel.
///
/// # Why `top_line` as well as the caret
///
/// Restoring only the caret puts the caret's line *somewhere* on screen — CodeMirror's minimal
/// scroll brings it just inside the nearest edge — which is not the same view. Restoring only
/// the viewport means the first arrow key teleports the user back to line 1, because the
/// selection was never moved. Both, or the fix is half a fix.
///
/// A selection anchor and head are deliberately **not** stored. Nobody asked for it, and a
/// buffer that reopens with text highlighted looks like the user selected something they did
/// not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ViewPosition {
    /// Absolute path. The identity of the record; the store is keyed on it.
    #[ts(type = "string")]
    pub path: PathBuf,
    /// First visible line, 1-based. What "the same lines" means.
    pub top_line: u32,
    /// Caret line, 1-based.
    pub line: u32,
    /// Caret column, 1-based, UTF-16 code units.
    pub column: u32,
    /// The 1-based start lines of the collapsed blocks, ascending. (M19)
    ///
    /// Part of "the same lines" for the same reason `top_line` is: a file left with its imports
    /// and three long functions collapsed is a different document to read than the same file
    /// with everything open, and restoring the scroll without the folds lands the user on a line
    /// number that now means something else.
    ///
    /// **Start lines, not offsets**, for the reason this whole type stores lines: the record is
    /// written against one version of a file and applied to another. A line that no longer names
    /// a foldable range is dropped in silence by `ui/src/editor/folding.ts::foldEffectsFor`,
    /// where a stale offset would collapse a range of text nobody chose.
    ///
    /// `#[serde(default)]` so a `positions.json` written before M19 loads unchanged. There is no
    /// migration and none is owed: this file is a cache of where somebody was looking, the whole
    /// of it is re-derived by opening a file, and it has no schema version to move — which is
    /// exactly the distinction `Workspace::CURRENT_SCHEMA` exists to make about the file that
    /// does.
    #[serde(default)]
    pub folds: Vec<u32>,
    /// Which markdown layout this file was last read in. (M20)
    ///
    /// `#[serde(default)]` for the reason [`ViewPosition::folds`] gives at length: this file is
    /// a cache of where somebody was looking, it has no schema version, and a `positions.json`
    /// written before M20 must load unchanged rather than be migrated.
    ///
    /// Unlike `folds` there is nothing to normalise. A `Vec<u32>` from a webview can be
    /// unsorted, duplicated or unbounded, which is why `cide_core::persist::normalise_folds`
    /// exists; an enum that failed to deserialise is not a value this field can hold.
    #[serde(default)]
    pub markdown_view: MarkdownView,
    /// Milliseconds since the Unix epoch, at the last note. The LRU key.
    ///
    /// Written by Rust rather than taken from the caller: a webview clock is the user's system
    /// clock seen through a JS realm, and the eviction order is not something a renderer should
    /// be able to get wrong.
    pub touched_at: u64,
}
