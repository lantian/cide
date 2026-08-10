//! The shelf: our own patch files, kept deliberately separate from `git stash`.
//!
//! IDEA has both and they are different features, so cide has both. A stash is a git object
//! on a ref, it takes the whole working tree, and `git stash pop` elsewhere can find it. A
//! shelf entry is a patch file under `$XDG_STATE_HOME/cide/repos/<blake3(root)>/shelf/`, it
//! can hold *part* of the working tree — one changelist, one file, three hunks — and it does
//! not touch the repository at all.
//!
//! Patches are generated with `show_binary`, so a shelved binary file restores. That is the
//! one place in this crate where a patch we wrote describes a binary, and it is safe because
//! it is libgit2's own `GIT binary patch` block copied verbatim out of
//! `git_patch_print` — this crate does not synthesize it.

use std::path::Path;

use cide_ipc::git::{DiffSide, GitError, PathSelection, ShelfEntry};
use git2::{ApplyLocation, Diff};
use serde::{Deserialize, Serialize};

use crate::diff::{DiffRequest, RawFile};
use crate::{Result, diff, repo as repo_mod, sidecar, stage};

/// `shelf/index.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalogue {
    #[serde(default)]
    entries: Vec<ShelfEntry>,
}

fn catalogue_path(root: &Path) -> std::path::PathBuf {
    sidecar::shelf_dir(root).join("index.json")
}

fn patch_path(root: &Path, id: &str) -> std::path::PathBuf {
    sidecar::shelf_dir(root).join(format!("{id}.patch"))
}

fn load(root: &Path) -> Catalogue {
    let path = catalogue_path(root);
    match sidecar::read_opt(&path) {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), %error, "shelf catalogue unreadable; treating the shelf as empty");
            Catalogue::default()
        }),
        _ => Catalogue::default(),
    }
}

fn save(root: &Path, catalogue: &Catalogue) -> Result<()> {
    let path = catalogue_path(root);
    let json = serde_json::to_vec_pretty(catalogue).map_err(|e| sidecar::sidecar_err(&path, e))?;
    sidecar::write_atomic(&path, &json)
}

pub fn list(root: &Path) -> Vec<ShelfEntry> {
    let mut entries = load(root).entries;
    // Newest first — the shelf is a stack in every UI that has one.
    entries.sort_by(|a, b| b.created.cmp(&a.created));
    entries
}

/// Write the selected changes to a patch file and take them out of the working tree.
///
/// The rollback happens **after** the patch is on disk and fsynced, so a crash between the
/// two leaves the user with both copies rather than neither.
pub fn shelve(root: &Path, name: &str, selections: &[PathSelection]) -> Result<ShelfEntry> {
    let repo = repo_mod::open(root)?;
    let mut text = Vec::new();
    let mut files = Vec::new();

    for selection in selections {
        // `renames(true)` is inert in this combination: `file_diff` passes a pathspec, and
        // libgit2 filters the rename's other side out before `find_similar` runs. A shelved
        // rename is therefore written as a whole-new-file patch. Kept rather than deleted
        // because it states the intent, and the day `file_diff` widens its diff it starts
        // working — at which point the rollback below has to grow the old path too, or the
        // entry restores a rename whose source is already gone. See
        // `tests/patch_props.rs::a_rename_is_only_detected_when_both_sides_are_in_the_diff`.
        let request = DiffRequest::new(DiffSide::Combined)
            .binary(true)
            .renames(true);
        let Some(file) = diff::file_diff(&repo, &selection.path, request)? else {
            return Err(GitError::NoSuchChange {
                path: selection.path.clone(),
            });
        };
        // Before a byte is synthesized, not after. `rollback` at the end of this function
        // checks the same thing, and relying on that was wrong twice over: the patch file and
        // the catalogue entry are already written and fsynced by then, so a moved file left a
        // shelf entry full of lines the user never picked sitting behind a failed shelve. The
        // positional selections that make this reachable arrive from the diff pane, which
        // holds only `Combined` selections — the same side this diff is taken on, so the revs
        // are comparable.
        diff::check_rev(selection, &file)?;
        text.extend_from_slice(&render(&file, selection)?);
        files.push(file.path.clone());
    }

    if text.is_empty() {
        return Err(GitError::NothingToCommit);
    }

    let created = now();
    let id = format!("{created:x}-{}", &blake3::hash(&text).to_hex()[..8]);
    sidecar::write_atomic(&patch_path(root, &id), &text)?;

    let entry = ShelfEntry {
        id: id.clone(),
        name: name.to_string(),
        created,
        files,
    };
    let mut catalogue = load(root);
    catalogue.entries.push(entry.clone());
    save(root, &catalogue)?;

    stage::rollback(root, selections)?;
    Ok(entry)
}

/// The patch text for one selection: the whole file, or only the chosen hunks.
fn render(file: &RawFile, selection: &PathSelection) -> Result<Vec<u8>> {
    if matches!(selection.selection, cide_ipc::git::Selection::Whole) || file.change_count() == 0 {
        return Ok(file.render());
    }
    let chosen = crate::patch::choose(file, &selection.selection)?;
    if chosen.covers_everything {
        return Ok(file.render());
    }
    Ok(crate::patch::synthesize(file, &chosen.lines)?.unwrap_or_default())
}

/// Put a shelved change back into the working tree.
///
/// `keep` leaves the patch on the shelf, which is IDEA's "unshelve and keep" — useful for
/// applying the same change to two branches.
pub fn unshelve(root: &Path, id: &str, keep: bool) -> Result<()> {
    let repo = repo_mod::open(root)?;
    let path = patch_path(root, id);
    let Some(text) = sidecar::read_opt(&path)? else {
        return Err(GitError::NoSuchShelf { id: id.to_string() });
    };

    let parsed = Diff::from_buffer(&text).map_err(|e| GitError::PatchRejected {
        detail: e.message().to_string(),
        patch: String::from_utf8_lossy(&text).into_owned(),
    })?;
    repo.apply(&parsed, ApplyLocation::WorkDir, None)
        .map_err(|e| GitError::PatchRejected {
            detail: format!("{:?}: {}", e.class(), e.message()),
            patch: String::from_utf8_lossy(&text).into_owned(),
        })?;

    if !keep {
        drop_entry(root, id)?;
    }
    Ok(())
}

/// Remove a shelved change without applying it.
pub fn drop_entry(root: &Path, id: &str) -> Result<()> {
    let mut catalogue = load(root);
    let before = catalogue.entries.len();
    catalogue.entries.retain(|e| e.id != id);
    if catalogue.entries.len() == before {
        return Err(GitError::NoSuchShelf { id: id.to_string() });
    }
    save(root, &catalogue)?;
    sidecar::remove(&patch_path(root, id))
}

/// The raw patch text, for a preview pane.
pub fn read_patch(root: &Path, id: &str) -> Result<String> {
    let Some(text) = sidecar::read_opt(&patch_path(root, id))? else {
        return Err(GitError::NoSuchShelf { id: id.to_string() });
    };
    Ok(String::from_utf8_lossy(&text).into_owned())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
