//! Per-path git status for the file tree, keyed by the absolute paths `cide-fs` produces.
//!
//! # Where the join lives, and what the alternatives cost
//!
//! `cide-fs` walks the filesystem and knows nothing about git; `cide-git` reads git and knows
//! nothing about the file tree's rows. Something has to put the two together. Three shapes
//! were available:
//!
//! 1. **`TreeRow` gains a `status` field**, filled in `cide-app::files` as rows are windowed.
//!    Simplest to consume — the row arrives already tagged. It loses on two counts. It
//!    couples the fs index to git, so `cide-fs` can no longer be tested without a repository
//!    and a project on a plain directory pays for a git lookup that always answers "clean".
//!    Worse, it puts git on the critical path of `fs_tree_rows`: every scroll would wait on a
//!    status walk, and the *first* paint of the tree would wait on one too. A repository with
//!    a slow `git status` must show an untagged tree and then tags, never an empty pane.
//!
//! 2. **`git_status_for_paths(project, paths)` asked for the visible window**, the design
//!    this file's command is descended from. It keeps the crates independent, which is why it
//!    is the one chosen. Asked *per window*, though, it is two round trips per scroll tick,
//!    and each one still costs a full `git status` walk underneath: libgit2 has no per-path
//!    query that skips the walk, so windowing the question saves nothing on the Rust side and
//!    only adds latency on the wire. It also cannot answer requirement 3 — a directory is
//!    modified when something *under* it is, and the paths under a collapsed directory are
//!    exactly the ones not in the window.
//!
//! 3. **What this is**: one command, `git_tree_status`, that returns the whole per-path map
//!    at once and is re-read on invalidation rather than on scroll. This is (2)'s placement
//!    with (1)'s locality of lookup and neither one's cost. The map is bounded by the number
//!    of *changed* paths, not by the size of the repository — a 100k-file checkout with four
//!    edits ships a handful of entries — so "a 100k-file repo must never ask about 100k
//!    paths" is satisfied by never asking about paths at all.
//!
//! # Why the walk does not recurse
//!
//! `recurse_untracked_dirs(false)` and `recurse_ignored_dirs(false)` are what keep (3) true.
//! A freshly cloned-and-unbuilt repository has one untracked `target/` and one untracked
//! `src/newfeature/`; recursing would turn those into 100k entries and hand the whole
//! blow-up straight back to the frontend. Unrecursed, libgit2 reports the *directory*, and
//! the tree resolves a row inside it by looking at its nearest ancestor in the map. That
//! lookup is also the only place inheritance happens, which is why `modified` marks on
//! ancestors are never inherited downwards — see `ui/src/sidebar/treeStatus.ts`.
//!
//! # Directories
//!
//! IDEA shows a folder as changed when something under it is, and so does this. It is
//! computed here, once, by walking each changed path's *ancestors* — `O(changed × depth)` for
//! the whole map — rather than by asking each visible row about its subtree, which would be
//! `O(subtree)` per row and would make scrolling past a large `src/` quadratic.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use cide_ipc::git::{TreeStatus, TreeStatusMap};
use git2::{Status, StatusOptions};

use crate::{Wrap, repo as repo_mod};

/// The most entries one map will carry.
///
/// Reached only by a change that touches the whole tree — a `sed -i` over every file, or a
/// line-ending flip. 20k is already an order of magnitude past a reviewable changeset, and
/// the cap is what stops one bad command from putting a multi-megabyte payload on the wire
/// every time the watcher fires. Entries are collected in path order, so a truncated map is a
/// prefix: the paths it does carry are still right, which is why the tail can be shown
/// untagged rather than the whole map thrown away.
pub const MAX_ENTRIES: usize = 20_000;

/// Per-path status for every repository under `roots`, keyed by absolute path.
///
/// Never fails. A root with no repository above it contributes nothing, and a repository that
/// cannot be opened is logged and skipped — the same rule [`crate::status::changes_tree`]
/// follows, for the same reason: a project with four roots, one on an unmounted share, still
/// has three working repositories and a tree tagged from three of them beats an error.
pub fn tree_status(roots: &[PathBuf]) -> TreeStatusMap {
    let mut map = TreeStatusMap::default();
    // Keyed by work tree, because two project roots can live in one repository and the status
    // walk is the expensive part. Without this, opening `crates/` and `ui/` of one checkout as
    // two roots would walk the whole repository twice per refresh.
    let mut walked: HashMap<PathBuf, Vec<(String, TreeStatus)>> = HashMap::new();

    for root in roots {
        // The fs index spells paths as the user configured the root; git spells them from the
        // canonical work tree. Both are needed: the canonical form is what the strip below can
        // compare against, and the configured form is what a `TreeRow` will actually carry. A
        // root reached through a symlink is exactly where conflating the two shows up as a
        // tree with no tags at all and nothing anywhere saying why.
        let canonical_root = repo_mod::canonical(root);

        // Discovery is per root rather than over the whole slice: `discover` deduplicates by
        // work tree, so passing all the roots at once would attribute a shared repository's
        // entries to whichever root happened to be first and leave the others untagged.
        for info in repo_mod::discover(std::slice::from_ref(root)) {
            let entries = walked.entry(info.root.clone()).or_insert_with(|| {
                collect(&info.root).unwrap_or_else(|error| {
                    tracing::warn!(root = %info.root.display(), %error, "skipping an unreadable repository");
                    Vec::new()
                })
            });

            for (rel, status) in entries.iter() {
                // `info.root` is canonical and `rel` is repo-relative, so this is the
                // canonical absolute path. Two cases both matter and both fall out of the
                // strip: the repository *above* the root contributes entries outside it,
                // which are dropped; a submodule *below* the root contributes entries inside
                // it, which are kept.
                let absolute = info.root.join(rel);
                let Ok(inside) = absolute.strip_prefix(&canonical_root) else {
                    continue;
                };
                if !mark(&mut map.statuses, root, &root.join(inside), *status) {
                    map.truncated = true;
                    return map;
                }
            }
        }
    }
    map
}

/// Record one path's status and roll it up into its ancestors.
///
/// Returns `false` when the cap was reached, which is the caller's cue to stop and set
/// `truncated`.
///
/// The rollup is `or_insert`, never an overwrite: a directory that git named itself — an
/// untracked or ignored directory — must keep the status git gave it, or a submodule's own
/// entries would repaint the folder that contains them.
fn mark(
    into: &mut BTreeMap<String, TreeStatus>,
    root: &Path,
    path: &Path,
    status: TreeStatus,
) -> bool {
    if into.len() >= MAX_ENTRIES {
        return false;
    }
    into.insert(display(path), status);

    // An ignored directory is not a change, so it does not tint the folders above it. Every
    // other status does: a deleted file has no row of its own, and its parent turning blue is
    // the only place the tree can say that something under it went away.
    if status == TreeStatus::Ignored {
        return true;
    }

    // Up to and including the root, so that the top row of a multi-root project shows whether
    // that root has anything in it. `while let` on `parent()` alone would climb past the root
    // and out to `/`.
    let mut at = path.parent();
    while let Some(dir) = at {
        if into.len() >= MAX_ENTRIES {
            return false;
        }
        into.entry(display(dir)).or_insert(TreeStatus::Modified);
        if dir == root {
            break;
        }
        at = dir.parent();
    }
    true
}

/// The key a [`cide_ipc::TreeRow`] would carry for this path.
///
/// `to_string_lossy` is safe here in a way it is not on the walk: a key that does not round
/// trip simply fails to match any row, so the worst case is an untagged file rather than a
/// command aimed at a path the user never saw.
fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// One repository's changed paths, in path order.
fn collect(work_tree: &Path) -> crate::Result<Vec<(String, TreeStatus)>> {
    let repo = repo_mod::open(work_tree)?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        // See the module header: recursing is what turns a fresh checkout into 100k entries.
        .recurse_untracked_dirs(false)
        // Ignored *directories* only, which costs nothing — libgit2 has already decided the
        // directory is ignored in order to skip it, and this only asks it to say so instead
        // of dropping it. Recursing into one, by contrast, is a full walk of `target/`.
        .include_ignored(true)
        .recurse_ignored_dirs(false)
        .include_unmodified(false)
        // Rename detection off. It costs a content scan of every added/deleted pair, and the
        // tree cannot use the answer: a rename's old path is not on disk and so has no row,
        // while the new path is better described to a one-letter column as `A` than as a
        // rename it has no way to draw.
        .renames_head_to_index(false)
        .renames_index_to_workdir(false)
        .include_unreadable(false);

    let statuses = repo.statuses(Some(&mut opts)).wrap()?;
    let mut out = Vec::with_capacity(statuses.len());
    for entry in statuses.iter() {
        let Ok(path) = entry.path() else {
            // Same rule as the changes tree: a path libgit2 could not render as UTF-8 is
            // skipped rather than lossily named.
            continue;
        };
        // libgit2 reports an unrecursed directory with a trailing separator. The tree's rows
        // have none, and a key that does not match a row is a tag nobody sees.
        let path = path.strip_suffix('/').unwrap_or(path);
        out.push((path.to_string(), classify(entry.status())));
    }
    out.sort();
    Ok(out)
}

/// Collapse libgit2's two-sided status into the one letter the tree draws.
///
/// The order is the whole content of this function, so it is worth stating what each rung
/// buys:
///
/// * **Ignored first.** An ignored path carries no other bits worth reporting, and a tracked
///   path that merely matches an ignore pattern is not ignored as far as git is concerned.
/// * **Conflicted becomes `modified`.** A conflicted file *is* modified in the working tree,
///   and a one-glyph column is not a merge tool: conflicts are resolved in the commit panel,
///   which has real UI for them. Inventing a `!` the mock does not have would be a design
///   decision made in a status mapper.
/// * **Deleted before untracked.** `git rm --cached secrets.env` leaves the file on disk and
///   sets `INDEX_DELETED | WT_NEW`. Reading that as "untracked" is true but useless; reading
///   it as "this path is on its way out of the repository" is what the user just asked for,
///   and it is the case that makes `D` reachable on a file row at all — a path deleted from
///   the working tree has no row to tag.
/// * **Untracked before added.** `INDEX_NEW | WT_NEW` cannot occur, but `WT_NEW` alone is by
///   far the common case and putting it early keeps the cheap test first.
fn classify(status: Status) -> TreeStatus {
    if status.is_ignored() {
        TreeStatus::Ignored
    } else if status.is_conflicted() {
        TreeStatus::Modified
    } else if status.is_index_deleted() || status.is_wt_deleted() {
        TreeStatus::Deleted
    } else if status.is_wt_new() {
        TreeStatus::Untracked
    } else if status.is_index_new() {
        TreeStatus::Added
    } else {
        TreeStatus::Modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_orders_the_two_sides() {
        assert_eq!(classify(Status::IGNORED), TreeStatus::Ignored);
        assert_eq!(classify(Status::CONFLICTED), TreeStatus::Modified);
        assert_eq!(classify(Status::WT_NEW), TreeStatus::Untracked);
        assert_eq!(classify(Status::INDEX_NEW), TreeStatus::Added);
        assert_eq!(classify(Status::WT_MODIFIED), TreeStatus::Modified);
        assert_eq!(classify(Status::INDEX_MODIFIED), TreeStatus::Modified);
        assert_eq!(classify(Status::WT_TYPECHANGE), TreeStatus::Modified);
        // `git rm --cached`: still on disk, so it has a row, and the row says `D`.
        assert_eq!(
            classify(Status::INDEX_DELETED | Status::WT_NEW),
            TreeStatus::Deleted
        );
        // Staged as new and then edited is still new to HEAD.
        assert_eq!(
            classify(Status::INDEX_NEW | Status::WT_MODIFIED),
            TreeStatus::Added
        );
    }

    #[test]
    fn rollup_stops_at_the_root() {
        let root = Path::new("/w/app");
        let mut map = BTreeMap::new();
        assert!(mark(
            &mut map,
            root,
            Path::new("/w/app/src/a/b.rs"),
            TreeStatus::Modified
        ));
        assert_eq!(map.get("/w/app/src/a/b.rs"), Some(&TreeStatus::Modified));
        assert_eq!(map.get("/w/app/src/a"), Some(&TreeStatus::Modified));
        assert_eq!(map.get("/w/app/src"), Some(&TreeStatus::Modified));
        assert_eq!(map.get("/w/app"), Some(&TreeStatus::Modified));
        // Never above the root, whatever else is on that filesystem.
        assert_eq!(map.get("/w"), None);
        assert_eq!(map.get("/"), None);
    }

    #[test]
    fn an_ignored_directory_does_not_tint_its_parents() {
        let root = Path::new("/w/app");
        let mut map = BTreeMap::new();
        assert!(mark(
            &mut map,
            root,
            Path::new("/w/app/sub/build"),
            TreeStatus::Ignored
        ));
        assert_eq!(map.get("/w/app/sub/build"), Some(&TreeStatus::Ignored));
        assert_eq!(map.get("/w/app/sub"), None);
        assert_eq!(map.get("/w/app"), None);
    }

    #[test]
    fn a_named_directory_keeps_its_own_status_under_a_rollup() {
        let root = Path::new("/w/app");
        let mut map = BTreeMap::new();
        // The untracked directory arrives first, then a submodule inside it reports its own
        // change. The rollup must not repaint the folder git already named.
        mark(
            &mut map,
            root,
            Path::new("/w/app/new"),
            TreeStatus::Untracked,
        );
        mark(
            &mut map,
            root,
            Path::new("/w/app/new/sub/x.rs"),
            TreeStatus::Modified,
        );
        assert_eq!(map.get("/w/app/new"), Some(&TreeStatus::Untracked));
        assert_eq!(map.get("/w/app/new/sub"), Some(&TreeStatus::Modified));
    }

    #[test]
    fn marking_refuses_past_the_cap() {
        let root = Path::new("/w/app");
        let mut map = BTreeMap::new();
        for i in 0..MAX_ENTRIES {
            map.insert(format!("/w/app/f{i}"), TreeStatus::Modified);
        }
        assert!(!mark(
            &mut map,
            root,
            Path::new("/w/app/one-more.rs"),
            TreeStatus::Modified
        ));
    }
}
