//! Which rows survive. Pure: rows in, a keep-flag per row out, so every rule below has a test
//! that needs no disk and no calendar.
//!
//! # The rule, and the three holes in its obvious form
//!
//! JetBrains keeps "the last 5 working days", where a working day is a day the file changed.
//! That reading is why a file nobody has touched in a month still has its history: time passing
//! is not what ages a version, later work on that file is.
//!
//! Taken alone that leaves three ways for the store to grow without bound, all closed here:
//!
//! 1. **A deleted path never changes again**, so its days never roll off. It expires N *project*
//!    working days after the deletion instead, which is the one clock it still has. Days the
//!    project was not worked on do not count, so a fortnight's holiday does not lose a file
//!    deleted the day before leaving.
//! 2. **The size cap can be unreachable** if the baseline alone exceeds it and the newest version
//!    of every live file is exempt. The baseline stops at a fraction of the cap
//!    ([`BASELINE_SHARE`]) to leave room, and says that it stopped.
//! 3. **Rows cost memory** whether or not they hold bytes, so there is a row ceiling beside the
//!    byte cap.
//!
//! # What is never evicted, whatever the cap says
//!
//! The newest row of a live file. It is the only copy of the state that a first external edit
//! will destroy, which makes it the one row the feature exists to have. That makes the cap soft
//! by at most the size of the tree as it stands today, and the alternative is an agent's first
//! edit to a file having no "before".

use std::collections::BTreeSet;

use cide_ipc::local_history::VersionKind;

/// How far the first scan may fill the cap, smallest files first. See the module note.
///
/// Not the whole cap, or the first day of real work evicts the only history there is.
pub const BASELINE_SHARE: f64 = 0.7;

/// A store's limits. Machine-wide settings, resolved by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Working days of history to keep per path. Never zero: turning the feature off is a
    /// switch, not a retention of nothing, which would be a setting that deletes.
    pub days_to_keep: u32,
    /// Bytes of stored blobs.
    pub max_storage: u64,
    pub max_rows: usize,
}

/// One row, as far as retention can tell. Local rather than `crate::store::Row` so the tests
/// below build these by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fact {
    pub ts: u64,
    pub kind: VersionKind,
    pub blob: Option<u32>,
}

fn is_marker(kind: VersionKind) -> bool {
    matches!(kind, VersionKind::BaselineDone | VersionKind::Label)
}

/// `rows[i]` survives iff the answer's `[i]` is true.
///
/// `by_path` lists each path's row indices, oldest first. `stored[b]` is blob `b`'s size.
pub(crate) fn plan(
    rows: &[Fact],
    by_path: &[&[u32]],
    stored: &[u64],
    policy: &Policy,
    day_of: &dyn Fn(u64) -> i32,
) -> Vec<bool> {
    let days_to_keep = policy.days_to_keep.max(1) as usize;
    let mut keep = vec![true; rows.len()];
    let project_days: BTreeSet<i32> = rows
        .iter()
        .filter(|row| !is_marker(row.kind))
        .map(|row| day_of(row.ts))
        .collect();

    for indices in by_path {
        let Some(&newest) = indices.last() else {
            continue;
        };
        let head = &rows[newest as usize];

        // The pseudo-path. Only the newest scan marker means anything, and it must never go:
        // without it, a watcher event could not be told from a first sighting.
        if indices.iter().any(|&i| is_marker(rows[i as usize].kind)) {
            drop_older_markers(rows, indices, &mut keep);
            keep_recent_days(
                rows,
                indices,
                VersionKind::Label,
                days_to_keep,
                day_of,
                &mut keep,
            );
            continue;
        }
        if head.kind == VersionKind::Deleted {
            let since = project_days.range(day_of(head.ts) + 1..).count();
            if since >= days_to_keep {
                for &i in *indices {
                    keep[i as usize] = false;
                }
                continue;
            }
        }
        let mut days = 0;
        let mut last_day = None;
        for &i in indices.iter().rev() {
            let day = day_of(rows[i as usize].ts);
            if last_day != Some(day) {
                last_day = Some(day);
                days += 1;
            }
            if days > days_to_keep {
                keep[i as usize] = false;
            }
        }
    }

    enforce_cap(rows, by_path, stored, policy, &mut keep);
    keep
}

fn drop_older_markers(rows: &[Fact], indices: &[u32], keep: &mut [bool]) {
    let mut seen = false;
    for &i in indices.iter().rev() {
        if rows[i as usize].kind == VersionKind::BaselineDone {
            keep[i as usize] = !seen;
            seen = true;
        }
    }
}

/// Labels share a pseudo-path with the scan marker and age by the same days-of-use rule. They do
/// not pin the versions they name: a label is a bookmark, and a store that kept everything a
/// bookmark pointed at would have a retention rule the user cannot predict.
fn keep_recent_days(
    rows: &[Fact],
    indices: &[u32],
    kind: VersionKind,
    days_to_keep: usize,
    day_of: &dyn Fn(u64) -> i32,
    keep: &mut [bool],
) {
    let mut days = 0;
    let mut last_day = None;
    for &i in indices.iter().rev() {
        let row = &rows[i as usize];
        if row.kind != kind {
            continue;
        }
        let day = day_of(row.ts);
        if last_day != Some(day) {
            last_day = Some(day);
            days += 1;
        }
        if days > days_to_keep {
            keep[i as usize] = false;
        }
    }
}

/// Evict oldest-first until both limits hold, or nothing evictable is left.
fn enforce_cap(
    rows: &[Fact],
    by_path: &[&[u32]],
    stored: &[u64],
    policy: &Policy,
    keep: &mut [bool],
) {
    let mut references = vec![0_u32; stored.len()];
    let mut live: u64 = 0;
    let mut kept = 0_usize;
    for (row, _) in rows.iter().zip(keep.iter()).filter(|(_, keep)| **keep) {
        kept += 1;
        if let Some(blob) = row.blob {
            if references[blob as usize] == 0 {
                live += stored[blob as usize];
            }
            references[blob as usize] += 1;
        }
    }
    if live <= policy.max_storage && kept <= policy.max_rows {
        return;
    }

    // Rows are in id order, so this is oldest-first within each tier. Older versions go before
    // deleted files, because a deleted file's newest row is the only way to get it back.
    let mut old_versions = Vec::new();
    let mut deleted_heads = Vec::new();
    for indices in by_path {
        let Some((&newest, older)) = indices.split_last() else {
            continue;
        };
        if rows[newest as usize].kind == VersionKind::Deleted {
            deleted_heads.extend_from_slice(older);
            deleted_heads.push(newest);
        } else {
            old_versions.extend_from_slice(older);
        }
    }
    old_versions.sort_unstable();
    deleted_heads.sort_unstable();

    for &i in old_versions.iter().chain(&deleted_heads) {
        if live <= policy.max_storage && kept <= policy.max_rows {
            return;
        }
        let row = &rows[i as usize];
        if !keep[i as usize] || is_marker(row.kind) {
            continue;
        }
        keep[i as usize] = false;
        kept -= 1;
        if let Some(blob) = row.blob {
            references[blob as usize] -= 1;
            if references[blob as usize] == 0 {
                live -= stored[blob as usize];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use VersionKind::*;

    const DAY: u64 = 86_400_000;

    fn day_of(ts: u64) -> i32 {
        (ts / DAY) as i32
    }

    fn policy(days_to_keep: u32) -> Policy {
        Policy {
            days_to_keep,
            max_storage: u64::MAX,
            max_rows: usize::MAX,
        }
    }

    fn fact(day: u64, kind: VersionKind, blob: Option<u32>) -> Fact {
        Fact {
            ts: day * DAY,
            kind,
            blob,
        }
    }

    fn survivors(rows: &[Fact], paths: &[&[u32]], stored: &[u64], policy: &Policy) -> Vec<usize> {
        plan(rows, paths, stored, policy, &day_of)
            .iter()
            .enumerate()
            .filter(|(_, keep)| **keep)
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn a_path_keeps_its_last_n_days_of_changes_however_old_they_are() {
        let rows: Vec<_> = [1, 2, 2, 40, 41, 900]
            .iter()
            .map(|&day| fact(day, Modified, None))
            .collect();
        let all: Vec<u32> = (0..6).collect();
        assert_eq!(
            survivors(&rows, &[&all], &[], &policy(3)),
            [3, 4, 5],
            "three distinct days, counted by when the file changed, not by the calendar"
        );
    }

    #[test]
    fn several_versions_on_one_day_count_as_that_day() {
        let rows: Vec<_> = [1, 2, 2, 2]
            .iter()
            .map(|&day| fact(day, Modified, None))
            .collect();
        let all: Vec<u32> = (0..4).collect();
        assert_eq!(survivors(&rows, &[&all], &[], &policy(1)), [1, 2, 3]);
    }

    #[test]
    fn a_cold_file_keeps_its_history_while_the_project_moves_on() {
        let rows = vec![
            fact(1, Baseline, None),
            fact(100, Modified, None),
            fact(101, Modified, None),
            fact(102, Modified, None),
        ];
        assert_eq!(
            survivors(&rows, &[&[0], &[1, 2, 3]], &[], &policy(2)),
            [0, 2, 3],
            "the untouched file's only row is its last working day, not an expired one"
        );
    }

    /// The hole in the obvious rule: a deleted path never gets a newer day, so per-path counting
    /// alone would keep it forever.
    #[test]
    fn a_deleted_path_expires_after_n_project_working_days() {
        let rows = vec![
            fact(1, Baseline, None),
            fact(2, Deleted, None),
            fact(3, Modified, None),
            fact(50, Modified, None),
        ];
        assert_eq!(
            survivors(&rows, &[&[0, 1], &[2, 3]], &[], &policy(3)),
            [0, 1, 2, 3],
            "two project days since the deletion: still recoverable"
        );
        assert_eq!(
            survivors(&rows, &[&[0, 1], &[2, 3]], &[], &policy(2)),
            [2, 3],
            "days the project was not worked on do not count, the two that were do"
        );
    }

    #[test]
    fn a_deleted_file_is_not_expired_by_the_calendar_alone() {
        let rows = vec![fact(1, Deleted, None)];
        assert_eq!(
            plan(&rows, &[&[0]], &[], &policy(1), &|_| 1),
            [true],
            "nothing happened in the project since, however long ago that was"
        );
    }

    #[test]
    fn the_cap_evicts_old_versions_first_and_never_a_live_files_newest() {
        let rows = vec![
            fact(1, Modified, Some(0)),
            fact(2, Modified, Some(1)),
            fact(3, Modified, Some(2)),
            fact(4, Deleted, Some(3)),
        ];
        let limited = Policy {
            max_storage: 15,
            ..policy(100)
        };
        assert_eq!(
            survivors(&rows, &[&[0, 1, 2], &[3]], &[10, 10, 10, 10], &limited),
            [2],
            "old versions go first, then deleted files, and the live head stays"
        );
    }

    #[test]
    fn the_cap_prefers_old_versions_over_a_deleted_files_only_copy() {
        let rows = vec![
            fact(1, Modified, Some(0)),
            fact(2, Modified, Some(1)),
            fact(3, Deleted, Some(2)),
        ];
        let limited = Policy {
            max_storage: 20,
            ..policy(100)
        };
        assert_eq!(
            survivors(&rows, &[&[0, 1], &[2]], &[10, 10, 10], &limited),
            [1, 2]
        );
    }

    #[test]
    fn a_cap_the_live_files_exceed_on_their_own_is_soft() {
        let rows = vec![fact(1, Baseline, Some(0)), fact(1, Baseline, Some(1))];
        let limited = Policy {
            max_storage: 5,
            ..policy(100)
        };
        assert_eq!(survivors(&rows, &[&[0], &[1]], &[10, 10], &limited), [0, 1]);
    }

    #[test]
    fn a_blob_shared_by_two_rows_frees_nothing_until_both_go() {
        let rows = vec![
            fact(1, Modified, Some(0)),
            fact(2, Modified, Some(0)),
            fact(3, Modified, Some(1)),
        ];
        let limited = Policy {
            max_storage: 10,
            ..policy(100)
        };
        assert_eq!(survivors(&rows, &[&[0, 1, 2]], &[10, 10], &limited), [2]);
    }

    #[test]
    fn the_row_ceiling_holds_even_with_no_bytes_involved() {
        let rows: Vec<_> = (1..=5).map(|day| fact(day, Modified, None)).collect();
        let all: Vec<u32> = (0..5).collect();
        let limited = Policy {
            max_rows: 2,
            ..policy(100)
        };
        assert_eq!(survivors(&rows, &[&all], &[], &limited), [3, 4]);
    }

    #[test]
    fn the_newest_scan_marker_survives_and_labels_age_by_their_own_days() {
        let rows = vec![
            fact(1, BaselineDone, None),
            fact(2, Label, None),
            fact(3, BaselineDone, None),
            fact(4, Label, None),
            fact(5, Label, None),
        ];
        assert_eq!(
            survivors(&rows, &[&[0, 1, 2, 3, 4]], &[], &policy(2)),
            [2, 3, 4]
        );
    }

    #[test]
    fn markers_are_never_evicted_for_space() {
        let rows = vec![fact(1, BaselineDone, None), fact(2, Modified, Some(0))];
        let limited = Policy {
            max_rows: 0,
            max_storage: 0,
            ..policy(100)
        };
        assert_eq!(survivors(&rows, &[&[0], &[1]], &[10], &limited), [0, 1]);
    }

    #[test]
    fn zero_days_is_treated_as_one_rather_than_deleting_everything() {
        let rows = vec![fact(1, Modified, None), fact(2, Modified, None)];
        assert_eq!(survivors(&rows, &[&[0, 1]], &[], &policy(0)), [1]);
    }
}
