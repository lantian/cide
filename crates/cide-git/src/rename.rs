//! Resolve a single rename row into the paths each operation must change.
//!
//! Status, rather than a pathspec-filtered diff, owns the pairing. Filtering to the
//! destination alone loses the source before libgit2 can detect the rename.

use std::collections::BTreeSet;
use std::path::Path;

use cide_ipc::git::{DiffSide, GitError, PartialRefusal, PathSelection, Selection};
use git2::{Repository, Status};

use crate::{Result, Wrap, diff, status};

pub(crate) struct Rename {
    pub head: Option<String>,
    pub index: Option<String>,
    pub worktree: Option<String>,
    pub current: String,
    index_renamed: bool,
    worktree_renamed: bool,
}

impl Rename {
    pub fn endpoints(&self, side: DiffSide) -> Option<(Option<&str>, &str)> {
        match side {
            DiffSide::Staged if self.index_renamed => {
                Some((self.head.as_deref(), self.index.as_deref()?))
            }
            DiffSide::Unstaged if self.worktree_renamed => {
                Some((self.index.as_deref(), self.worktree.as_deref()?))
            }
            DiffSide::Combined => Some((self.head.as_deref(), &self.current)),
            _ => None,
        }
    }

    fn names(&self, path: &str) -> bool {
        self.current == path || (self.index_renamed && self.index.as_deref() == Some(path))
    }
}

pub(crate) fn collect(repo: &Repository) -> Result<Vec<Rename>> {
    let mut options = status::options(false);
    let statuses = repo.statuses(Some(&mut options)).wrap()?;
    let mut renames = Vec::new();
    for entry in statuses.iter() {
        let flags = entry.status();
        if !flags.intersects(Status::INDEX_RENAMED | Status::WT_RENAMED) {
            continue;
        }
        let staged = entry.head_to_index();
        let unstaged = entry.index_to_workdir();
        let head = staged
            .as_ref()
            .and_then(|d| d.old_file().path())
            .or_else(|| unstaged.as_ref().and_then(|d| d.old_file().path()));
        let index = staged
            .as_ref()
            .and_then(|d| d.new_file().path())
            .or_else(|| unstaged.as_ref().and_then(|d| d.old_file().path()));
        let worktree = unstaged
            .as_ref()
            .and_then(|d| d.new_file().path())
            .or_else(|| staged.as_ref().and_then(|d| d.new_file().path()));
        let current = worktree.or(index).or(head);
        let owned = |path: Option<&Path>| -> Result<Option<String>> {
            path.map(|p| {
                p.to_str().map(str::to_owned).ok_or_else(|| GitError::Git {
                    detail: "a rename contains a non-UTF-8 path".into(),
                })
            })
            .transpose()
        };
        renames.push(Rename {
            current: owned(current)?.expect("a rename has a path"),
            head: if flags.is_index_new() {
                None
            } else {
                owned(head)?
            },
            index: if flags.is_index_deleted() {
                None
            } else {
                owned(index)?
            },
            worktree: if flags.is_wt_deleted() {
                None
            } else {
                owned(worktree)?
            },
            index_renamed: flags.is_index_renamed(),
            worktree_renamed: flags.is_wt_renamed(),
        });
    }
    Ok(renames)
}

pub(crate) fn endpoints(
    repo: &Repository,
    path: &str,
    side: DiffSide,
) -> Result<Option<(Option<String>, String)>> {
    Ok(collect(repo)?
        .iter()
        .find(|r| r.names(path))
        .and_then(|r| r.endpoints(side))
        .map(|(old, new)| (old.map(str::to_owned), new.to_owned())))
}

pub(crate) struct Expanded {
    pub selections: Vec<PathSelection>,
    /// Rename sources are deletions even if a new, untracked file now occupies that name.
    pub removals: BTreeSet<String>,
    /// Includes intermediate index names in a staged A→B, unstaged B→C rename.
    pub affected: BTreeSet<String>,
}

impl Expanded {
    pub fn check_restore_sources(&self, repo: &Repository) -> Result<()> {
        for path in &self.removals {
            if repo
                .workdir()
                .is_some_and(|root| std::fs::symlink_metadata(root.join(path)).is_ok())
            {
                return Err(GitError::Git {
                    detail: format!(
                        "cannot restore rename source {path}: another file now occupies that path"
                    ),
                });
            }
        }
        Ok(())
    }
}

pub(crate) fn expand(
    repo: &Repository,
    selections: &[PathSelection],
    side: DiffSide,
) -> Result<Expanded> {
    let mut out = Expanded {
        selections: Vec::new(),
        removals: BTreeSet::new(),
        affected: BTreeSet::new(),
    };
    if selections.is_empty() {
        return Ok(out);
    }
    let renames = collect(repo)?;
    let explicit: BTreeSet<_> = selections.iter().map(|s| s.path.as_str()).collect();
    let mut seen = BTreeSet::new();
    for selection in selections {
        let rename = renames.iter().find(|r| r.names(&selection.path));
        let Some((old, new)) = rename.and_then(|r| r.endpoints(side)) else {
            if seen.insert(selection.path.clone()) {
                out.selections.push(selection.clone());
            }
            continue;
        };
        if selection.rev.is_some() {
            let file = diff::file_diff(
                repo,
                &selection.path,
                diff::DiffRequest::new(side).renames(true),
            )?
            .ok_or_else(|| GitError::StaleSelection {
                path: selection.path.clone(),
            })?;
            diff::check_rev(selection, &file)?;
        }
        if !matches!(selection.selection, Selection::Whole) {
            return Err(GitError::PartialRefused {
                path: selection.path.clone(),
                reason: PartialRefusal::Rename,
            });
        }
        let rename = rename.expect("endpoints came from this rename");
        if side == DiffSide::Combined {
            out.affected.extend(
                rename
                    .head
                    .iter()
                    .chain(&rename.index)
                    .chain(&rename.worktree)
                    .cloned(),
            );
        } else {
            out.affected.insert(new.to_owned());
            out.affected.extend(old.map(str::to_owned));
        }
        if let Some(old) = old.filter(|old| *old != new && !explicit.contains(old)) {
            out.removals.insert(old.to_owned());
            if seen.insert(old.to_owned()) {
                out.selections.push(PathSelection::whole(old));
            }
        }
        // A renamed index entry can have been deleted from disk. Combined then only
        // changes the original HEAD path; the intermediate entry is consumed, not restored.
        if (side != DiffSide::Combined || rename.worktree.is_some()) && seen.insert(new.to_owned())
        {
            out.selections.push(PathSelection::whole(new));
        }
    }
    Ok(out)
}
