//! *Scratches*: the second synthetic group, and the only one cide writes into.
//!
//! [`crate::groups::ProjectGroups`] is the state; this is the policy, the same shape as
//! [`crate::libraries`] and sharing nothing with it but the row space. Where the two differ is
//! worth reading before changing either, because almost every decision below is the *opposite*
//! of the one next door and for a reason:
//!
//! | | *External Libraries* | *Scratches* |
//! | --- | --- | --- |
//! | shown when | the project has a manifest | **always**, for any project |
//! | filled by | `cargo metadata`, on a thread | one `read_dir`, inline |
//! | filled when | the user expands the header | **eagerly**, on the first tree read |
//! | writable | no — a rename would break every project on the machine | **yes** |
//! | empty means | something went wrong; say what | nothing has been made yet; say so |
//!
//! # Why it is shown even when it is empty
//!
//! *External Libraries* refuses to draw a header it cannot fill, and the argument is that a
//! group promising somebody else's source with none to show is worse than no group. That
//! argument does not transfer: this header does not promise contents, it promises a *place*.
//! An empty drawer is still a drawer, and a user who has never made a scratch is precisely the
//! user who needs to find out the drawer exists. So the header is always there and an empty one
//! carries a note saying what makes a scratch — which is also the only discovery route that
//! does not depend on the user already knowing the chord.
//!
//! # Why the listing is eager, when the other group's is not
//!
//! Because it costs one `read_dir` of a flat directory holding a handful of files, and the
//! alternative costs a `cargo metadata` fork. Laziness is a mechanism for making an expensive
//! thing wait until it is wanted; applied to a syscall it buys nothing and pays for it in
//! staleness — a group filled on expand has to be *re*-filled after every create, rename and
//! delete, and every path that forgets is a drawer showing the file the user just deleted.
//! Listing eagerly and re-listing after each mutation is the same rule stated once.
//!
//! Nothing watches the drawer, deliberately: `cide_fs::groups` has no watcher and acquiring one
//! for a directory only cide writes to would be an inotify watch to learn what the writing code
//! already knows. A scratch changed by something *other* than cide — an editor in another
//! window of another program — is picked up by collapsing and re-expanding the group, which is
//! the same refresh gesture the other group offers.

use std::path::{Path, PathBuf};

use cide_fs::groups::Entry;

use crate::groups::ProjectGroups;

/// The group's id, in the `cide://group/<id>` sentinel its header carries.
/// `ui/src/sidebar/groupRows.ts` names the same string.
pub const GROUP_ID: &str = "scratches";

/// What the header row says.
///
/// IDEA calls its equivalent *Scratches and Consoles* because it also holds database consoles,
/// which cide has none of. Naming a thing after a feature that does not exist is how a user
/// goes looking for it, so the label is the half that is true.
pub const GROUP_LABEL: &str = "Scratches";

/// What an empty drawer says.
///
/// A row and not an absence, exactly as *Resolving…* is next door: a group that draws a twisty
/// onto nothing is indistinguishable from one that failed. This one is also the feature's only
/// self-documenting surface — it names the command rather than a chord, because a chord is a
/// thing `keymap.json` can change and a command id is not.
const EMPTY: &str = "No scratch files yet — “New scratch file…” makes one.";

impl ProjectGroups {
    /// Draw the *Scratches* header for a project rooted at `primary_root`, and list its drawer.
    ///
    /// Called once, from [`ProjectGroups::prepare`](crate::groups::ProjectGroups::prepare),
    /// which owns the guard. `None` for a project with no roots at all — there is nothing to
    /// key a drawer by, and `cide_core::workspace` refuses to make such a project anyway, so
    /// this is the defensive arm rather than a case.
    ///
    /// **The drawer is not created here.** `dir_for` is arithmetic over a hash; `ensure_dir`
    /// writes to disk, and writing a directory into `$XDG_STATE_HOME` for every project that is
    /// ever opened — including one opened by accident, and one that never makes a scratch — is
    /// a cost with no matching gesture. The first [`ProjectGroups::new_scratch`] creates it.
    pub(crate) fn show_scratches(&self, primary_root: Option<&PathBuf>) {
        let Some(root) = primary_root else {
            return;
        };
        let dir = cide_core::scratch::dir_for(root);
        *self.scratch_dir.lock() = Some(dir);
        self.rows.write().show(GROUP_ID, GROUP_LABEL);
        self.relist_scratches();
    }

    /// Where this project's scratch files live, or `None` before the group has been shown.
    pub fn scratch_dir(&self) -> Option<PathBuf> {
        self.scratch_dir.lock().clone()
    }

    /// Whether this path is inside the drawer.
    ///
    /// The question every mutating `fs_*` handler asks after the containment check passes: a
    /// rename that landed in the drawer has to re-list it, because nothing watches it. Textual
    /// containment through `cide_core::toolchain::under`, which is component-wise — so a
    /// sibling directory whose name merely starts with the drawer's is not inside it.
    pub fn is_scratch_path(&self, path: &Path) -> bool {
        self.scratch_dir
            .lock()
            .as_deref()
            .is_some_and(|dir| cide_core::toolchain::under(path, dir))
    }

    /// Re-read the drawer and install its contents as the group's rows.
    ///
    /// Idempotent and cheap — one `read_dir` — which is what lets every mutation call it rather
    /// than reasoning about which ones could have changed the listing. It is called after a
    /// create, a rename and a delete, and each of those is a gesture a human made, so the cost
    /// is one syscall per keystroke-sized event.
    ///
    /// A group that has never been shown is left alone: `fulfil` answers `false` for an unknown
    /// id, so this is safe to call from a handler that does not know whether the tree has been
    /// read yet.
    pub fn relist_scratches(&self) {
        let Some(dir) = self.scratch_dir() else {
            return;
        };
        let files = cide_core::scratch::list(&dir);
        let count = files.len();
        let rows: Vec<Entry> = if files.is_empty() {
            vec![Entry::note(EMPTY)]
        } else {
            files
                .into_iter()
                .map(|path| Entry {
                    // `file_name` is `Some` for every path `scratch::list` returns — it filters
                    // on exactly that — so the fallback is unreachable rather than a policy.
                    name: path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    // No version, no size, no mtime. The panel is 252px wide and a scratch's
                    // name is the whole of what distinguishes one from another.
                    detail: None,
                    // Flat: `scratch::list` returns files only, so nothing here gets a twisty.
                    dir: false,
                    path: Some(path),
                })
                .collect()
        };
        // The count on the header, withheld at zero — where the note underneath already says
        // it, and `Scratches  0` beside *No scratch files yet* is two ways of saying one thing.
        self.rows
            .write()
            .fulfil(GROUP_ID, rows, (count > 0).then(|| count.to_string()));
    }

    /// Create a scratch file of this type, and answer its path.
    ///
    /// The drawer is created here rather than at project open — see [`Self::show_scratches`] —
    /// and the listing is refreshed before this returns, so the row exists the instant the
    /// command answers. That ordering is the same one `cmd::fs::create_entry` argues for at
    /// length: a row that appears a few hundred milliseconds later, once a watcher notices, is
    /// long enough for the user to decide the gesture did nothing. There is no watcher here at
    /// all, so it would never appear.
    pub fn new_scratch(&self, primary_root: &Path, ext: &str) -> cide_core::Result<PathBuf> {
        let dir = cide_core::scratch::ensure_dir(primary_root)?;
        // Recorded even when `show_scratches` has not run — a window that creates a scratch
        // before its tree has ever been read must still be allowed to save it, and
        // `writable_dirs` is what decides that.
        *self.scratch_dir.lock() = Some(dir.clone());
        let path = cide_core::scratch::create(&dir, ext)?;
        self.rows.write().show(GROUP_ID, GROUP_LABEL);
        self.relist_scratches();
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::TreeRowKind;

    /// A project root of its own, so the blake3 key — and therefore the drawer — is unique to
    /// this test and nothing shared is written.
    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-scratches-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(cide_core::scratch::dir_for(root));
        let _ = std::fs::remove_file(cide_core::scratch::origin_path(root));
        let _ = std::fs::remove_dir_all(root);
    }

    fn names(groups: &ProjectGroups) -> Vec<String> {
        groups.rows(0, 100).into_iter().map(|r| r.name).collect()
    }

    /// The difference from *External Libraries*, and the whole discovery story: a project with
    /// nothing in its drawer still has the drawer, and the empty one says what fills it.
    #[test]
    fn a_project_with_no_scratches_still_has_the_group_and_the_group_says_so() {
        let root = temp("empty");
        let groups = ProjectGroups::new();
        groups.show_scratches(Some(&root));

        let rows = groups.rows(0, 10);
        assert_eq!(rows.len(), 1, "collapsed, so only the header draws");
        assert_eq!(rows[0].kind, TreeRowKind::Group);
        assert_eq!(rows[0].name, GROUP_LABEL);
        assert_eq!(
            rows[0].detail, None,
            "no count beside a note that already says there is nothing"
        );

        groups.expand(&cide_fs::groups::group_path(GROUP_ID));
        let rows = groups.rows(0, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].kind, TreeRowKind::Note);
        assert_eq!(rows[1].name, EMPTY);
        assert!(!rows[1].has_children, "a sentence has nothing to open");

        assert!(
            !cide_core::scratch::dir_for(&root).exists(),
            "opening a project must not write a directory into $XDG_STATE_HOME"
        );
        cleanup(&root);
    }

    /// Creating one: the file is on disk, the row is in the tree, and neither waited for a
    /// watcher that does not exist.
    #[test]
    fn a_new_scratch_is_a_row_the_instant_the_command_answers() {
        let root = temp("create");
        let groups = ProjectGroups::new();
        groups.show_scratches(Some(&root));
        groups.expand(&cide_fs::groups::group_path(GROUP_ID));

        let path = groups.new_scratch(&root, "rs").expect("create");
        assert!(path.is_file());
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("scratch.rs")
        );

        assert_eq!(names(&groups), [GROUP_LABEL, "scratch.rs"]);
        let rows = groups.rows(0, 10);
        assert_eq!(rows[0].detail.as_deref(), Some("1"));
        assert_eq!(rows[1].kind, TreeRowKind::File);
        assert_eq!(rows[1].path, path);
        assert!(!rows[1].has_children, "the drawer is flat");

        groups.new_scratch(&root, "rs").expect("second");
        groups.new_scratch(&root, "json").expect("other type");
        assert_eq!(
            names(&groups),
            [GROUP_LABEL, "scratch.json", "scratch.rs", "scratch_1.rs"],
            "sorted by name, and the counter is per extension"
        );
        cleanup(&root);
    }

    /// The property every mutating handler leans on: a delete outside cide's index is still a
    /// row that goes away, because the drawer is re-listed rather than watched.
    #[test]
    fn deleting_the_last_scratch_puts_the_note_back() {
        let root = temp("relist");
        let groups = ProjectGroups::new();
        groups.show_scratches(Some(&root));
        groups.expand(&cide_fs::groups::group_path(GROUP_ID));
        let path = groups.new_scratch(&root, "md").expect("create");
        assert_eq!(names(&groups), [GROUP_LABEL, "scratch.md"]);

        std::fs::remove_file(&path).expect("delete");
        assert_eq!(
            names(&groups),
            [GROUP_LABEL, "scratch.md"],
            "nothing watches the drawer, so the row is still there until somebody re-lists"
        );
        groups.relist_scratches();
        assert_eq!(names(&groups), [GROUP_LABEL, EMPTY]);
        cleanup(&root);
    }

    /// Containment, which is what `fs_rename` and `fs_delete` are allowed inside.
    #[test]
    fn only_paths_inside_the_drawer_are_scratch_paths() {
        let root = temp("inside");
        let groups = ProjectGroups::new();
        assert!(
            !groups.is_scratch_path(&cide_core::scratch::dir_for(&root).join("scratch.rs")),
            "before the group is shown there is no drawer, so nothing is inside it"
        );
        assert!(groups.writable_dirs().is_empty());

        groups.show_scratches(Some(&root));
        let dir = cide_core::scratch::dir_for(&root);
        assert_eq!(groups.writable_dirs(), vec![dir.clone()]);
        assert!(groups.is_scratch_path(&dir.join("scratch.rs")));
        assert!(groups.is_scratch_path(&dir));
        assert!(!groups.is_scratch_path(&root.join("src/main.rs")));
        assert!(!groups.is_scratch_path(Path::new("/etc/passwd")));
        // Component-wise, so a sibling whose name starts with the drawer's is outside it. This
        // is the sharp edge of a hashed directory name: `<key>.json` is the origin record, and
        // a string prefix test would put it *inside* the drawer and make it renamable.
        assert!(!groups.is_scratch_path(&cide_core::scratch::origin_path(&root)));
        cleanup(&root);
    }

    /// A project with no roots draws no drawer, rather than one keyed by nothing.
    #[test]
    fn a_project_with_no_roots_has_no_scratch_group() {
        let groups = ProjectGroups::new();
        groups.show_scratches(None);
        assert_eq!(groups.count(), 0);
        assert!(groups.scratch_dir().is_none());
    }
}
