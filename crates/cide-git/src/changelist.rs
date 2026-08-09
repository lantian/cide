//! Changelists, the index fingerprint, and the sidecar they live in.
//!
//! # Changelists are the truth
//!
//! IDEA's model, and the project's stated one: a changelist is a named set of *paths*, the
//! `.git/index` is a derived artifact, and committing a changelist rebuilds the index from
//! it. That is what makes "commit one changelist, the other is untouched" hold no matter
//! what the index happened to contain.
//!
//! Only explicit assignments are stored. A changed path nobody has filed belongs to the
//! **active** list, which is what makes a fresh repository behave exactly like plain git
//! while a user who has never opened the changelist menu never sees one. Storing every path
//! would mean the sidecar had to be rewritten on every file save.
//!
//! # The fingerprint
//!
//! Every write cide makes to the index is followed by recording a hash of the index's
//! entries here. Before every commit that hash is compared against the live index; a
//! mismatch means something outside cide staged something, and the commit reports
//! [`GitError::IndexChangedExternally`] rather than resetting over it.
//!
//! Bash panes inside cide are exactly where a user runs `git add`, so this collision is
//! likely rather than theoretical — it is the mitigation the plan attaches to the
//! changelists decision, not a nicety.

use std::collections::BTreeSet;
use std::path::Path;

use cide_ipc::git::GitError;
use git2::{Index, Repository};
use serde::{Deserialize, Serialize};

use crate::{Result, Wrap, sidecar};

/// The id of the list every unfiled change belongs to. IDEA calls it "Changes".
pub const DEFAULT_ID: &str = "default";
pub const DEFAULT_NAME: &str = "Changes";

/// One changelist as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changelist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub comment: String,
    /// Repo-relative, slash-separated. A `BTreeSet` so the file is stable across saves —
    /// a sidecar that reorders itself on every write is unreadable in a diff.
    #[serde(default)]
    pub paths: BTreeSet<String>,
}

/// `changelists.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sidecar {
    pub version: u32,
    /// The default list is always present and always first.
    pub lists: Vec<Changelist>,
    pub active: String,
    /// Hash of the index as cide last wrote it. `None` before cide has written one, which
    /// disables the guard for exactly one commit — there is nothing to compare against, and
    /// refusing the first commit in every repository would train the user to click through.
    #[serde(default)]
    pub index_fingerprint: Option<String>,
    /// IDEA's "use Git staging area instead". In this mode the index is the truth.
    #[serde(default)]
    pub use_staging_area: bool,
}

impl Sidecar {
    pub const CURRENT_VERSION: u32 = 1;

    pub fn new() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            lists: vec![Changelist {
                id: DEFAULT_ID.to_string(),
                name: DEFAULT_NAME.to_string(),
                comment: String::new(),
                paths: BTreeSet::new(),
            }],
            active: DEFAULT_ID.to_string(),
            index_fingerprint: None,
            use_staging_area: false,
        }
    }

    /// The list a path belongs to: its explicit assignment, else the active list.
    pub fn owner_of(&self, path: &str) -> &str {
        self.lists
            .iter()
            .find(|l| l.paths.contains(path))
            .map(|l| l.id.as_str())
            .unwrap_or(&self.active)
    }

    pub fn get(&self, id: &str) -> Result<&Changelist> {
        self.lists
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| GitError::NoSuchChangelist { id: id.to_string() })
    }

    fn get_mut(&mut self, id: &str) -> Result<&mut Changelist> {
        self.lists
            .iter_mut()
            .find(|l| l.id == id)
            .ok_or_else(|| GitError::NoSuchChangelist { id: id.to_string() })
    }

    /// Add a list. The id is derived from the name so a restart cannot renumber it.
    pub fn create(&mut self, name: &str, comment: &str) -> Result<String> {
        if self.lists.iter().any(|l| l.name == name) {
            return Err(GitError::DuplicateChangelist {
                name: name.to_string(),
            });
        }
        let id = slug(name, &self.lists);
        self.lists.push(Changelist {
            id: id.clone(),
            name: name.to_string(),
            comment: comment.to_string(),
            paths: BTreeSet::new(),
        });
        Ok(id)
    }

    pub fn rename(&mut self, id: &str, name: &str, comment: &str) -> Result<()> {
        if self.lists.iter().any(|l| l.name == name && l.id != id) {
            return Err(GitError::DuplicateChangelist {
                name: name.to_string(),
            });
        }
        let list = self.get_mut(id)?;
        list.name = name.to_string();
        list.comment = comment.to_string();
        Ok(())
    }

    /// Delete a list; its paths fall back to the default one.
    ///
    /// They are moved explicitly rather than left unfiled: "unfiled" means "the active
    /// list", and if the deleted list *was* active the paths would silently follow whichever
    /// list became active next.
    pub fn delete(&mut self, id: &str) -> Result<()> {
        if id == DEFAULT_ID {
            return Err(GitError::DefaultChangelist);
        }
        let Some(pos) = self.lists.iter().position(|l| l.id == id) else {
            return Err(GitError::NoSuchChangelist { id: id.to_string() });
        };
        let gone = self.lists.remove(pos);
        if self.active == id {
            self.active = DEFAULT_ID.to_string();
        }
        self.get_mut(DEFAULT_ID)?.paths.extend(gone.paths);
        Ok(())
    }

    /// Move paths into a list, removing them from wherever they were.
    ///
    /// Moving into the active list *clears* the assignment instead of recording it, so a
    /// path put back where it started stops taking up room in the sidecar.
    pub fn move_paths(&mut self, id: &str, paths: &[String]) -> Result<()> {
        self.get(id)?;
        for list in &mut self.lists {
            for path in paths {
                list.paths.remove(path);
            }
        }
        if id != self.active {
            let list = self.get_mut(id)?;
            list.paths.extend(paths.iter().cloned());
        }
        Ok(())
    }

    /// Make `id` the list that unfiled changes join.
    ///
    /// `live` is every path that currently has a change. Paths in it that were only
    /// *implicitly* in the outgoing active list are pinned to it explicitly first —
    /// otherwise switching the active list would silently drag every existing unfiled change
    /// along with it, which is not what "new changes go here" means and is a data loss the
    /// user cannot see until they commit.
    pub fn set_active(&mut self, id: &str, live: &BTreeSet<String>) -> Result<()> {
        self.get(id)?;
        if self.active == id {
            return Ok(());
        }
        let filed: BTreeSet<String> = self
            .lists
            .iter()
            .flat_map(|l| l.paths.iter().cloned())
            .collect();
        let previous = std::mem::replace(&mut self.active, id.to_string());
        let orphans: Vec<String> = live.difference(&filed).cloned().collect();
        self.get_mut(&previous)?.paths.extend(orphans);
        Ok(())
    }

    /// Drop assignments for paths that no longer have changes.
    ///
    /// Without this the sidecar grows forever: every path ever filed into a named list stays
    /// there after it is committed, and a year later the file is thousands of dead entries
    /// that a status refresh has to scan.
    ///
    /// Returns whether anything was dropped, so the caller can skip the write. A status
    /// refresh runs this several times a second while the user types, and rewriting an
    /// unchanged file that often is a disk write per keystroke for no result.
    pub fn reconcile(&mut self, live: &BTreeSet<String>) -> bool {
        let mut changed = false;
        for list in &mut self.lists {
            let before = list.paths.len();
            list.paths.retain(|p| live.contains(p));
            changed |= list.paths.len() != before;
        }
        changed
    }
}

impl Default for Sidecar {
    fn default() -> Self {
        Self::new()
    }
}

/// A stable, filesystem-safe id for a changelist name.
fn slug(name: &str, existing: &[Changelist]) -> String {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() {
        "list".to_string()
    } else {
        base
    };
    if !existing.iter().any(|l| l.id == base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !existing.iter().any(|l| &l.id == candidate))
        .unwrap_or(base)
}

/// Read the sidecar for `root`, falling back to a fresh one.
///
/// Never fails on content: a corrupt or newer-schema file costs the user their changelist
/// grouping, which is recoverable, whereas refusing to show the git panel over it is not
/// something they can act on. The bad file is left in place so it can be inspected.
pub fn load(root: &Path) -> Sidecar {
    let path = sidecar::changelists_path(root);
    let Ok(Some(bytes)) = sidecar::read_opt(&path) else {
        return Sidecar::new();
    };
    match serde_json::from_slice::<Sidecar>(&bytes) {
        Ok(s)
            if s.version == Sidecar::CURRENT_VERSION
                && s.lists.iter().any(|l| l.id == DEFAULT_ID) =>
        {
            s
        }
        Ok(s) => {
            tracing::warn!(
                path = %path.display(),
                version = s.version,
                "changelists sidecar is not a shape this build understands; ignoring it"
            );
            Sidecar::new()
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "changelists sidecar unreadable; ignoring it");
            Sidecar::new()
        }
    }
}

pub fn save(root: &Path, sidecar_data: &Sidecar) -> Result<()> {
    let path = sidecar::changelists_path(root);
    let json =
        serde_json::to_vec_pretty(sidecar_data).map_err(|e| sidecar::sidecar_err(&path, e))?;
    sidecar::write_atomic(&path, &json)
}

/// Read, mutate, write. The whole file is small and rewritten atomically, so there is no
/// partial-update path to get wrong.
pub fn update<T>(root: &Path, f: impl FnOnce(&mut Sidecar) -> Result<T>) -> Result<T> {
    let mut data = load(root);
    let out = f(&mut data)?;
    save(root, &data)?;
    Ok(out)
}

// --- the external-staging guard ----------------------------------------------------------

/// A hash of everything about the index that a commit depends on.
///
/// Entry path, blob id, mode and stage — not mtime, not the file size, not the flags. Those
/// change when git refreshes the index without changing what a commit would contain, and a
/// guard that fires on a `git status` in a pane is a guard the user turns off.
pub fn fingerprint(index: &Index) -> String {
    let mut hasher = blake3::Hasher::new();
    for entry in index.iter() {
        hasher.update(&entry.path);
        hasher.update(&[0]);
        hasher.update(entry.id.as_bytes());
        hasher.update(&entry.mode.to_le_bytes());
        hasher.update(&entry.flags.to_le_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex().to_string()
}

/// Whether the index differs from the one cide last wrote.
///
/// `false` when no fingerprint has been recorded yet: there is nothing to compare against,
/// and reporting a collision on the first commit in every repository would teach the user to
/// dismiss the bar without reading it.
pub fn index_changed_externally(root: &Path, repo: &Repository) -> Result<bool> {
    let data = load(root);
    let Some(expected) = data.index_fingerprint else {
        return Ok(false);
    };
    if data.use_staging_area {
        // The index is the user's to build in this mode; a change to it is not a collision.
        return Ok(false);
    }
    let index = repo.index().wrap()?;
    Ok(fingerprint(&index) != expected)
}

/// Fail unless the index is the one cide last wrote.
pub fn require_index_unchanged(root: &Path, repo: &Repository) -> Result<()> {
    let data = load(root);
    let Some(expected) = data.index_fingerprint else {
        return Ok(());
    };
    if data.use_staging_area {
        return Ok(());
    }
    let index = repo.index().wrap()?;
    let actual = fingerprint(&index);
    if actual != expected {
        return Err(GitError::IndexChangedExternally { expected, actual });
    }
    Ok(())
}

/// Record the index as cide's own. Called after **every** write cide makes to it.
pub fn record_index(root: &Path, repo: &Repository) -> Result<()> {
    // Re-read rather than trusting a handle the caller has been mutating: `git_index_write`
    // is what settles the on-disk entries, and hashing an in-memory copy that has not been
    // written would record a fingerprint the next process cannot reproduce.
    let mut index = repo.index().wrap()?;
    index.read(true).wrap()?;
    let hash = fingerprint(&index);
    update(root, |data| {
        data.index_fingerprint = Some(hash);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfiled_paths_belong_to_the_active_list() {
        let mut s = Sidecar::new();
        let fixes = s.create("Fixes", "").unwrap();
        assert_eq!(s.owner_of("src/main.rs"), DEFAULT_ID);
        s.move_paths(&fixes, &["src/main.rs".into()]).unwrap();
        assert_eq!(s.owner_of("src/main.rs"), fixes);
    }

    #[test]
    fn moving_back_to_the_active_list_clears_the_assignment() {
        let mut s = Sidecar::new();
        let fixes = s.create("Fixes", "").unwrap();
        s.move_paths(&fixes, &["a".into()]).unwrap();
        s.move_paths(DEFAULT_ID, &["a".into()]).unwrap();
        assert!(s.lists.iter().all(|l| l.paths.is_empty()));
        assert_eq!(s.owner_of("a"), DEFAULT_ID);
    }

    #[test]
    fn deleting_a_list_moves_its_paths_to_the_default() {
        let mut s = Sidecar::new();
        let fixes = s.create("Fixes", "").unwrap();
        s.move_paths(&fixes, &["a".into(), "b".into()]).unwrap();
        s.set_active(&fixes, &BTreeSet::new()).unwrap();
        s.delete(&fixes).unwrap();
        assert_eq!(s.active, DEFAULT_ID);
        assert_eq!(s.get(DEFAULT_ID).unwrap().paths.len(), 2);
    }

    #[test]
    fn the_default_list_cannot_be_deleted() {
        let mut s = Sidecar::new();
        assert_eq!(s.delete(DEFAULT_ID), Err(GitError::DefaultChangelist));
    }

    #[test]
    fn duplicate_names_are_refused() {
        let mut s = Sidecar::new();
        s.create("Fixes", "").unwrap();
        assert!(matches!(
            s.create("Fixes", ""),
            Err(GitError::DuplicateChangelist { .. })
        ));
    }

    #[test]
    fn slugs_do_not_collide() {
        let mut s = Sidecar::new();
        let a = s.create("My List!", "").unwrap();
        let b = s.create("my  list", "").unwrap();
        assert_eq!(a, "my-list");
        assert_ne!(a, b);
    }

    #[test]
    fn switching_the_active_list_pins_existing_unfiled_changes() {
        let mut s = Sidecar::new();
        let fixes = s.create("Fixes", "").unwrap();
        let live = BTreeSet::from(["a".to_string(), "b".to_string()]);
        s.set_active(&fixes, &live).unwrap();
        // `a` and `b` existed before the switch, so they stay where the user last saw them.
        assert_eq!(s.owner_of("a"), DEFAULT_ID);
        assert_eq!(s.owner_of("b"), DEFAULT_ID);
        // Anything that shows up afterwards joins the newly active list.
        assert_eq!(s.owner_of("c"), fixes);
    }

    #[test]
    fn reconcile_drops_dead_assignments() {
        let mut s = Sidecar::new();
        let fixes = s.create("Fixes", "").unwrap();
        s.move_paths(&fixes, &["a".into(), "b".into()]).unwrap();
        assert!(s.reconcile(&BTreeSet::from(["a".to_string()])));
        assert_eq!(s.get(&fixes).unwrap().paths.len(), 1);
        // A second pass has nothing to do and must say so, or the sidecar is rewritten on
        // every status refresh.
        assert!(!s.reconcile(&BTreeSet::from(["a".to_string()])));
    }
}
