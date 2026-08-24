//! Reading history: one file's diff at a revision, everything a range touched, a file's
//! contents as they were, and what a typed revspec resolves to.
//!
//! Everything here is frozen. There is no `rev` field on anything this module produces and
//! there must never be one — a diff between two commits cannot go stale, so there is no
//! staleness check to perform and no [`cide_ipc::git::Selection`] that could be refused. That is
//! the whole reason [`cide_ipc::history::RevisionDiff`] exists beside
//! [`cide_ipc::git::FileDiff`] rather than reusing it; see that type's own header.
//!
//! # The one trap this module is built around
//!
//! **A pathspec-limited diff cannot detect a rename.** `git_diff_find_similar` pairs an
//! `Added` delta with a `Deleted` one, and a pathspec drops whichever of the two the caller did
//! not name — so the pair never forms and the survivor comes back `Added`, with no old path and
//! a diff that shows the whole file as new. `cide-app`'s `git_diff_file` already documents this
//! for the working-tree case.
//!
//! So [`revision_diff`] builds the diff over the **whole tree** and then picks the delta it was
//! asked about. That costs one tree-to-tree diff per double-click, which for one commit is
//! milliseconds; the shortcut is only unaffordable across a whole log walk, where the cost is
//! paid per commit rather than per gesture.
//!
//! # Which sides are legal, and why the illegal ones are refused rather than guessed
//!
//! [`RevSide`] has three variants and only some pairings are meaningful:
//!
//! | | `Commit` | `FirstParent` | `WorkingTree` |
//! | --- | --- | --- | --- |
//! | as `new` | yes | **no** | yes |
//! | as `old` | yes | yes, when `new` is a `Commit` | **no** |
//!
//! [`RevSide::FirstParent`] is *the first parent of the other side*, so it is meaningless as the
//! new side (there is nothing for it to be relative to) and meaningless against a working tree,
//! which has no parents. [`RevSide::WorkingTree`] is the only side in the family that is not
//! frozen, and a diff whose **old** side moves under it is not a diff of anything: the same call
//! made twice gives two answers with no commit to name in either.
//!
//! Guessing — silently swapping the sides, or reading `FirstParent` as `HEAD^` — would produce a
//! plausible diff of the wrong pair, and a tab titled with the pair the caller asked for. So
//! they are refused.

use std::path::Path;

use cide_ipc::git::GitError;
use cide_ipc::history::{
    ResolvedRev, RevSide, RevisionBlob, RevisionChange, RevisionDiff, RevisionRange,
};
use git2::{Commit, Diff, DiffOptions, ObjectType, Oid, Patch, Repository, Tree};

use crate::{Result, Wrap, diff, repo as repo_mod};

/// How many files a [`RevisionRange`] lists before it says it stopped.
///
/// A range across a release is routinely thousands of files, and the pane is a list nobody
/// scrolls past the first screen of. The cap bounds the wire payload and, more to the point, the
/// number of patches generated to count lines — the expensive half. [`RevisionRange::truncated`]
/// is what stops the shortened list reading as the whole answer.
pub const RANGE_FILE_CAP: usize = 2_000;

/// How much of a file [`file_at_revision`] returns.
///
/// The same two megabytes [`crate::blame::MAX_BLAME_BYTES`] uses, and for the same reason: it is
/// about the point where the surface showing the text stops being usable. A refusal would be
/// wrong here though — the top of an eight-megabyte file is still worth reading — so this
/// truncates and says so, and [`RevisionBlob::bytes`] keeps the real size.
pub const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

/// Blobs larger than this are marked binary by libgit2 rather than diffed, when the counts are
/// all that is wanted.
///
/// Applied by [`range_files`] and **not** by [`revision_diff`]: the range view generates a patch
/// per file only to add up two numbers, so a forty-megabyte generated file that never reaches
/// xdiff costs nothing and loses nothing. The single-file view is the one the user explicitly
/// asked to read, and capping it there would report a perfectly ordinary large source file as
/// binary.
const COUNT_MAX_BLOB: i32 = 1024 * 1024;

// --- one file at a revision -------------------------------------------------------------------

/// One file's hunks between two revisions.
///
/// [`GitError::NoSuchChange`] when the commit did not touch the path — not an empty diff, which
/// would render as *"this file is identical"* and is a different claim from *"this commit is not
/// about this file"*.
pub fn revision_diff(
    root: &Path,
    path: &str,
    new: &RevSide,
    old: &RevSide,
) -> Result<RevisionDiff> {
    let repo = repo_mod::open(root)?;
    let (new_side, old_side) = resolve_sides(&repo, new, old)?;
    let diff = build_diff(&repo, &new_side, &old_side, false)?;

    let Some(index) = locate(&diff, path) else {
        return Err(GitError::NoSuchChange {
            path: path.to_string(),
        });
    };
    let Some(raw) = diff::raw_file_at(&diff, index)? else {
        return Err(GitError::NoSuchChange {
            path: path.to_string(),
        });
    };

    // The whole file on both sides, for the pane that draws more than the hunks. The same
    // helpers the staging surface uses, so the skip rules and the byte cap cannot drift
    // between the two diff panes.
    let texts = if diff::text_skip(&raw) {
        diff::DiffTexts::NONE
    } else {
        let old_path = raw.old_path.as_deref().unwrap_or(&raw.path);
        let old_read = match &old_side {
            Side::Tree { tree, .. } => diff::tree_blob_read(&repo, tree.as_ref(), old_path),
            // Refused as the old side before anything was resolved.
            Side::Workdir => diff::TextRead::Missing,
        };
        let new_read = match &new_side {
            Side::Tree { tree, .. } => diff::tree_blob_read(&repo, tree.as_ref(), &raw.path),
            Side::Workdir => diff::workdir_read(&repo, &raw.path),
        };
        diff::DiffTexts::combine(old_read, new_read)
    };

    Ok(RevisionDiff {
        path: raw.path.clone(),
        old_path: raw.old_path.clone(),
        new: new.clone(),
        old: old.clone(),
        new_oid: new_side.oid().map(|oid| oid.to_string()),
        old_oid: old_side.oid().map(|oid| oid.to_string()),
        status: diff::state_of(raw.status),
        binary: raw.binary,
        old_mode: raw.old_mode,
        new_mode: raw.new_mode,
        // `diff::hunk_views` and not a second mapping. The `LineOrigin` translation and the
        // trailing-newline rule are the two places a copy would drift, and it would drift on the
        // read-only surface — the one nobody stages from, so the one where a wrong line number
        // produces no visible failure until somebody trusts it.
        hunks: diff::hunk_views(&raw),
        old_text: texts.old_text,
        new_text: texts.new_text,
        texts_omitted: texts.omitted,
    })
}

/// The index of the delta for `path`, matching either side of it.
///
/// Either side, because a rename is one delta with two names and the caller may hold either:
/// the log's file list names the post-rename path, a blame hop or a history tab may name the
/// pre-rename one, and both mean the same change.
fn locate(diff: &Diff<'_>, path: &str) -> Option<usize> {
    let mut by_old = None;
    for (index, delta) in diff.deltas().enumerate() {
        if path_of(delta.new_file().path_bytes()).as_deref() == Some(path) {
            return Some(index);
        }
        if by_old.is_none() && path_of(delta.old_file().path_bytes()).as_deref() == Some(path) {
            by_old = Some(index);
        }
    }
    by_old
}

// --- every file in a range ----------------------------------------------------------------------

/// Every file that differs between two revisions, with line counts.
pub fn range_files(root: &Path, new: &RevSide, old: &RevSide) -> Result<RevisionRange> {
    let repo = repo_mod::open(root)?;
    let (new_side, old_side) = resolve_sides(&repo, new, old)?;
    let diff = build_diff(&repo, &new_side, &old_side, true)?;

    let total = diff.deltas().len();
    let mut files = Vec::with_capacity(total.min(RANGE_FILE_CAP));
    for index in 0..total.min(RANGE_FILE_CAP) {
        let Some(delta) = diff.get_delta(index) else {
            continue;
        };
        // `Patch::from_diff` generates one delta's patch on demand. `Diff::stats` is the wrong
        // tool for exactly this: it loops every delta and generates all of them, so the
        // `max_size` gate above would save nothing.
        let counts = Patch::from_diff(&diff, index).wrap()?;
        let (additions, deletions) = match &counts {
            Some(patch) => {
                let (_context, additions, deletions) = patch.line_stats().wrap()?;
                (additions as u32, deletions as u32)
            }
            // libgit2 has no patch for an unmodified or binary-without-content delta.
            None => (0, 0),
        };
        // Read *after* the patch: libgit2 only decides a delta is binary while generating it,
        // and the flag on the delta struct is filled in at that moment.
        let binary = delta.flags().is_binary();
        let new_path = path_of(delta.new_file().path_bytes());
        let old_path = path_of(delta.old_file().path_bytes());
        files.push(RevisionChange {
            path: new_path
                .clone()
                .or_else(|| old_path.clone())
                .unwrap_or_default(),
            old_path: old_path.filter(|old| Some(old.as_str()) != new_path.as_deref()),
            status: diff::state_of(delta.status()),
            binary,
            additions,
            deletions,
        });
    }

    Ok(RevisionRange {
        new: new.clone(),
        old: old.clone(),
        new_oid: new_side.oid().map(|oid| oid.to_string()),
        old_oid: old_side.oid().map(|oid| oid.to_string()),
        new_summary: new_side.summary().to_string(),
        old_summary: old_side.summary().to_string(),
        files,
        truncated: total > RANGE_FILE_CAP,
    })
}

// --- one file's contents --------------------------------------------------------------------

/// The bytes of `path` as of `rev`.
///
/// `binary` and `truncated` are separate flags and never both meaningful, because the pane says
/// two different sentences: a binary blob has nothing to show at all, and a large source file has
/// too much to show but the top of it is still what the reader came for.
pub fn file_at_revision(root: &Path, path: &str, rev: &str) -> Result<RevisionBlob> {
    let repo = repo_mod::open(root)?;
    let commit = commit_at(&repo, rev)?;
    let tree = commit.tree().wrap()?;
    let entry = tree
        .get_path(Path::new(path))
        .map_err(|_| GitError::NotTracked {
            path: path.to_string(),
        })?;
    // A directory or a submodule; neither has contents to show, and from the caller's side the
    // answer is the same as for a path that is not there.
    if entry.kind() != Some(ObjectType::Blob) {
        return Err(GitError::NotTracked {
            path: path.to_string(),
        });
    }
    let blob = repo.find_blob(entry.id()).wrap()?;
    let bytes = blob.size();

    if blob.is_binary() {
        return Ok(RevisionBlob {
            text: String::new(),
            oid: blob.id().to_string(),
            binary: true,
            bytes: bytes.min(u32::MAX as usize) as u32,
            truncated: false,
        });
    }

    let content = blob.content();
    let truncated = content.len() > MAX_BLOB_BYTES;
    let shown = if truncated {
        // Back off to the last line break inside the cap. A half line at the bottom of a
        // read-only pane reads as file corruption, and the reader has no way to tell it from
        // one — the text is right up to that point and then simply stops mid-token.
        let head = &content[..MAX_BLOB_BYTES];
        match head.iter().rposition(|&byte| byte == b'\n') {
            Some(at) => &head[..=at],
            None => head,
        }
    } else {
        content
    };

    Ok(RevisionBlob {
        // Lossy and not a refusal: a source file with one Latin-1 byte in a comment is still a
        // file somebody wants to read, and the replacement character says where the problem is.
        text: String::from_utf8_lossy(shown).into_owned(),
        oid: blob.id().to_string(),
        binary: false,
        bytes: bytes.min(u32::MAX as usize) as u32,
        truncated,
    })
}

// --- resolving what a user typed ----------------------------------------------------------------

/// What `spec` resolves to, with the three refusals a revision box needs to explain itself.
///
/// This is what a picker calls so that a typed name becomes an oid **before** it is persisted.
/// A tab that saved the string `HEAD~3` would name a different commit tomorrow and show a
/// different diff, silently — the same rule [`RevSide::FirstParent`]'s own doc states from the
/// other direction.
pub fn resolve_rev(root: &Path, spec: &str) -> Result<ResolvedRev> {
    let repo = repo_mod::open(root)?;
    let parsed = repo.revparse(spec).map_err(|error| match error.code() {
        // Its own variant because the fix is specific and mechanical — type more characters —
        // and telling somebody their revspec does not parse when it parses fine and matches four
        // objects sends them looking for a syntax error.
        git2::ErrorCode::Ambiguous => GitError::AmbiguousRev {
            spec: spec.to_string(),
        },
        _ => GitError::BadRevspec {
            spec: spec.to_string(),
            detail: error.message().to_string(),
        },
    })?;

    let range = parsed.mode().is_range() || parsed.mode().is_merge_base();
    // `revparse` names the *older* end `from` and the newer end `to`, which is the same order
    // `a..b` is written in and the same order [`ResolvedRev`] carries.
    let from = parsed.from().ok_or_else(|| GitError::BadRevspec {
        spec: spec.to_string(),
        detail: "resolved to nothing".to_string(),
    })?;
    let from_commit = peel(spec, from)?;
    let to_commit = match parsed.to() {
        Some(object) => Some(peel(spec, object)?),
        None => None,
    };

    // The branch name when the spec named one, so a tab opened from a chip is titled `main`
    // rather than an abbreviation the user has to decode. What was typed, not the reference's
    // own shorthand: resolving `HEAD` and then labelling the tab `main` claims a branch the user
    // did not name and stops being true the moment they switch.
    let named = !range && repo.resolve_reference_from_short_name(spec).is_ok();
    let label = if named {
        spec.to_string()
    } else {
        match &to_commit {
            // The two-dot separator is U+2025, which is what the tab strip uses so an ellipsis
            // in a truncated title cannot be mistaken for a range.
            Some(to) => format!("{} ‥ {}", abbrev(&repo, from_commit), abbrev(&repo, *to)),
            None => abbrev(&repo, from_commit),
        }
    };

    Ok(ResolvedRev {
        spec: spec.to_string(),
        from: from_commit.to_string(),
        to: to_commit.map(|oid| oid.to_string()),
        range,
        label,
    })
}

/// Peel a resolved object to a commit, or say what it is instead.
fn peel(spec: &str, object: &git2::Object<'_>) -> Result<Oid> {
    let kind = object.kind();
    object
        .peel(ObjectType::Commit)
        .map(|commit| commit.id())
        .map_err(|_| GitError::NotACommit {
            spec: spec.to_string(),
            // git's own word, so the message reads *`v1.2.0^{tree}` is a tree* rather than the
            // unhelpful *not a commit*.
            kind: kind.map(|kind| kind.str().to_string()).unwrap_or_default(),
        })
}

/// libgit2's own uniqueness rule, so an abbreviation cannot disagree with what `git log` prints
/// in the same repository. Falls back to eight characters if the object cannot be looked up,
/// which is the fixed width the rest of this crate uses.
fn abbrev(repo: &Repository, oid: Oid) -> String {
    repo.find_object(oid, None)
        .ok()
        .and_then(|object| object.short_id().ok())
        .and_then(|buf| buf.as_str().ok().map(str::to_string))
        .unwrap_or_else(|| oid.to_string().chars().take(8).collect())
}

// --- sides ----------------------------------------------------------------------------------

/// One side of a comparison, resolved.
enum Side<'repo> {
    /// A commit's tree. `tree` is `None` for the empty tree, which is what the first parent of a
    /// root commit is — every file in that commit is an addition against nothing, and that is
    /// exactly the diff `git show <root>` prints.
    Tree {
        tree: Option<Tree<'repo>>,
        oid: Option<Oid>,
        summary: String,
    },
    /// The files on disk right now, index included.
    Workdir,
}

impl Side<'_> {
    fn oid(&self) -> Option<Oid> {
        match self {
            Side::Tree { oid, .. } => *oid,
            Side::Workdir => None,
        }
    }

    fn summary(&self) -> &str {
        match self {
            Side::Tree { summary, .. } => summary,
            Side::Workdir => "",
        }
    }

    fn tree(&self) -> Option<&Tree<'_>> {
        match self {
            Side::Tree { tree, .. } => tree.as_ref(),
            Side::Workdir => None,
        }
    }
}

fn resolve_sides<'repo>(
    repo: &'repo Repository,
    new: &RevSide,
    old: &RevSide,
) -> Result<(Side<'repo>, Side<'repo>)> {
    // Refused before anything is resolved, so an illegal pairing costs no object reads and the
    // message is about the pairing rather than about whichever side happened to fail first.
    if matches!(new, RevSide::FirstParent) {
        return Err(illegal(
            "firstParent",
            "the first parent of what? `FirstParent` is relative to the other side, so it can \
             only be the old one",
        ));
    }
    if matches!(old, RevSide::WorkingTree) {
        return Err(illegal(
            "workingTree",
            "the working tree can only be the new side; a comparison whose old side moves while \
             you read it is not a diff of anything",
        ));
    }
    if matches!(old, RevSide::FirstParent) && !matches!(new, RevSide::Commit { .. }) {
        return Err(illegal(
            "firstParent",
            "`FirstParent` needs a commit on the other side to be the parent of",
        ));
    }

    let new_side = match new {
        RevSide::Commit { oid } => tree_side(repo, oid)?,
        RevSide::WorkingTree => Side::Workdir,
        RevSide::FirstParent => unreachable!("refused above"),
    };
    let old_side = match old {
        RevSide::Commit { oid } => tree_side(repo, oid)?,
        RevSide::FirstParent => {
            let RevSide::Commit { oid } = new else {
                unreachable!("refused above")
            };
            let commit = commit_at(repo, oid)?;
            match commit.parents().next() {
                Some(parent) => Side::Tree {
                    tree: Some(parent.tree().wrap()?),
                    oid: Some(parent.id()),
                    summary: summary_of(&parent),
                },
                // A root commit's first parent is the empty tree, not an error: `git show` on the
                // first commit of a repository prints every file as an addition, and so does
                // this.
                None => Side::Tree {
                    tree: None,
                    oid: None,
                    summary: String::new(),
                },
            }
        }
        RevSide::WorkingTree => unreachable!("refused above"),
    };
    Ok((new_side, old_side))
}

fn tree_side<'repo>(repo: &'repo Repository, rev: &str) -> Result<Side<'repo>> {
    let commit = commit_at(repo, rev)?;
    Ok(Side::Tree {
        tree: Some(commit.tree().wrap()?),
        oid: Some(commit.id()),
        summary: summary_of(&commit),
    })
}

/// The whole-tree diff between two resolved sides.
///
/// **No pathspec, ever.** See the module header: a pathspec is what turns a rename into an
/// `Added`, and it does it silently.
fn build_diff<'repo>(
    repo: &'repo Repository,
    new: &Side<'repo>,
    old: &Side<'repo>,
    counting: bool,
) -> Result<Diff<'repo>> {
    let mut options = DiffOptions::new();
    options.context_lines(3).include_typechange(true);
    if counting {
        options.max_size(i64::from(COUNT_MAX_BLOB));
    }
    if matches!(new, Side::Workdir) {
        // A file that exists on disk and not in the commit *is* a difference between the two
        // sides. Hiding it would make the file list disagree with the diff a double-click on the
        // same path produces, which is the sort of inconsistency a user reads as a bug in the
        // diff rather than as a policy about untracked files. Ignored files stay out —
        // `include_ignored` is off by default.
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
    }

    let mut built = match new {
        Side::Tree { tree, .. } => repo
            .diff_tree_to_tree(old.tree(), tree.as_ref(), Some(&mut options))
            .wrap()?,
        // The same call `DiffSide::Combined` makes in `diff.rs`, with a different tree: index and
        // working tree together against a commit, which is what "how does my checkout differ
        // from that release" means.
        Side::Workdir => repo
            .diff_tree_to_workdir_with_index(old.tree(), Some(&mut options))
            .wrap()?,
    };

    let mut find = git2::DiffFindOptions::new();
    // Renames on, copies off — git's own default for `git show` and `git diff`, so the hunks
    // here match what the binary prints for the same pair. Copy detection would additionally
    // pair an added file with an unmodified one, which changes the *status* of deltas the user
    // is comparing against `git show` output.
    find.renames(true).copies(false);
    built.find_similar(Some(&mut find)).wrap()?;
    Ok(built)
}

// --- shared helpers ---------------------------------------------------------------------------

/// A commit named by an oid this process already resolved and persisted.
///
/// Its refusals are [`GitError::NoSuchRevision`] and [`GitError::NoSuchCommit`] rather than
/// [`GitError::BadRevspec`], because nothing here was typed by a user: a `RevSide::Commit` holds
/// a full oid that [`resolve_rev`] produced, so the interesting failure is *that commit is gone*
/// — a rewrite, a `gc`, a shallow clone — and not *that does not parse*.
fn commit_at<'repo>(repo: &'repo Repository, rev: &str) -> Result<Commit<'repo>> {
    let object = repo
        .revparse_single(rev)
        .map_err(|error| match error.code() {
            git2::ErrorCode::Ambiguous => GitError::AmbiguousRev {
                spec: rev.to_string(),
            },
            git2::ErrorCode::NotFound => GitError::NoSuchCommit {
                rev: rev.to_string(),
            },
            _ => GitError::NoSuchRevision {
                rev: rev.to_string(),
            },
        })?;
    let kind = object.kind();
    object.peel_to_commit().map_err(|_| GitError::NotACommit {
        spec: rev.to_string(),
        kind: kind.map(|kind| kind.str().to_string()).unwrap_or_default(),
    })
}

fn summary_of(commit: &Commit<'_>) -> String {
    commit
        .summary_bytes()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default()
}

fn path_of(bytes: Option<&[u8]>) -> Option<String> {
    bytes.map(|bytes| String::from_utf8_lossy(bytes).into_owned())
}

/// The refusal for an illegal side pairing.
///
/// [`GitError::BadRevspec`] and not a variant of its own, because `cide-ipc` is frozen for this
/// milestone and this is the closest existing shape: *this revision expression cannot be used,
/// and here is the sentence saying why*. `spec` carries the offending side's wire tag so the
/// message can name it. A dedicated `InvalidRevSide` would be better — the pairing is a fact
/// about two sides, not about one string — and is worth adding the next time that file moves.
fn illegal(side: &str, detail: &str) -> GitError {
    GitError::BadRevspec {
        spec: side.to_string(),
        detail: detail.to_string(),
    }
}
