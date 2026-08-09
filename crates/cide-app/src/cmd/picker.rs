//! The fuzzy pickers: Ctrl+P over a project's files, and one-shot ranking for the command
//! palette.
//!
//! # Why the file picker is a poll and not a subscription
//!
//! The matcher is still being filled while the user types. Pushing a frame per keystroke
//! *and* per injected batch would put an unbounded number of events on the IPC channel during
//! a walk; instead the overlay asks, and the frame it gets back says whether to ask again
//! ([`cide_ipc::PickerFrame::running`]). One round trip per keystroke plus a poll while
//! indexing is a fixed, small cost, and it keeps the ordering problem — a frame for an old
//! query arriving after a newer one — solvable by echoing the query back.

use cide_fs::FsError;
use cide_ipc::{PickerFrame, PickerItem, ProjectId};
use cide_search::{Candidate, Matcher};
use tauri::State;

use crate::files::FsRegistry;

/// Rows one query may return. The overlay draws about twelve.
const DEFAULT_LIMIT: u32 = 50;

/// Query a project's file picker.
///
/// Answerable from the instant `fs.index` starts: `total` climbing between two calls with
/// the same query is the walk still running, which is what the `6 of 2,418` counter shows.
#[tauri::command(rename_all = "camelCase")]
pub fn picker_query(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    query: String,
    limit: Option<u32>,
) -> Result<PickerFrame, FsError> {
    let fs = registry.get(project).ok_or(FsError::NoIndex)?;
    let matcher = fs.matcher();
    matcher.query(&query);
    Ok(matcher.frame(limit.unwrap_or(DEFAULT_LIMIT) as usize))
}

/// Rank a list the caller supplies — the command palette, and anything else with a fixed,
/// small candidate set.
///
/// Same scoring as the file picker on purpose. Two overlays that rank the same query
/// differently reads as a bug even when both answers are defensible on their own.
#[tauri::command(rename_all = "camelCase")]
pub fn picker_rank(query: String, items: Vec<PickerItem>, limit: Option<u32>) -> PickerFrame {
    let candidates: Vec<Candidate> = items
        .into_iter()
        .map(|i| Candidate::new(i.text, i.value))
        .collect();
    cide_search::rank(&query, &candidates, limit.unwrap_or(DEFAULT_LIMIT) as usize)
}
