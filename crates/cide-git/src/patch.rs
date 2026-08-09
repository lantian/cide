//! Synthesizing a unified diff of only the selected changes.
//!
//! This is the one function in the project where a bug destroys uncommitted work, so it is
//! written to be boring and it is verified against the real `git`.
//!
//! # The algorithm
//!
//! For each hunk, per line:
//!
//! | original | selected | emitted | counts toward |
//! | --- | --- | --- | --- |
//! | ` ` context | — | ` ` | old and new |
//! | `+` addition | yes | `+` | new |
//! | `+` addition | no | *dropped* | — |
//! | `-` deletion | yes | `-` | old |
//! | `-` deletion | no | ` ` | old and new |
//!
//! An unselected deletion becomes context because the patch's old side is the pre-image we
//! are applying to: the line is still there, we simply are not removing it. Dropping it
//! instead would claim the pre-image never had it, and the patch would refuse to apply — or
//! worse, apply at the wrong offset.
//!
//! It follows that **the old-side count never changes**, only the new-side one; the assert in
//! [`synthesize`] states that as an invariant rather than a hope.
//!
//! # Hunk headers
//!
//! `git add -p` recomputes exactly two things: the counts, and `new_offset += delta`, where
//! `delta` accumulates `new_count - old_count` over the hunks actually emitted. This does the
//! same, with one extra piece of care that `add -p` gets for free by copying the original
//! header text: xdiff prints the *position before* an empty range (`xdl_emit_hunk_hdr` does
//! `c1 ? s1 : s1 - 1`, and omits `,count` when the count is 1). Both quirks are reproduced
//! here, so a hunk we did not modify renders byte-identically to the one libgit2 printed.
//!
//! # `\ No newline at end of file`
//!
//! The marker travels with the line it annotates ([`crate::diff::RawLine::eofnl`]), so
//! dropping a line drops its marker. What it cannot survive is being moved into the middle of
//! a patch, which happens when the user deselects the deletion of a file's last unterminated
//! line while selecting something after it. That selection describes a file that both does
//! and does not end in a newline; [`synthesize`] refuses it as
//! [`PartialRefusal::NoNewlineOrdering`] rather than emitting a patch no tool can read.

use std::collections::BTreeSet;

use cide_ipc::git::{GitError, PartialRefusal, Selection};

use crate::Result;
use crate::diff::{RawFile, RawHunk};

/// A position in a [`RawFile`]: `(hunk index, line index within the hunk)`.
pub type Position = (usize, usize);

/// The lines an operation will act on, resolved against a concrete diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub lines: BTreeSet<Position>,
    /// Every selectable line in the file is selected, so the caller should take the index
    /// API instead — exact by construction, and it works for binaries and modes too.
    pub covers_everything: bool,
}

/// Resolve a wire [`Selection`] against the diff it was made from.
///
/// Context lines named by a `Lines` selection are ignored rather than rejected: a drag over
/// a diff picks them up, and refusing the gesture would be surprising when the meaning is
/// unambiguous.
pub fn choose(file: &RawFile, selection: &Selection) -> Result<Chosen> {
    let all = every_change(file);
    let lines = match selection {
        Selection::Whole => all.clone(),
        Selection::Hunks { hunks } => {
            let mut chosen = BTreeSet::new();
            for &hunk in hunks {
                let index = hunk as usize;
                let Some(raw) = file.hunks.get(index) else {
                    return Err(GitError::StaleSelection {
                        path: file.path.clone(),
                    });
                };
                for (line, raw_line) in raw.lines.iter().enumerate() {
                    if raw_line.is_change() {
                        chosen.insert((index, line));
                    }
                }
            }
            chosen
        }
        Selection::Lines { lines } => {
            let mut chosen = BTreeSet::new();
            for reference in lines {
                let (hunk, line) = (reference.hunk as usize, reference.line as usize);
                let Some(raw) = file.hunks.get(hunk).and_then(|h| h.lines.get(line)) else {
                    return Err(GitError::StaleSelection {
                        path: file.path.clone(),
                    });
                };
                if raw.is_change() {
                    chosen.insert((hunk, line));
                }
            }
            chosen
        }
    };

    if lines.is_empty() {
        return Err(GitError::PartialRefused {
            path: file.path.clone(),
            reason: PartialRefusal::Empty,
        });
    }
    Ok(Chosen {
        covers_everything: lines == all,
        lines,
    })
}

/// The complement of a selection: every change the selection did *not* name.
///
/// This is how unstaging and rollback are expressed without ever reversing a patch. Both are
/// "rebuild this file from its pre-image plus the changes we are keeping", and the changes
/// being kept are exactly this set. Reversing a unified diff means rewriting `new file mode`
/// into `deleted file mode`, swapping the `index` line and the `---`/`+++` paths, and getting
/// every one of them right; not doing it at all is a smaller thing to get right.
pub fn complement(file: &RawFile, chosen: &Chosen) -> BTreeSet<Position> {
    every_change(file)
        .into_iter()
        .filter(|position| !chosen.lines.contains(position))
        .collect()
}

/// Every addition and deletion in the file.
pub fn every_change(file: &RawFile) -> BTreeSet<Position> {
    let mut out = BTreeSet::new();
    for (hunk, raw) in file.hunks.iter().enumerate() {
        for (line, raw_line) in raw.lines.iter().enumerate() {
            if raw_line.is_change() {
                out.insert((hunk, line));
            }
        }
    }
    out
}

/// Build a unified diff containing only `chosen`.
///
/// `Ok(None)` means the selection is empty — a legitimate outcome for
/// [`complement`], which is empty exactly when the whole file was selected.
pub fn synthesize(file: &RawFile, chosen: &BTreeSet<Position>) -> Result<Option<Vec<u8>>> {
    if let Some(reason) = file.partial_refusal() {
        return Err(GitError::PartialRefused {
            path: file.path.clone(),
            reason,
        });
    }
    if chosen.is_empty() {
        return Ok(None);
    }

    let mut body = Vec::new();
    let mut delta: i64 = 0;
    // Where the last `\ No newline` marker was written, as (byte offset just past it). A
    // marker is only legal at the very end of the patch, so remembering the last one and
    // checking it at the end is the whole validation.
    let mut marker_end: Option<usize> = None;

    for (index, hunk) in file.hunks.iter().enumerate() {
        let Some(emitted) = emit_hunk(hunk, index, chosen, delta, &mut marker_end, &mut body)?
        else {
            continue;
        };
        delta += emitted;
    }

    if body.is_empty() {
        return Ok(None);
    }
    if let Some(end) = marker_end
        && end != body.len()
    {
        // The marker landed somewhere other than the end of the patch: the selection
        // describes a file that both does and does not end in a newline.
        return Err(GitError::PartialRefused {
            path: file.path.clone(),
            reason: PartialRefusal::NoNewlineOrdering,
        });
    }

    let mut out = file.header.clone();
    out.extend_from_slice(&body);
    Ok(Some(out))
}

/// Append one hunk if anything in it was selected. Returns its contribution to `delta`.
fn emit_hunk(
    hunk: &RawHunk,
    index: usize,
    chosen: &BTreeSet<Position>,
    delta: i64,
    marker_end: &mut Option<usize>,
    out: &mut Vec<u8>,
) -> Result<Option<i64>> {
    let mut lines: Vec<(u8, &crate::diff::RawLine)> = Vec::with_capacity(hunk.lines.len());
    let (mut old_count, mut new_count) = (0u32, 0u32);
    let mut changed = false;

    for (line, raw) in hunk.lines.iter().enumerate() {
        match raw.origin {
            b'+' => {
                if chosen.contains(&(index, line)) {
                    lines.push((b'+', raw));
                    new_count += 1;
                    changed = true;
                }
            }
            b'-' => {
                if chosen.contains(&(index, line)) {
                    lines.push((b'-', raw));
                    old_count += 1;
                    changed = true;
                } else {
                    lines.push((b' ', raw));
                    old_count += 1;
                    new_count += 1;
                }
            }
            _ => {
                lines.push((b' ', raw));
                old_count += 1;
                new_count += 1;
            }
        }
    }

    if !changed {
        return Ok(None);
    }

    // Deletions either stay deletions or become context, and both count toward the old side,
    // so the old side of a rewritten hunk is always the old side of the original. If this
    // ever fires, the line classification above is wrong and the patch would corrupt the
    // pre-image rather than fail to apply.
    if old_count != hunk.old_lines {
        return Err(GitError::Git {
            detail: format!(
                "hunk {index}: recomputed old count {old_count} != libgit2's {}",
                hunk.old_lines
            ),
        });
    }

    // xdiff's internal 1-based start of the range. For an empty old range libgit2 reports
    // the line *before* it, which is what has to be undone before the arithmetic and redone
    // when printing.
    let old_pos = if hunk.old_lines > 0 {
        hunk.old_start
    } else {
        hunk.old_start + 1
    };
    let new_pos = (i64::from(old_pos) + delta).max(0) as u32;

    out.extend_from_slice(b"@@ -");
    write_range(out, old_pos, old_count);
    out.extend_from_slice(b" +");
    write_range(out, new_pos, new_count);
    out.extend_from_slice(b" @@");
    out.extend_from_slice(hunk.section());
    out.push(b'\n');

    for (origin, raw) in lines {
        out.push(origin);
        out.extend_from_slice(&raw.content);
        if let Some(marker) = &raw.eofnl {
            out.extend_from_slice(marker);
            *marker_end = Some(out.len());
        }
    }

    Ok(Some(i64::from(new_count) - i64::from(old_count)))
}

/// One side of a hunk header, in xdiff's format.
///
/// Reproduces `xdl_emit_hunk_hdr` exactly: the position before an empty range, and no
/// `,count` when the count is 1. Formatting it any other way still applies, but stops a
/// hunk we did not touch from rendering byte-identically to libgit2's own output — and that
/// byte-identity is what makes the property tests able to compare whole patches.
fn write_range(out: &mut Vec<u8>, start: u32, count: u32) {
    let printed = if count == 0 {
        start.saturating_sub(1)
    } else {
        start
    };
    out.extend_from_slice(printed.to_string().as_bytes());
    if count != 1 {
        out.push(b',');
        out.extend_from_slice(count.to_string().as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::RawLine;

    fn line(origin: u8, text: &str) -> RawLine {
        RawLine {
            origin,
            content: format!("{text}\n").into_bytes(),
            eofnl: None,
            old_lineno: None,
            new_lineno: None,
        }
    }

    fn unterminated(origin: u8, text: &str) -> RawLine {
        RawLine {
            origin,
            content: text.as_bytes().to_vec(),
            eofnl: Some(b"\n\\ No newline at end of file\n".to_vec()),
            old_lineno: None,
            new_lineno: None,
        }
    }

    fn file(hunks: Vec<RawHunk>) -> RawFile {
        RawFile {
            path: "f.txt".into(),
            old_path: None,
            header: b"diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n".to_vec(),
            binary_body: None,
            status: git2::Delta::Modified,
            binary: false,
            old_mode: u32::from(git2::FileMode::Blob),
            new_mode: u32::from(git2::FileMode::Blob),
            hunks,
        }
    }

    fn hunk(
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
        lines: Vec<RawLine>,
    ) -> RawHunk {
        RawHunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
            header: format!("@@ -{old_start},{old_lines} +{new_start},{new_lines} @@\n")
                .into_bytes(),
            lines,
        }
    }

    #[test]
    fn an_unselected_deletion_becomes_context() {
        let f = file(vec![hunk(
            1,
            2,
            1,
            2,
            vec![
                line(b'-', "a"),
                line(b'-', "b"),
                line(b'+', "A"),
                line(b'+', "B"),
            ],
        )]);
        // Select only the first deletion and the first addition.
        let chosen = BTreeSet::from([(0, 0), (0, 2)]);
        let patch = synthesize(&f, &chosen).unwrap().unwrap();
        let text = String::from_utf8(patch).unwrap();
        // Emission follows the original diff's line order, so the surviving deletion comes
        // first, `b` becomes context, and the kept addition lands after it. That is what
        // `git add -p` produces for the same choice and what `git apply` reads back — the
        // intuitive "A then b" would need the hunk reordered, which no patch format allows.
        assert!(text.contains("@@ -1,2 +1,2 @@\n-a\n b\n+A\n"), "{text}");
    }

    #[test]
    fn skipped_hunks_do_not_shift_the_ones_after_them() {
        let f = file(vec![
            hunk(
                1,
                1,
                1,
                3,
                vec![line(b'+', "x"), line(b'+', "y"), line(b' ', "a")],
            ),
            hunk(10, 1, 12, 2, vec![line(b' ', "j"), line(b'+', "k")]),
        ]);
        // Only the second hunk's addition.
        let patch = synthesize(&f, &BTreeSet::from([(1, 1)])).unwrap().unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(!text.contains("+x"), "{text}");
        // The first hunk contributed nothing, so the new side is still at the old offset.
        assert!(text.contains("@@ -10 +10,2 @@\n"), "{text}");
    }

    #[test]
    fn accumulated_offsets_follow_the_hunks_actually_emitted() {
        let f = file(vec![
            hunk(
                1,
                1,
                1,
                3,
                vec![line(b' ', "a"), line(b'+', "x"), line(b'+', "y")],
            ),
            hunk(10, 1, 12, 2, vec![line(b' ', "j"), line(b'+', "k")]),
        ]);
        let patch = synthesize(&f, &BTreeSet::from([(0, 1), (1, 1)]))
            .unwrap()
            .unwrap();
        let text = String::from_utf8(patch).unwrap();
        // One line added by the first hunk, so the second starts one further along.
        assert!(text.contains("@@ -1 +1,2 @@\n a\n+x\n"), "{text}");
        assert!(text.contains("@@ -10 +11,2 @@\n j\n+k\n"), "{text}");
    }

    #[test]
    fn an_empty_new_side_prints_the_line_before_it() {
        // Deleting every line of a three-line file.
        let f = file(vec![hunk(
            1,
            3,
            0,
            0,
            vec![line(b'-', "a"), line(b'-', "b"), line(b'-', "c")],
        )]);
        let patch = synthesize(&f, &BTreeSet::from([(0, 0), (0, 1), (0, 2)]))
            .unwrap()
            .unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(text.contains("@@ -1,3 +0,0 @@\n"), "{text}");
    }

    #[test]
    fn an_empty_old_side_keeps_libgit2s_zero() {
        // Creating a file: `@@ -0,0 +1,2 @@`.
        let f = file(vec![hunk(
            0,
            0,
            1,
            2,
            vec![line(b'+', "a"), line(b'+', "b")],
        )]);
        let patch = synthesize(&f, &BTreeSet::from([(0, 0)])).unwrap().unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(text.contains("@@ -0,0 +1 @@\n+a\n"), "{text}");
    }

    #[test]
    fn a_marker_at_the_end_is_kept() {
        let f = file(vec![hunk(
            1,
            1,
            1,
            1,
            vec![line(b'-', "a"), unterminated(b'+', "b")],
        )]);
        let patch = synthesize(&f, &BTreeSet::from([(0, 0), (0, 1)]))
            .unwrap()
            .unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(
            text.ends_with("+b\n\\ No newline at end of file\n"),
            "{text}"
        );
    }

    #[test]
    fn a_marker_stranded_mid_patch_is_refused() {
        // Old file ends `b` with no newline; new file replaces it and adds a line. Keeping
        // `b` (deselecting its deletion) while adding `c` after it is not expressible.
        let f = file(vec![hunk(
            1,
            1,
            1,
            2,
            vec![unterminated(b'-', "b"), line(b'+', "b"), line(b'+', "c")],
        )]);
        let err = synthesize(&f, &BTreeSet::from([(0, 2)])).unwrap_err();
        assert_eq!(
            err,
            GitError::PartialRefused {
                path: "f.txt".into(),
                reason: PartialRefusal::NoNewlineOrdering
            }
        );
    }

    #[test]
    fn dropping_a_line_drops_its_marker() {
        let f = file(vec![hunk(
            1,
            1,
            1,
            2,
            vec![line(b'-', "a"), line(b'+', "a"), unterminated(b'+', "z")],
        )]);
        let patch = synthesize(&f, &BTreeSet::from([(0, 0), (0, 1)]))
            .unwrap()
            .unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(!text.contains("No newline"), "{text}");
    }

    #[test]
    fn selecting_everything_is_recognised_as_whole_file() {
        let f = file(vec![hunk(
            1,
            1,
            1,
            1,
            vec![line(b'-', "a"), line(b'+', "b")],
        )]);
        let chosen = choose(&f, &Selection::Whole).unwrap();
        assert!(chosen.covers_everything);
        assert!(complement(&f, &chosen).is_empty());
    }

    #[test]
    fn an_out_of_range_hunk_is_a_stale_selection() {
        let f = file(vec![hunk(1, 1, 1, 1, vec![line(b'+', "a")])]);
        assert!(matches!(
            choose(&f, &Selection::Hunks { hunks: vec![7] }),
            Err(GitError::StaleSelection { .. })
        ));
    }

    #[test]
    fn selecting_only_context_is_empty_rather_than_a_no_op_patch() {
        let f = file(vec![hunk(
            1,
            2,
            1,
            2,
            vec![line(b' ', "a"), line(b'+', "b")],
        )]);
        let selection = Selection::Lines {
            lines: vec![cide_ipc::git::LineRef { hunk: 0, line: 0 }],
        };
        assert!(matches!(
            choose(&f, &selection),
            Err(GitError::PartialRefused {
                reason: PartialRefusal::Empty,
                ..
            })
        ));
    }
}
