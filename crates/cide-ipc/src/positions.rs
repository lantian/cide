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
    /// Milliseconds since the Unix epoch, at the last note. The LRU key.
    ///
    /// Written by Rust rather than taken from the caller: a webview clock is the user's system
    /// clock seen through a JS realm, and the eviction order is not something a renderer should
    /// be able to get wrong.
    pub touched_at: u64,
}
