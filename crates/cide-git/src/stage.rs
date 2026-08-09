//! Staging, unstaging and rollback.
//!
//! # Two mechanisms, chosen per file
//!
//! * **Whole files go through the index API.** `add_path` / `remove_path` /
//!   `reset_default` are what `git add` and `git reset` themselves call, so binaries, mode
//!   bits, symlinks, submodule gitlinks, deletions and renames are exact by construction and
//!   no patch we wrote is involved.
//! * **Partial selections go through [`crate::patch::synthesize`]** and
//!   `Repository::apply(ApplyLocation::Index)`.
//!
//! A partial selection that happens to name every change is normalised to the whole-file
//! path before anything else looks at it, so the ordinary "tick the row" gesture never
//! touches patch synthesis.
//!
//! # Unstage and rollback never reverse a patch
//!
//! Both are expressed as *rebuild the file from its pre-image plus the changes being kept*:
//! reset the path to HEAD, then apply a forward patch of the changes the user did **not**
//! select. Reversing a unified diff means rewriting `new file mode` into `deleted file
//! mode`, swapping the `index` line and the `---`/`+++` paths and getting every one of them
//! right, on the code path where a mistake destroys work. Not doing it at all is a smaller
//! thing to get right, and it reuses the synthesizer the property tests already cover.

use std::path::Path;

use cide_ipc::git::{DiffSide, GitError, PathSelection, Selection};
use git2::{ApplyLocation, Diff, Repository, build::CheckoutBuilder};

use crate::diff::{DiffRequest, RawFile};
use crate::{Result, Wrap, changelist, diff, patch, repo as repo_mod};

/// One file's resolved plan.
enum Plan {
    /// Take the working tree's whole file, or drop it if it is gone.
    Whole(String),
    /// A synthesized unified diff.
    Partial(Vec<u8>),
}

/// Fetch the diff a selection refers to and check it has not moved underneath it.
fn resolve(repo: &Repository, selection: &PathSelection, side: DiffSide) -> Result<RawFile> {
    let request = DiffRequest::new(side);
    let file =
        diff::file_diff(repo, &selection.path, request)?.ok_or_else(|| GitError::NoSuchChange {
            path: selection.path.clone(),
        })?;
    if let Some(expected) = &selection.rev
        && &file.rev() != expected
    {
        // The panel checked boxes against a diff that no longer exists — Claude edited the
        // file mid-gesture, say. Applying the old positions to the new diff stages
        // *different* lines, which is a silent wrong answer, so it is refused instead.
        return Err(GitError::StaleSelection {
            path: selection.path.clone(),
        });
    }
    Ok(file)
}

/// Whether the selection covers the entire file, one way or another.
fn is_whole(file: &RawFile, selection: &Selection) -> Result<bool> {
    if matches!(selection, Selection::Whole) {
        return Ok(true);
    }
    // A file with no hunks at all — a pure mode change, a binary, a submodule bump — has
    // nothing to select, so any selection of it is the whole thing.
    if file.change_count() == 0 {
        return Ok(true);
    }
    Ok(patch::choose(file, selection)?.covers_everything)
}

// --- stage --------------------------------------------------------------------------------

/// Move the selected changes from the working tree into the index.
pub fn stage(root: &Path, selections: &[PathSelection]) -> Result<()> {
    let repo = repo_mod::open(root)?;
    let mut plans = Vec::with_capacity(selections.len());

    // Everything is resolved and synthesized before a single byte is written. A failure
    // halfway through would otherwise leave the index holding some of what was asked for and
    // none of the rest, with no record of which.
    for selection in selections {
        let file = resolve(&repo, selection, DiffSide::Unstaged)?;
        if is_whole(&file, &selection.selection)? {
            plans.push(Plan::Whole(file.path.clone()));
            continue;
        }
        let chosen = patch::choose(&file, &selection.selection)?;
        match patch::synthesize(&file, &chosen.lines)? {
            Some(text) => plans.push(Plan::Partial(text)),
            None => continue,
        }
    }

    apply_plans(&repo, &plans, ApplyLocation::Index)?;
    changelist::record_index(root, &repo)
}

/// Run every plan: the synthesized patches in one `git_apply`, then the whole files.
///
/// The patch goes first because `git_apply` writes the index itself, so any in-memory index
/// edits made before it would be committed by *its* writer rather than by ours — which works
/// today and would stop working the moment libgit2 changed when it re-reads.
fn apply_plans(repo: &Repository, plans: &[Plan], location: ApplyLocation) -> Result<()> {
    let mut text = Vec::new();
    for plan in plans {
        if let Plan::Partial(patch) = plan {
            text.extend_from_slice(patch);
        }
    }
    if !text.is_empty() {
        apply(repo, &text, location)?;
    }

    let wholes: Vec<&String> = plans
        .iter()
        .filter_map(|plan| match plan {
            Plan::Whole(path) => Some(path),
            Plan::Partial(_) => None,
        })
        .collect();
    if wholes.is_empty() {
        return Ok(());
    }

    let mut index = repo.index().wrap()?;
    // Pick up whatever `git_apply` just wrote, or the whole-file adds below would be made
    // against a stale in-memory index and would drop them on write.
    index.read(true).wrap()?;
    for path in wholes {
        add_or_remove(repo, &mut index, path)?;
    }
    index.write().wrap()
}

/// Stage a whole path exactly the way `git add` does.
fn add_or_remove(repo: &Repository, index: &mut git2::Index, path: &str) -> Result<()> {
    let absolute = repo
        .workdir()
        .ok_or_else(|| GitError::Bare {
            path: repo.path().display().to_string(),
        })?
        .join(path);
    // `symlink_metadata`, not `exists`: a dangling symlink is a real, stageable entry, and
    // `exists` follows the link and calls it absent.
    if std::fs::symlink_metadata(&absolute).is_ok() {
        // `add_bypath` is the one call that handles regular files, executables, symlinks and
        // submodule gitlinks correctly, and it runs the repo's filters — so a `text=auto`
        // file lands in the index with LF exactly as `git add` would leave it.
        index.add_path(Path::new(path)).wrap()
    } else {
        index.remove_path(Path::new(path)).wrap()
    }
}

/// Escape a literal path so libgit2 matches it and nothing else.
///
/// `checkout_head` and `reset_default` both take *pathspecs*, and libgit2 runs them through
/// `fnmatch`. Every path this crate passes came out of a diff and names one exact file, so a
/// file literally called `a[1].txt` would otherwise also match — and drag `a1.txt` along.
/// Verified against the libgit2 this crate links, by `tests/staging.rs`: unescaped, a rollback
/// of `a[1].txt` overwrites `a1.txt` with its HEAD content and destroys whatever was in it.
///
/// `checkout_head` also takes `disable_pathspec_match`, which is used *as well*; `reset_default`
/// has no such flag, which is why the escaping exists rather than the flag alone.
fn escape_pathspec(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn apply(repo: &Repository, text: &[u8], location: ApplyLocation) -> Result<()> {
    let diff = Diff::from_buffer(text).map_err(|e| GitError::PatchRejected {
        detail: e.message().to_string(),
        patch: String::from_utf8_lossy(text).into_owned(),
    })?;
    repo.apply(&diff, location, None)
        .map_err(|e| GitError::PatchRejected {
            detail: format!("{:?}: {}", e.class(), e.message()),
            patch: String::from_utf8_lossy(text).into_owned(),
        })
}

// --- unstage ------------------------------------------------------------------------------

/// Take the selected changes back out of the index.
pub fn unstage(root: &Path, selections: &[PathSelection]) -> Result<()> {
    let repo = repo_mod::open(root)?;
    // `git_reset_default` peels its target to a commit, so this must be the commit and not
    // the tree that everything else in this crate works with.
    let head = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.into_object());

    let mut resets: Vec<String> = Vec::new();
    let mut keeps: Vec<Vec<u8>> = Vec::new();

    for selection in selections {
        let file = resolve(&repo, selection, DiffSide::Staged)?;
        resets.push(file.path.clone());
        if is_whole(&file, &selection.selection)? {
            continue;
        }
        let chosen = patch::choose(&file, &selection.selection)?;
        let keep = patch::complement(&file, &chosen);
        if let Some(text) = patch::synthesize(&file, &keep)? {
            keeps.push(text);
        }
    }

    // `reset_default` with a `None` target removes the entries outright, which is the right
    // answer on an unborn branch: there is no HEAD content to go back to.
    let specs: Vec<String> = resets.iter().map(|p| escape_pathspec(p)).collect();
    repo.reset_default(head.as_ref(), specs.iter().map(String::as_str))
        .wrap()?;

    for text in &keeps {
        apply(&repo, text, ApplyLocation::Index)?;
    }
    changelist::record_index(root, &repo)
}

// --- rollback -----------------------------------------------------------------------------

/// Throw the selected changes away.
///
/// **This destroys uncommitted work**, which is the point of the gesture, so it is the one
/// function here that is deliberately not clever: the file goes back to its HEAD state and
/// the changes the user chose to keep are applied on top. Nothing is inferred, nothing is
/// merged.
///
/// A path that is not in HEAD is untracked, so rolling it back means deleting it. Partially
/// rolling one back truncates it to the lines that were kept.
pub fn rollback(root: &Path, selections: &[PathSelection]) -> Result<()> {
    let repo = repo_mod::open(root)?;
    let head_tree = diff::head_tree(&repo)?;

    let mut plans: Vec<(String, bool, Option<Vec<u8>>)> = Vec::new();
    for selection in selections {
        let file = resolve(&repo, selection, DiffSide::Combined)?;
        let tracked = head_tree
            .as_ref()
            .is_some_and(|tree| tree.get_path(Path::new(&file.path)).is_ok());
        if is_whole(&file, &selection.selection)? {
            plans.push((file.path.clone(), tracked, None));
            continue;
        }
        let chosen = patch::choose(&file, &selection.selection)?;
        let keep = patch::complement(&file, &chosen);
        plans.push((file.path.clone(), tracked, patch::synthesize(&file, &keep)?));
    }

    let workdir = repo
        .workdir()
        .ok_or_else(|| GitError::Bare {
            path: repo.path().display().to_string(),
        })?
        .to_path_buf();

    let tracked: Vec<&str> = plans
        .iter()
        .filter(|(_, tracked, _)| *tracked)
        .map(|(path, _, _)| path.as_str())
        .collect();
    if !tracked.is_empty() {
        let mut checkout = CheckoutBuilder::new();
        // `disable_pathspec_match` is what makes each entry an exact file name. Without it
        // libgit2 fnmatches them, and rolling back `a[1].txt` also force-overwrites `a1.txt`
        // — someone else's uncommitted work, destroyed by a gesture that never named it.
        checkout
            .force()
            .remove_untracked(false)
            .disable_pathspec_match(true);
        for path in &tracked {
            checkout.path(path);
        }
        repo.checkout_head(Some(&mut checkout)).wrap()?;
    }

    for (path, is_tracked, _) in &plans {
        if !is_tracked {
            // Untracked: HEAD has nothing to restore, so its pre-image is "absent".
            match std::fs::remove_file(workdir.join(path)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).wrap(),
            }
        }
    }

    for (_, _, keep) in &plans {
        if let Some(text) = keep {
            apply(&repo, text, ApplyLocation::WorkDir)?;
        }
    }
    changelist::record_index(root, &repo)
}

#[cfg(test)]
mod tests {
    use super::escape_pathspec;

    #[test]
    fn pathspec_escaping_covers_every_fnmatch_metacharacter() {
        assert_eq!(escape_pathspec("src/main.rs"), "src/main.rs");
        assert_eq!(escape_pathspec("a[1].txt"), "a\\[1\\].txt");
        assert_eq!(escape_pathspec("q?x*y.txt"), "q\\?x\\*y.txt");
        // A backslash in a filename is legal on Linux and would otherwise escape the
        // character after it.
        assert_eq!(escape_pathspec("we\\ird"), "we\\\\ird");
    }
}
