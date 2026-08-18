//! *Project Notes*: the third synthetic row, and the first that is **pinned**.
//!
//! [`crate::groups::ProjectGroups`] is the state; this is the policy, the same shape as
//! [`crate::libraries`] and [`crate::scratches`] and sharing nothing with either but the row
//! space. Where the three differ is the fastest way to understand this one:
//!
//! | | *External Libraries* | *Scratches* | *Project Notes* |
//! | --- | --- | --- | --- |
//! | the row is | a header that expands | a header that expands | a **pin** that *opens* |
//! | shown when | the project has a manifest | always, for any project | **always**, for any project |
//! | filled by | `cargo metadata`, on a thread | one `read_dir`, inline | **nothing — it has no contents** |
//! | re-listed | on invalidate | after every mutation | **never; there is no listing** |
//! | writable by the tree | no | yes | **no** — see [`ProjectGroups::writable_dirs`] |
//! | writable by the editor | no (read-only tab) | yes | **yes** |
//!
//! # Why a pin and not a header with one file under it
//!
//! A header is expand-only — `rowVerbs('group')` grants nothing else, which
//! `ui/scripts/check-groups.mjs` asserts — so *Project Notes* as a group would be a drawer the
//! user clicks to reveal a single row called `notes.md`, which they then click again. The report
//! this was built from asks for a row "that allows me to open this file"; two clicks and a
//! twisty onto one child is a worse answer, and the mechanism to do better is one field in
//! `cide_fs::groups`.
//!
//! # Why it is a pin inside `Groups` rather than a third row source
//!
//! Because there is exactly one row space and `cmd::fs::compose` serves one window across the
//! seam between the index's rows and the groups'. A third source would mean nested composition
//! in `fs_tree_rows`, `fs_tree_match`, `tree_count`, `FsStatus::rows` and `reveal_in_groups`,
//! each with its own off-by-one, in the most delicate arithmetic in the crate. Putting the pin
//! in `Groups` costs `count`, `match_rows`, `row_of_group` and `position_of` **nothing at all** —
//! each already adds 1 for an unexpanded group, and a pin is never expanded.
//!
//! # The write/rename asymmetry, which is deliberate
//!
//! The editor saves the notes file — `file_write` applies no containment check, only
//! `cide_core::toolchain::read_only_reason_in`, which is about dependency caches — while the
//! *tree* refuses to rename, cut, trash or reveal the row, because the notes directory is
//! deliberately **not** in [`ProjectGroups::writable_dirs`]. That reads as an inconsistency and
//! is not one: the row is pinned to one path, so a rename would leave the pin pointing at
//! nothing and the next click would create a second, empty `notes.md` beside the user's real
//! notes. See the note on `writable_dirs` itself, which is where a future reader will go to
//! "fix" it.
//!
//! # What this deliberately does not do
//!
//! Nothing watches the file: it is outside every root, so `cide-fs` never walks it, it is never
//! in `dir_paths()` and never a picker or content-search candidate. Unlike *Scratches* there is
//! no listing to keep fresh — the row's content is a fixed label — so there is no `relist_*`
//! equivalent here at all, and an edit made by another program is picked up by the editor's own
//! conflict machinery on the next tab activation rather than by the tree.

use std::path::PathBuf;

use crate::groups::ProjectGroups;

/// The pin's id, in the `cide://group/<id>` sentinel its row carries.
/// `ui/src/sidebar/groupRows.ts` names the same string, and `check:notes` pins the agreement.
pub const GROUP_ID: &str = "projectNotes";

/// What the row says.
///
/// The user's words. *Notes* alone would sit in a tree beside a user's own `notes/` directory
/// and read as one of theirs; *Project Notes* says whose they are and what keys them.
pub const GROUP_LABEL: &str = "Project Notes";

impl ProjectGroups {
    /// Draw the *Project Notes* pin for a project rooted at `primary_root`.
    ///
    /// Called once, from [`ProjectGroups::prepare`](crate::groups::ProjectGroups::prepare),
    /// which owns the guard. `None` for a project with no roots at all — there is nothing to key
    /// a notes file by, and `cide_core::workspace` refuses to make such a project anyway, so this
    /// is the defensive arm rather than a case.
    ///
    /// **Nothing is created on disk here**, and that is the requirement rather than an
    /// optimisation: `file_for` is arithmetic over a hash, and writing an empty `notes.md` into
    /// `$XDG_STATE_HOME` for every project that is ever opened — including one opened by
    /// accident and one whose row is never clicked — is a cost with no matching gesture. The
    /// first `fs_notes_ensure` creates it. The same rule `show_scratches` states next door.
    ///
    /// **The path is not remembered**, unlike the scratch drawer's. `ProjectGroups` held a
    /// `notes_file` memo here first and nothing ever read it: `cmd::fs::fs_notes_ensure` derives
    /// the path from `roots[0]` for itself, because it needs the *root* — the origin breadcrumb
    /// is written from it — and not merely the file. A second derivation of the one path the
    /// user's writing lives at is exactly what `cide_core::notes` refuses to have when it
    /// reuses `scratch::key` rather than copying the hashing rule, and it would have been free
    /// to drift the day a project's roots changed under a `prepare` that had already run.
    /// `scratch_dir` is remembered for a reason that does not apply here: `writable_dirs` asks
    /// for it on every mutating file command, and the notes file is deliberately not in that
    /// set.
    pub(crate) fn show_notes(&self, primary_root: Option<&PathBuf>) {
        if primary_root.is_none() {
            return;
        }
        self.rows.write().pin(GROUP_ID, GROUP_LABEL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::TreeRowKind;
    use std::path::Path;

    /// A project root of its own, so the blake3 key — and therefore the notes path — is unique
    /// to this test and nothing shared is written.
    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-notes-app-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(cide_core::notes::dir_for(root));
        let _ = std::fs::remove_file(cide_core::notes::origin_path(root));
        let _ = std::fs::remove_dir_all(root);
    }

    fn names(groups: &ProjectGroups) -> Vec<String> {
        groups.rows(0, 100).into_iter().map(|r| r.name).collect()
    }

    /// The pin is one row that opens, and showing it writes nothing.
    #[test]
    fn every_project_gets_the_pin_and_showing_it_writes_nothing() {
        let root = temp("shown");
        let groups = ProjectGroups::new();
        groups.show_notes(Some(&root));

        let rows = groups.rows(0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, TreeRowKind::Pin);
        assert_eq!(rows[0].name, GROUP_LABEL);
        assert!(!rows[0].has_children, "a pin opens; it does not unfold");
        assert!(!rows[0].expanded);
        assert_eq!(rows[0].path, cide_fs::groups::group_path(GROUP_ID));

        assert!(
            !cide_core::notes::dir_for(&root).exists(),
            "opening a project must not write anything into $XDG_STATE_HOME"
        );
        assert!(!cide_core::notes::file_for(&root).exists());
        cleanup(&root);
    }

    /// Draw order, and that it does not depend on which policy ran first.
    #[test]
    fn the_pin_draws_above_both_headers_whatever_order_they_were_shown_in() {
        let root = temp("order");
        let groups = ProjectGroups::new();
        groups.show_scratches(Some(&root));
        groups
            .rows
            .write()
            .show("externalLibraries", "External Libraries");
        groups.show_notes(Some(&root));
        assert_eq!(
            names(&groups),
            [GROUP_LABEL, "Scratches", "External Libraries"],
            "a pin sorts above every header even when it was pinned last"
        );
        let _ = std::fs::remove_dir_all(cide_core::scratch::dir_for(&root));
        let _ = std::fs::remove_file(cide_core::scratch::origin_path(&root));
        cleanup(&root);
    }

    /// The asymmetry, asserted so a widening of `writable_dirs` breaks a test rather than a
    /// user's notes: the tree may not rename or trash the row, while the editor still saves it.
    #[test]
    fn the_notes_file_is_not_a_directory_the_tree_may_change_things_in() {
        let root = temp("writable");
        let groups = ProjectGroups::new();
        groups.show_notes(Some(&root));
        assert!(
            groups.writable_dirs().is_empty(),
            "a rename would orphan the pin and the next click would make a second notes.md"
        );
        cleanup(&root);
    }

    /// A project with no roots draws no pin, rather than one keyed by nothing.
    #[test]
    fn a_project_with_no_roots_has_no_pin() {
        let groups = ProjectGroups::new();
        groups.show_notes(None);
        assert_eq!(groups.count(), 0);
    }
}
