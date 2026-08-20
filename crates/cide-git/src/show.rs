//! One commit's contents: the changed-file list, which parent it was diffed against, and the
//! `+`/`−` counts. (M19)
//!
//! # The counts are the whole problem
//!
//! Listing a commit's files is one tree-to-tree diff and is fast whatever the commit. Counting
//! its lines is a *content* diff of every file in it, and on a four-thousand-file merge that is
//! the difference between a pane that appears instantly and a pane that appears after four
//! seconds. So the file list and the counts are two calls, and the first one is allowed to say
//! [`LineCount::NotCounted`].
//!
//! ## `Diff::stats()` is the wrong tool
//!
//! It is the obvious one, and it is a trap: `git_diff_get_stats` loops every delta and generates
//! a **patch** for each, so it runs xdiff over the whole commit before returning a single number.
//! There is no way to bound it, no way to skip a file and no way to stop.
//!
//! [`Patch::from_diff`] generates **one** delta on demand, which is what lets every gate apply
//! *per file, before xdiff runs*:
//!
//! * The blob's size, read out of the **object header** before anything else happens, so the
//!   forty-megabyte generated file never reaches xdiff. [`DiffOptions::max_size`] is set as well
//!   and is the backstop, but it cannot be the gate on its own for two reasons: libgit2 applies it
//!   during patch generation, and its answer is *binary*, where the reader asked *why* —
//!   [`LineCount::TooLarge`] carries the number. See `blob_size` for why `DiffFile::size()` is not
//!   the field to read here.
//! * [`DetailLimits::count_files`] — above it, **no patch is generated at all** and every file is
//!   [`LineCount::NotCounted`]. This is the rule that makes a four-thousand-file commit return in
//!   the time of one tree diff.
//! * [`DetailLimits::wire_files`] — the list is truncated on the wire while
//!   [`CommitTotals::files`] stays honest, so the pane can say *"showing 2000 of 12000"* rather
//!   than quietly being wrong about how big the commit was.
//!
//! `Patch::from_diff` returning `Ok(None)` means libgit2 called the delta binary — we already
//! know the file changed, we just cannot count lines in it — so that is [`LineCount::Binary`],
//! `0/0`, and `partial`. Which is exactly what `git show --numstat` prints for a binary file: a
//! dash in both columns, not a zero.
//!
//! # There is no cancellation, so there is a deadline
//!
//! libgit2 exposes no hook inside a diff, so a counting pass cannot be interrupted from outside.
//! [`line_counts`] therefore checks [`LAZY_DEADLINE`] between files and marks the rest
//! [`LineCount::TimedOut`] — distinct from `NotCounted` because a retry is worth offering for one
//! and pointless for the other.

use std::path::Path;
use std::time::{Duration, Instant};

use cide_ipc::git::GitError;
use cide_ipc::history::{
    CommitDetail, CommitFile, CommitLineCounts, CommitTotals, DiffAgainst, LineCount,
};
use git2::{Commit, Diff, DiffOptions, Oid, Patch, Repository, Tree};

use crate::{Result, Wrap, diff as diff_mod, log, repo as repo_mod};

/// Files a first response puts on the wire by default.
pub const COMMIT_FILE_CAP: usize = 1_000;
/// Above this many deltas, the first response counts nothing. See the module header — this is the
/// gate that makes a huge merge cheap, and it works by *not generating patches*, not by
/// generating them faster.
pub const MAX_COUNT_FILES: usize = 400;
/// Per-file byte ceiling for counting. One mebibyte is libgit2's own default `max_size` and the
/// point past which a "source file" is a generated artefact or a checked-in blob.
pub const MAX_COUNT_BYTES: u64 = 1 << 20;
/// The most files any response will put on the wire, whatever the caller asks for. A
/// twelve-thousand-file merge must not put twelve thousand rows on the IPC wire in order to have
/// the renderer drop them.
pub const MAX_DETAIL_FILES: usize = 2_000;
/// How long the lazy counting pass may run. There is no cancellation, so this *is* the bound.
pub const LAZY_DEADLINE: Duration = Duration::from_secs(2);
/// The most paths one [`line_counts`] request may name. A caller asking for more than this has a
/// bug, and the bug would otherwise be a linear scan of the request list per delta.
pub const LAZY_MAX_PATHS: usize = 5_000;

/// The four numbers that decide what a `show` costs.
///
/// A struct rather than four arguments because three of the four are almost always the defaults
/// and the fourth is the one a caller means to change; four positional `usize`s at a call site is
/// how `wire_files` and `count_files` end up swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetailLimits {
    /// Above this many deltas, nothing is counted.
    pub count_files: usize,
    /// Per-file byte ceiling; a bigger blob is [`LineCount::TooLarge`].
    pub count_bytes: u64,
    /// How many files reach the wire. Clamped to [`MAX_DETAIL_FILES`].
    pub wire_files: usize,
    /// Wall-clock ceiling on the counting pass, or `None` for no deadline. `None` is right for
    /// [`detail`], which is already bounded by `count_files`, and wrong for [`line_counts`],
    /// which is asked to count precisely the cases `count_files` refused.
    pub deadline: Option<Duration>,
}

impl Default for DetailLimits {
    fn default() -> Self {
        Self {
            count_files: MAX_COUNT_FILES,
            count_bytes: MAX_COUNT_BYTES,
            wire_files: COMMIT_FILE_CAP,
            deadline: None,
        }
    }
}

impl DetailLimits {
    /// The limits the lazy second pass runs under: count everything, and stop on the clock.
    pub fn lazy() -> Self {
        Self {
            count_files: usize::MAX,
            count_bytes: MAX_COUNT_BYTES,
            wire_files: MAX_DETAIL_FILES,
            deadline: Some(LAZY_DEADLINE),
        }
    }

    fn wire(&self) -> usize {
        self.wire_files.min(MAX_DETAIL_FILES)
    }
}

/// Everything the commit detail pane draws.
pub fn detail(
    root: &Path,
    rev: &str,
    parent: Option<u32>,
    limits: DetailLimits,
) -> Result<CommitDetail> {
    let repo = repo_mod::open(root)?;
    let commit = resolve(&repo, rev)?;
    let chips = log::ref_chips(&repo)?;
    let row = log::commit_row(repo_mod::repo_id(root), &commit, &chips);

    let (against, old_tree) = against(&repo, &commit, parent)?;
    let new_tree = commit.tree().wrap()?;
    let diff = build(&repo, old_tree.as_ref(), &new_tree, limits)?;

    // `None` here is the whole reason `detail` is cheap: over `count_files` deltas the counting
    // pass never runs and no patch is ever generated.
    let counted = diff.deltas().len() <= limits.count_files;
    let (files, total, _) = collect(&repo, &diff, limits, counted, None, None)?;
    let files_truncated = diff.deltas().len() > limits.wire();

    let committer = commit.committer();
    Ok(CommitDetail {
        merge: commit.parent_count() > 1,
        commit: row,
        // The **full** message, summary included. A message whose body repeats the summary — what
        // every `git commit -m` of a one-line change produces once someone adds a trailer —
        // cannot be reconstructed from a split, and the split has to be undone anywhere the
        // message is copied or amended.
        message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
        committer: String::from_utf8_lossy(committer.name_bytes()).into_owned(),
        committer_email: String::from_utf8_lossy(committer.email_bytes()).into_owned(),
        against,
        files,
        files_truncated,
        total,
    })
}

/// The second pass: the same file list, with the counts filled in.
///
/// `paths` names the subset to count — the ones the pane can actually see — and an empty slice
/// means all of them. The list that comes back is the **same paths in the same order** as
/// [`CommitDetail::files`], so the renderer patches by index; matching by path string would go
/// wrong for the two entries a rename produces.
pub fn line_counts(
    root: &Path,
    rev: &str,
    parent: Option<u32>,
    paths: &[String],
    limits: DetailLimits,
) -> Result<CommitLineCounts> {
    let repo = repo_mod::open(root)?;
    let commit = resolve(&repo, rev)?;
    let (_, old_tree) = against(&repo, &commit, parent)?;
    let new_tree = commit.tree().wrap()?;
    let diff = build(&repo, old_tree.as_ref(), &new_tree, limits)?;

    let wanted: Option<std::collections::HashSet<&str>> = if paths.is_empty() {
        None
    } else {
        Some(
            paths
                .iter()
                .take(LAZY_MAX_PATHS)
                .map(String::as_str)
                .collect(),
        )
    };
    let started = Instant::now();
    let (files, total, deadline_hit) = collect(
        &repo,
        &diff,
        limits,
        true,
        wanted.as_ref(),
        limits.deadline.map(|d| (started, d)),
    )?;

    Ok(CommitLineCounts {
        files,
        total,
        deadline_hit,
    })
}

// --- the shared middle ---------------------------------------------------------------------------

/// Resolve a revspec to a commit, with the two refusals kept apart.
fn resolve<'r>(repo: &'r Repository, rev: &str) -> Result<Commit<'r>> {
    let object = repo
        .revparse_single(rev)
        .map_err(|_| GitError::NoSuchCommit {
            rev: rev.to_string(),
        })?;
    let kind = object.kind();
    object.peel_to_commit().map_err(|_| GitError::NotACommit {
        spec: rev.to_string(),
        // "that is a tree" is actionable; libgit2's own message here is not.
        kind: kind
            .map(|k| k.str().to_string())
            .unwrap_or_else(|| "object".into()),
    })
}

/// Which side the file list is computed against, and that side's tree.
///
/// First parent by default — `git show --first-parent`, and the only choice that makes a merge's
/// diff small enough to read. `parent: Some(n)` selects another, and a root commit is
/// [`DiffAgainst::EmptyTree`], where every file is an addition.
fn against<'r>(
    repo: &'r Repository,
    commit: &Commit<'r>,
    parent: Option<u32>,
) -> Result<(DiffAgainst, Option<Tree<'r>>)> {
    let count = commit.parent_count() as u32;
    if count == 0 {
        return Ok((DiffAgainst::EmptyTree, None));
    }
    let index = parent.unwrap_or(0);
    if index >= count {
        // A mis-wired "compare against parent 3" on a two-parent merge. Refused rather than
        // clamped: clamping shows a diff against a *different* parent under a label that says
        // otherwise, which is a lie the user cannot see.
        return Err(GitError::NoSuchCommit {
            rev: format!("{}^{}", commit.id(), index + 1),
        });
    }
    let oid = commit.parent_id(index as usize).wrap()?;
    let tree = repo.find_commit(oid).wrap()?.tree().wrap()?;
    Ok((
        DiffAgainst::Parent {
            index,
            oid: oid.to_string(),
        },
        Some(tree),
    ))
}

fn build<'r>(
    repo: &'r Repository,
    old: Option<&Tree<'r>>,
    new: &Tree<'r>,
    limits: DetailLimits,
) -> Result<Diff<'r>> {
    let mut opts = DiffOptions::new();
    // A typechange (a file becoming a symlink) is a change a commit made and a row the pane must
    // show; without this libgit2 reports it as a delete plus an add.
    opts.include_typechange(true);
    // libgit2 marks anything larger binary itself, before xdiff sees it. `i64` because that is
    // libgit2's own type for the option; the value never approaches the sign boundary.
    opts.max_size(limits.count_bytes.min(i64::MAX as u64) as i64);
    let mut diff = repo
        .diff_tree_to_tree(old, Some(new), Some(&mut opts))
        .wrap()?;
    // Renames are found here and not merely reported: both sides are in the diff (it is a whole
    // tree against a whole tree, with no pathspec), which is the condition `find_similar` needs
    // and the one `cmd/git.rs::git_diff_file` cannot meet.
    let mut find = git2::DiffFindOptions::new();
    find.renames(true).copies(false).for_untracked(false);
    diff.find_similar(Some(&mut find)).wrap()?;
    Ok(diff)
}

/// Turn a diff into the wire list, counting the files the gates allow.
///
/// Returns the list, the totals, and whether a deadline cut the pass short.
fn collect(
    repo: &Repository,
    diff: &Diff<'_>,
    limits: DetailLimits,
    counting: bool,
    wanted: Option<&std::collections::HashSet<&str>>,
    deadline: Option<(Instant, Duration)>,
) -> Result<(Vec<CommitFile>, CommitTotals, bool)> {
    // The object database, for blob sizes. See `blob_size`.
    let odb = repo.odb().wrap()?;
    let count = diff.deltas().len();
    let wire = limits.wire();
    let mut files = Vec::with_capacity(count.min(wire));
    let mut added_total = 0u32;
    let mut deleted_total = 0u32;
    let mut partial = false;
    let mut deadline_hit = false;

    for index in 0..count {
        let Some(delta) = diff.get_delta(index) else {
            continue;
        };
        let path = path_of(&delta.new_file())
            .or_else(|| path_of(&delta.old_file()))
            .unwrap_or_default();
        let old_path = match delta.status() {
            git2::Delta::Renamed | git2::Delta::Copied => path_of(&delta.old_file()),
            _ => None,
        };
        let mut binary = delta.old_file().is_binary() || delta.new_file().is_binary();
        let bytes = blob_size(&odb, &delta.old_file()).max(blob_size(&odb, &delta.new_file()));

        // Two ways to end up uncounted and they mean the same thing to the wire: the whole
        // commit was too big to count (`!counting`), or this file is not one the caller asked
        // about. Both are `NotCounted` and both make the totals a lower bound.
        let skipped = !counting || wanted.is_some_and(|set| !set.contains(path.as_str()));
        let lines = if skipped {
            partial = true;
            LineCount::NotCounted
        } else if deadline_hit {
            partial = true;
            LineCount::TimedOut
        } else if bytes > limits.count_bytes {
            // Checked here as well as through `max_size`, because the two answers differ: libgit2
            // would say *binary* and the reader asked *why*. `TooLarge` carries the number.
            partial = true;
            LineCount::TooLarge { bytes }
        } else if let Some((started, budget)) = deadline
            && started.elapsed() > budget
        {
            deadline_hit = true;
            partial = true;
            LineCount::TimedOut
        } else {
            match Patch::from_diff(diff, index).wrap()? {
                // A patch exists, but it may still be a *binary* one. libgit2 decides that while
                // generating — `git_diff_file.flags` gains `GIT_DIFF_FLAG_BINARY` in
                // `patch_generate.c`, not while building the tree diff — so the delta has to be
                // re-read afterwards. Reading it before generation, which is the obvious thing to
                // do, gets `false` for every binary file in a tree-to-tree diff.
                Some(patch) if !delta_is_binary(diff, index) => {
                    let (_, added, deleted) = patch.line_stats().wrap()?;
                    added_total = added_total.saturating_add(added as u32);
                    deleted_total = deleted_total.saturating_add(deleted as u32);
                    LineCount::Counted {
                        added: added as u32,
                        deleted: deleted as u32,
                    }
                }
                // Binary, one way or the other: `Ok(None)` from `from_diff` (an unmodified or
                // unreadable delta) or a patch libgit2 flagged. We already know the file changed;
                // there are simply no lines to count, which is what `git --numstat` says with a
                // dash in both columns rather than a zero.
                _ => {
                    binary = true;
                    partial = true;
                    LineCount::Binary
                }
            }
        };

        if files.len() < wire {
            files.push(CommitFile {
                path,
                old_path,
                status: diff_mod::state_of(delta.status()),
                binary,
                lines,
            });
        }
    }

    Ok((
        files,
        CommitTotals {
            // The **honest** count, not the length of the list above. A pane that reported the
            // truncated length would say a twelve-thousand-file merge touched two thousand files.
            files: count as u32,
            added: added_total,
            deleted: deleted_total,
            partial,
        },
        deadline_hit,
    ))
}

/// One side's blob size, straight out of the object header.
///
/// **Not** `DiffFile::size()`. In a tree-to-tree diff libgit2 leaves that field zero until the
/// blob is actually loaded, which happens during patch generation — so a size check written
/// against it passes every oversized file straight through to xdiff, which is the one thing the
/// cap exists to prevent. `Odb::read_header` reads the object header and not the object, so this
/// costs a lookup rather than a megabyte.
fn blob_size(odb: &git2::Odb<'_>, file: &git2::DiffFile<'_>) -> u64 {
    let oid = file.id();
    if oid.is_zero() {
        return 0;
    }
    odb.read_header(oid)
        .map(|(size, _)| size as u64)
        .unwrap_or(0)
}

/// Did libgit2 flag this delta binary while generating its patch?
fn delta_is_binary(diff: &Diff<'_>, index: usize) -> bool {
    diff.get_delta(index)
        .is_some_and(|delta| delta.old_file().is_binary() || delta.new_file().is_binary())
}

fn path_of(file: &git2::DiffFile<'_>) -> Option<String> {
    file.path_bytes()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

/// The oid of a commit named by a revspec, for callers that only need the identity.
///
/// Here rather than at the call site so that "what does this string resolve to" has one answer
/// with one pair of refusals behind it.
pub fn resolve_oid(root: &Path, rev: &str) -> Result<Oid> {
    let repo = repo_mod::open(root)?;
    Ok(resolve(&repo, rev)?.id())
}
