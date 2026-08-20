//! Reading diffs out of libgit2 in a form that can be re-emitted byte for byte.
//!
//! # Why the raw model exists
//!
//! [`FileDiff`] is what the frontend renders; [`RawFile`] is what [`crate::patch`] rewrites.
//! They are not the same thing and merging them would lose the bytes that matter. A
//! `DiffLineView` carries a `String` with the newline stripped, because that is what a React
//! row wants. A `RawLine` carries the exact `&[u8]` libgit2 produced, including whether it
//! ended in a newline and including the `\ No newline at end of file` marker that followed
//! it — because those bytes are the difference between a patch git accepts and one that
//! silently truncates a file.
//!
//! # How the bytes are obtained
//!
//! `git_patch_print` is driven with a callback, not `git_patch_to_buf`, and the callback
//! keeps each piece separately. That gives the file header exactly as libgit2 renders it
//! (`diff --git`, `old mode`/`new mode`, `similarity index`, `rename from`/`rename to`,
//! `index`, `---`/`+++`) without this crate ever formatting a header itself. Verified
//! against libgit2 1.9.6's `diff_print.c`: `flush_file_header` delivers the whole header as
//! one `GIT_DIFF_LINE_FILE_HDR` line, and `git_diff_print_callback__to_buf` writes an origin
//! character only for context, addition and deletion — which is exactly the rule
//! [`RawHunk::render`] follows.

use std::path::Path;

use cide_ipc::git::{
    DiffHunkView, DiffLineView, DiffSide, FileDiff, FileState, GitError, LineOrigin,
    PartialRefusal, PathSelection,
};
use git2::{Delta, Diff, DiffLineType, DiffOptions, Oid, Patch, Repository};

use crate::{Result, Wrap};

/// One line of a hunk, ready to be written back out unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLine {
    /// `b' '`, `b'+'` or `b'-'`.
    pub origin: u8,
    /// The line's bytes as libgit2 produced them. Ends with `\n` except for the last line of
    /// a file that has no trailing newline.
    pub content: Vec<u8>,
    /// The marker libgit2 emitted straight after this line, verbatim — in practice
    /// `"\n\\ No newline at end of file\n"`, which is `xdl_emit_diffrec`'s third buffer.
    ///
    /// Stored rather than reconstructed so that a libgit2 that words it differently still
    /// round-trips. Kept *with* its line so that dropping the line drops the marker, which
    /// is the only correct coupling.
    pub eofnl: Option<Vec<u8>>,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

impl RawLine {
    pub fn is_change(&self) -> bool {
        self.origin != b' '
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The `@@ -a,b +c,d @@` line including any trailing section heading and its newline.
    pub header: Vec<u8>,
    pub lines: Vec<RawLine>,
}

impl RawHunk {
    /// The part of the header after the closing `@@` — git's "section heading", usually the
    /// enclosing function. Preserved when a header is recomputed because it is the only
    /// context a reviewer gets on a rewritten hunk.
    pub fn section(&self) -> &[u8] {
        // `@@ -1,2 +1,3 @@ fn main() {` — find the second `@@` and take everything after it.
        let Some(first) = find(&self.header, b"@@") else {
            return b"";
        };
        let rest = &self.header[first + 2..];
        match find(rest, b"@@") {
            Some(second) => strip_newline(&rest[second + 2..]),
            None => b"",
        }
    }

    /// Append this hunk, unchanged, to `out`.
    pub fn render(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.header);
        for line in &self.lines {
            out.push(line.origin);
            out.extend_from_slice(&line.content);
            if let Some(marker) = &line.eofnl {
                out.extend_from_slice(marker);
            }
        }
    }
}

/// One file's diff, in libgit2's own bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFile {
    pub path: String,
    pub old_path: Option<String>,
    /// `diff --git ...` through `+++ b/...`, exactly as libgit2 rendered it.
    pub header: Vec<u8>,
    /// The `GIT binary patch` block, when the diff was built with `show_binary`.
    pub binary_body: Option<Vec<u8>>,
    pub status: Delta,
    pub binary: bool,
    pub old_mode: u32,
    pub new_mode: u32,
    pub hunks: Vec<RawHunk>,
}

impl RawFile {
    /// The full patch for this file, byte-identical to `git_patch_to_buf`.
    pub fn render(&self) -> Vec<u8> {
        let mut out = self.header.clone();
        if let Some(body) = &self.binary_body {
            out.extend_from_slice(body);
        }
        for hunk in &self.hunks {
            hunk.render(&mut out);
        }
        out
    }

    /// Identity of this exact diff, carried on [`FileDiff::rev`] and checked before a
    /// selection is applied.
    pub fn rev(&self) -> String {
        blake3::hash(&self.render()).to_hex()[..16].to_string()
    }

    /// Why this file cannot be partially staged, if it cannot.
    ///
    /// This list is the entire safety argument for patch synthesis: everything on it is a
    /// case where a hand-built unified diff is silently wrong, so none of them is ever
    /// described by a patch this crate wrote.
    pub fn partial_refusal(&self) -> Option<PartialRefusal> {
        if self.binary {
            return Some(PartialRefusal::Binary);
        }
        if self.old_mode == u32::from(git2::FileMode::Commit)
            || self.new_mode == u32::from(git2::FileMode::Commit)
        {
            return Some(PartialRefusal::Submodule);
        }
        if self.old_mode == u32::from(git2::FileMode::Link)
            || self.new_mode == u32::from(git2::FileMode::Link)
            || self.status == Delta::Typechange
        {
            return Some(PartialRefusal::TypeChange);
        }
        if self.status == Delta::Deleted {
            // A header that says `deleted file mode` cannot also keep lines.
            return Some(PartialRefusal::Deletion);
        }
        if matches!(self.status, Delta::Renamed | Delta::Copied) {
            // `rename from`/`rename to` and the content hunks are one indivisible delta.
            return Some(PartialRefusal::Rename);
        }
        None
    }

    /// Total selectable lines — additions and deletions across every hunk.
    pub fn change_count(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| l.is_change())
            .count()
    }
}

/// Refuse a selection whose diff has moved since it was made.
///
/// Every operation that turns a [`PathSelection`] into positions has to ask this first: a
/// `Hunks` or `Lines` selection is *nothing but* positions in one particular diff, so applying
/// it to a diff taken later names different lines — silently, and with no later step that
/// would notice.
///
/// It lives here, beside [`RawFile::rev`], because it was previously written out three times
/// and one of the three was missing: `stage::resolve` had it, `commit::rebuild_index` had it,
/// and `shelf::shelve` had none — so a stale selection got as far as a patch file on disk and
/// a catalogue entry before `stage::rollback` refused it at the end, leaving a shelf entry
/// full of lines the user never picked behind a failed operation. A shared function is the
/// fix that also stops the *next* caller forgetting by omission.
///
/// `rev: None` skips the check, which is only correct for [`Selection::Whole`] — it names no
/// positions, so there is nothing for the file to have moved out from under.
pub fn check_rev(selection: &PathSelection, file: &RawFile) -> Result<()> {
    if let Some(expected) = &selection.rev
        && &file.rev() != expected
    {
        return Err(GitError::StaleSelection {
            path: selection.path.clone(),
        });
    }
    Ok(())
}

/// Which pair of trees to diff, and how much content to include.
#[derive(Debug, Clone, Copy)]
pub struct DiffRequest {
    pub side: DiffSide,
    /// Emit `GIT binary patch` blocks. Wanted by the shelf (a patch has to restore a binary
    /// file) and never by staging (a binary file is staged whole through the index).
    pub binary: bool,
    pub context: u32,
    /// Detect renames. Off for staging — a rename delta is refused there anyway and finding
    /// them costs a similarity pass over every added and deleted blob.
    pub renames: bool,
}

impl DiffRequest {
    pub fn new(side: DiffSide) -> Self {
        Self {
            side,
            binary: false,
            // git's own default, and what every diff the user has ever read uses.
            context: 3,
            renames: false,
        }
    }

    pub fn binary(mut self, yes: bool) -> Self {
        self.binary = yes;
        self
    }

    pub fn renames(mut self, yes: bool) -> Self {
        self.renames = yes;
        self
    }
}

fn options(request: &DiffRequest, pathspec: Option<&str>) -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.context_lines(request.context)
        .include_typechange(true)
        .show_binary(request.binary);
    if request.side != DiffSide::Staged {
        // Untracked files have to be diffable or a new file could never be staged by hunk.
        // `show_untracked_content` is what turns an untracked entry from a bare delta into
        // one with lines; without it the panel offers hunks that do not exist.
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
    }
    if let Some(spec) = pathspec {
        opts.pathspec(spec);
        // A pathspec that matches nothing should produce an empty diff, not every file.
        opts.disable_pathspec_match(true);
    }
    opts
}

/// Build the diff for one side of the index.
pub fn build<'r>(
    repo: &'r Repository,
    request: DiffRequest,
    pathspec: Option<&str>,
) -> Result<Diff<'r>> {
    let mut opts = options(&request, pathspec);
    let head_tree = head_tree(repo)?;

    let mut diff = match request.side {
        DiffSide::Staged => {
            let index = repo.index().wrap()?;
            repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut opts))
                .wrap()?
        }
        DiffSide::Unstaged => repo.diff_index_to_workdir(None, Some(&mut opts)).wrap()?,
        DiffSide::Combined => repo
            .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
            .wrap()?,
    };

    if request.renames {
        let mut find = git2::DiffFindOptions::new();
        find.renames(true).copies(false).for_untracked(false);
        diff.find_similar(Some(&mut find)).wrap()?;
    }
    Ok(diff)
}

/// `HEAD^{tree}`, or `None` on an unborn branch.
pub fn head_tree(repo: &Repository) -> Result<Option<git2::Tree<'_>>> {
    match repo.head() {
        Ok(head) => {
            let tree = head.peel_to_tree().wrap()?;
            Ok(Some(tree))
        }
        // An unborn HEAD is a repository with no commits, which is a normal state and not an
        // error: everything in it is an addition against the empty tree.
        Err(e)
            if e.code() == git2::ErrorCode::UnbornBranch
                || e.code() == git2::ErrorCode::NotFound =>
        {
            Ok(None)
        }
        Err(e) => Err(e).wrap(),
    }
}

/// Pull every delta out of a diff in raw form.
pub fn raw_files(diff: &Diff<'_>) -> Result<Vec<RawFile>> {
    let mut out = Vec::with_capacity(diff.deltas().len());
    for index in 0..diff.deltas().len() {
        if let Some(file) = raw_file_at(diff, index)? {
            out.push(file);
        }
    }
    Ok(out)
}

/// One delta in raw form. `None` when libgit2 has no patch for it (an unmodified entry).
pub fn raw_file_at(diff: &Diff<'_>, index: usize) -> Result<Option<RawFile>> {
    let Some(delta) = diff.get_delta(index) else {
        return Ok(None);
    };
    let Some(mut patch) = Patch::from_diff(diff, index).wrap()? else {
        return Ok(None);
    };

    let mut file = RawFile {
        path: path_of(&delta.new_file())
            .or_else(|| path_of(&delta.old_file()))
            .unwrap_or_default(),
        old_path: path_of(&delta.old_file())
            .filter(|p| Some(p.as_str()) != path_of(&delta.new_file()).as_deref()),
        header: Vec::new(),
        binary_body: None,
        status: delta.status(),
        binary: delta.flags().is_binary(),
        old_mode: u32::from(delta.old_file().mode()),
        new_mode: u32::from(delta.new_file().mode()),
        hunks: Vec::new(),
    };

    // One pass over libgit2's own printer. Each callback carries exactly the bytes the
    // printer would have written, so nothing here formats anything.
    let mut error: Option<GitError> = None;
    let print = patch.print(&mut |_delta, hunk, line| {
        match line.origin_value() {
            DiffLineType::FileHeader => file.header.extend_from_slice(line.content()),
            DiffLineType::Binary => {
                file.binary_body
                    .get_or_insert_with(Vec::new)
                    .extend_from_slice(line.content());
            }
            DiffLineType::HunkHeader => {
                let Some(hunk) = hunk else {
                    error = Some(GitError::Git {
                        detail: "libgit2 emitted a hunk header with no hunk".into(),
                    });
                    return false;
                };
                file.hunks.push(RawHunk {
                    old_start: hunk.old_start(),
                    old_lines: hunk.old_lines(),
                    new_start: hunk.new_start(),
                    new_lines: hunk.new_lines(),
                    header: line.content().to_vec(),
                    lines: Vec::new(),
                });
            }
            DiffLineType::Context | DiffLineType::Addition | DiffLineType::Deletion => {
                let Some(current) = file.hunks.last_mut() else {
                    error = Some(GitError::Git {
                        detail: "libgit2 emitted a diff line outside any hunk".into(),
                    });
                    return false;
                };
                current.lines.push(RawLine {
                    origin: line.origin() as u8,
                    content: line.content().to_vec(),
                    eofnl: None,
                    old_lineno: line.old_lineno(),
                    new_lineno: line.new_lineno(),
                });
            }
            // Always delivered immediately after the line it annotates — see
            // `git_xdiff_cb`, where it is the third buffer of the same xdiff callback.
            DiffLineType::ContextEOFNL | DiffLineType::AddEOFNL | DiffLineType::DeleteEOFNL => {
                let Some(previous) = file.hunks.last_mut().and_then(|h| h.lines.last_mut()) else {
                    error = Some(GitError::Git {
                        detail: "libgit2 emitted a no-newline marker before any line".into(),
                    });
                    return false;
                };
                previous.eofnl = Some(line.content().to_vec());
            }
        }
        true
    });

    if let Some(error) = error {
        return Err(error);
    }
    print.wrap()?;
    Ok(Some(file))
}

/// The diff of exactly one path, or `None` when it has no changes on that side.
pub fn file_diff(repo: &Repository, path: &str, request: DiffRequest) -> Result<Option<RawFile>> {
    let diff = build(repo, request, Some(path))?;
    Ok(raw_files(&diff)?.into_iter().find(|f| f.path == path))
}

/// The frontend's view of one file's hunks, on their own.
///
/// Split out of [`view`] so the revision surface can share the mapping. [`FileDiff`] wraps
/// these in `side`, `rev` and `partial_ok`, and all three are meaningless for a diff of two
/// committed trees: there is no working-tree side to name, nothing for the `rev` check to
/// refuse a stale [`cide_ipc::git::Selection`] against, and no staging operation the answer
/// could ever feed. So the revision path wants the hunks and none of the wrapper.
///
/// Extracted rather than copied, and the difference matters: the `LineOrigin` translation and
/// the [`strip_newline`] rule are the two places a second copy would drift, and it would drift
/// on the read-only surface — the one nobody is staging from, so the one where a wrong line
/// number produces no visible failure until somebody trusts it.
pub fn hunk_views(file: &RawFile) -> Vec<DiffHunkView> {
    file.hunks
        .iter()
        .enumerate()
        .map(|(index, hunk)| DiffHunkView {
            index: index as u32,
            header: String::from_utf8_lossy(strip_newline(&hunk.header)).into_owned(),
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            lines: hunk
                .lines
                .iter()
                .map(|line| DiffLineView {
                    origin: match line.origin {
                        b'+' => LineOrigin::Addition,
                        b'-' => LineOrigin::Deletion,
                        _ => LineOrigin::Context,
                    },
                    content: String::from_utf8_lossy(strip_newline(&line.content)).into_owned(),
                    old_lineno: line.old_lineno,
                    new_lineno: line.new_lineno,
                    no_newline: line.eofnl.is_some(),
                })
                .collect(),
        })
        .collect()
}

/// The frontend's view of a file diff.
pub fn view(file: &RawFile, side: DiffSide) -> FileDiff {
    FileDiff {
        path: file.path.clone(),
        old_path: file.old_path.clone(),
        side,
        status: state_of(file.status),
        binary: file.binary,
        old_mode: file.old_mode,
        new_mode: file.new_mode,
        hunks: hunk_views(file),
        rev: file.rev(),
        partial_ok: file.partial_refusal().is_none(),
    }
}

pub fn state_of(delta: Delta) -> FileState {
    match delta {
        Delta::Unmodified | Delta::Unreadable => FileState::Unmodified,
        Delta::Added => FileState::Added,
        Delta::Deleted => FileState::Deleted,
        Delta::Modified => FileState::Modified,
        Delta::Renamed => FileState::Renamed,
        Delta::Copied => FileState::Copied,
        Delta::Ignored => FileState::Ignored,
        Delta::Untracked => FileState::Untracked,
        Delta::Typechange => FileState::TypeChange,
        Delta::Conflicted => FileState::Conflicted,
    }
}

fn path_of(file: &git2::DiffFile<'_>) -> Option<String> {
    file.path_bytes()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Blob id of `path` in the index, for callers that need the pre-image.
pub fn index_blob(repo: &Repository, path: &str) -> Result<Option<Oid>> {
    let index = repo.index().wrap()?;
    Ok(index
        .get_path(Path::new(path), 0)
        .map(|entry| entry.id)
        .filter(|oid| !oid.is_zero()))
}

pub(crate) fn strip_newline(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(header: &str) -> RawHunk {
        RawHunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            header: header.as_bytes().to_vec(),
            lines: Vec::new(),
        }
    }

    #[test]
    fn section_heading_is_everything_after_the_second_at_at() {
        assert_eq!(
            hunk("@@ -1,2 +1,3 @@ fn main() {\n").section(),
            b" fn main() {"
        );
        assert_eq!(hunk("@@ -1,2 +1,3 @@\n").section(), b"");
    }

    #[test]
    fn strip_newline_handles_crlf_and_bare_lines() {
        assert_eq!(strip_newline(b"abc\n"), b"abc");
        assert_eq!(strip_newline(b"abc\r\n"), b"abc");
        assert_eq!(strip_newline(b"abc"), b"abc");
    }
}
