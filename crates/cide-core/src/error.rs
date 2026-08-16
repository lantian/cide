//! Errors crossing the IPC boundary.
//!
//! Every variant is a tag the frontend can branch on. Nothing here is a bare `String`:
//! `TabPinned` is a thing the UI reacts to by dimming a close button, and matching on
//! prose to discover that is how error handling rots.

use cide_ipc::{PaneId, ProjectId, SplitId, TabId, UnsavedTab};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// `rename_all_fields` as well as `rename_all`: the first renames variants, the second the
// *fields* of struct variants. Without it `UnsavedChanges { tabs }` would be fine but the
// next multi-word field added here would reach the webview as snake_case, which has already
// been a real bug in this project.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    content = "detail"
)]
#[ts(export)]
pub enum CoreError {
    /// `tabs[0]` is the pinned project console. It cannot be closed or reordered, and this
    /// is enforced here rather than in the frontend so that no UI bug can lose it.
    #[error("the project console tab is pinned and cannot be closed or moved")]
    TabPinned,

    /// Closing would discard unsaved edits, and the caller did not pass `force`.
    ///
    /// Enforced here, in the domain, for exactly the reason [`Self::TabPinned`] is: the
    /// frontend's dialog is a courtesy, and a courtesy is not a guard. A dropped promise, a
    /// component that unmounts mid-await, a second window that never rendered the dialog —
    /// each of those is a lost buffer if Rust closes on request. So `close_tab` and
    /// `close_project` refuse, and the *only* way past is a caller that says `force`,
    /// which is a caller that has already shown the user this list.
    ///
    /// Carries the tabs rather than a count, so the frontend can name them without a second
    /// round trip against a workspace that has since moved on.
    #[error("{} would be discarded: {}", plural(tabs.len()), names(tabs))]
    UnsavedChanges { tabs: Vec<UnsavedTab> },

    /// The primary Claude pane cannot leave its tab — not by closing, and not by detaching
    /// into a window of its own. It is the project's conversation, and the pinned console
    /// exists to show it.
    ///
    /// The "while it is the only pane" qualifier this used to carry was dropped when
    /// `layout::take_pane` started refusing the role outright: keeping it meant the message
    /// contradicted itself out loud on the exact gesture that produces it — a user closing
    /// the primary of a two-pane console being told it cannot be closed while it is the only
    /// pane, with a second pane visible beside it.
    #[error("the project console's primary Claude pane cannot be closed or detached")]
    PanePrimary,

    /// A project must keep at least its console tab.
    #[error("a project must keep at least one tab")]
    LastTab,

    /// A tab must keep at least one pane.
    ///
    /// Distinct from [`Self::LastTab`] because these are tagged so the frontend can word
    /// the message: closing the only pane of an ordinary tab has nothing to do with tabs,
    /// and telling the user it does is worse than saying nothing.
    #[error("a tab must keep at least one pane")]
    LastPane,

    /// A project must have at least one root directory.
    #[error("a project must have at least one root")]
    NoRoots,

    #[error("no such project: {0}")]
    NoSuchProject(ProjectId),
    #[error("no such tab: {0}")]
    NoSuchTab(TabId),
    #[error("no such pane: {0}")]
    NoSuchPane(PaneId),
    #[error("no such split: {0}")]
    NoSuchSplit(SplitId),
    #[error("no such command: {0}")]
    NoSuchCommand(String),

    /// An index-based reorder was given an out-of-range position.
    #[error("index {index} is out of range (len {len})")]
    IndexOutOfRange { index: usize, len: usize },

    /// The pane tree violated an invariant. Only ever produced by `layout::validate`, and
    /// only ever a bug in this crate.
    #[error("pane tree invariant violated: {0}")]
    Invariant(String),

    /// A write carried a precondition and the file on disk no longer matched it. (M15)
    ///
    /// A **tagged** variant rather than an `Io(String)` carrying the sentence, and the tag is
    /// the whole reason it exists: the frontend has to tell "the file moved under you, here is
    /// the conflict bar" from "the disk is full, here is a failure toast", and matching on prose
    /// is how a translated or reworded message silently turns one into the other.
    ///
    /// Only ever produced when the caller *asked* for the check — an explicit Ctrl+S passes no
    /// stamp and cannot see this. See `cide_core::document::write_if_unchanged`.
    #[error("{path} changed on disk since it was opened")]
    FileChanged { path: String },

    #[error("{0}")]
    Io(String),

    #[error("{0}")]
    Serde(String),
}

/// `1 unsaved file` / `3 unsaved files`, for [`CoreError::UnsavedChanges`].
///
/// A named function rather than an inline expression in the `#[error]` attribute: the format
/// string is not a place anyone reads carefully, and "1 unsaved files" in an error a user
/// sees is the kind of thing that survives for years.
fn plural(count: usize) -> String {
    match count {
        1 => "1 unsaved file".to_string(),
        n => format!("{n} unsaved files"),
    }
}

/// The tabs' basenames, comma-separated.
///
/// Basenames rather than full paths: the message is one line and three absolute paths in a
/// monorepo do not fit on one. The structured `tabs` field carries the paths for the dialog,
/// which has room for them.
fn names(tabs: &[UnsavedTab]) -> String {
    tabs.iter()
        .map(|t| t.title.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{ProjectId, TabId};
    use std::path::PathBuf;

    /// Pin the bytes `ui/src/ipc/client.ts`'s `unsavedChanges()` reads.
    ///
    /// That function narrows `{ kind, detail }` by hand, because `generated.ts` is built
    /// from `cide-ipc` and `CoreError` lives here — there is no generated type for it to
    /// check against. So nothing else in the tree would notice this enum's serde attributes
    /// changing, and the way it fails is the bad way: `unsavedChanges()` answers `null`, the
    /// store re-throws instead of confirming, and the close guard is a `×` that reports an
    /// error nobody reads.
    ///
    /// Asserted as JSON rather than by round-tripping through `Deserialize`, which would
    /// agree with itself whatever the tags were.
    #[test]
    fn the_unsaved_refusal_keeps_the_wire_shape_the_frontend_reads() {
        let tab = TabId::new();
        let project = ProjectId::new();
        let error = CoreError::UnsavedChanges {
            tabs: vec![cide_ipc::UnsavedTab {
                tab,
                project,
                project_name: "cide".into(),
                path: PathBuf::from("/home/dev/work/cide/src/main.rs"),
                title: "main.rs".into(),
            }],
        };

        let value = serde_json::to_value(&error).expect("serialises");
        assert_eq!(
            value["kind"], "unsavedChanges",
            "the tag the frontend matches"
        );
        let row = &value["detail"]["tabs"][0];
        assert_eq!(row["tab"], tab.to_string());
        assert_eq!(row["project"], project.to_string());
        // From `UnsavedTab`'s own `rename_all`, not from this enum's `rename_all_fields` —
        // that one covers the *variant's* fields (`tabs`), which happen to be single words
        // today. Both are asserted here because the frontend cannot tell them apart: it
        // reads one object, and either attribute going missing empties the dialog's rows.
        assert_eq!(row["projectName"], "cide");
        assert!(row.get("project_name").is_none(), "no snake_case leak");
        assert!(
            value["detail"]["tabs"].is_array(),
            "the variant field keeps its name"
        );
        assert_eq!(row["path"], "/home/dev/work/cide/src/main.rs");
        assert_eq!(row["title"], "main.rs");
    }

    /// A unit variant carries no `detail`, which is what makes the `kind` check above
    /// sufficient on its own — a caller that reads `detail` unconditionally would break on
    /// every other refusal `tab_close` can answer with.
    #[test]
    fn a_unit_refusal_carries_only_its_tag() {
        let value = serde_json::to_value(CoreError::TabPinned).expect("serialises");
        assert_eq!(value["kind"], "tabPinned");
        assert!(value.get("detail").is_none());
    }
}
