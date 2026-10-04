//! Read the changes a commit message should describe, without staging anything.

use std::path::Path;

use cide_ipc::git::{DiffSide, GitError, PathSelection, Selection};

use crate::{Result, changelist, diff, patch, rename, repo};

/// Normal commits describe precisely the selected working-tree changes. Staging-area
/// commits and merge/cherry-pick/revert conclusions record the index as it stands.
pub fn commit_diff(root: &Path, selections: &[PathSelection]) -> Result<String> {
    let repo = repo::open(root)?;
    let conflicts = repo::conflicted_paths(&repo)?;
    if !conflicts.is_empty() {
        return Err(GitError::Conflicted { paths: conflicts });
    }
    let staged = changelist::load(root).use_staging_area
        || matches!(
            repo::operation_in_progress(&repo).as_deref(),
            Some("merge" | "cherry-pick" | "revert")
        );
    let mut text = String::new();
    if staged {
        let changes = diff::build(&repo, diff::DiffRequest::new(DiffSide::Staged), None)?;
        for file in diff::raw_files(&changes)? {
            text.push_str(&String::from_utf8_lossy(&file.render()));
        }
    } else {
        let expanded = rename::expand(&repo, selections, DiffSide::Combined)?;
        for selection in &expanded.selections {
            if expanded.removals.contains(&selection.path) {
                text.push_str(&String::from_utf8_lossy(
                    &diff::head_deletion(&repo, &selection.path, false)?.render(),
                ));
                continue;
            }
            let Some(file) = diff::file_diff(
                &repo,
                &selection.path,
                diff::DiffRequest::new(DiffSide::Combined),
            )?
            else {
                return Err(GitError::NoSuchChange {
                    path: selection.path.clone(),
                });
            };
            diff::check_rev(selection, &file)?;
            let bytes =
                if matches!(selection.selection, Selection::Whole) || file.change_count() == 0 {
                    file.render()
                } else {
                    let chosen = patch::choose(&file, &selection.selection)?;
                    if chosen.covers_everything {
                        file.render()
                    } else {
                        patch::synthesize(&file, &chosen.lines)?.unwrap_or_default()
                    }
                };
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    Ok(text)
}
