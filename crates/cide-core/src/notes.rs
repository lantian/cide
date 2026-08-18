//! Project notes: one pinned markdown file per project, in state cide owns.
//!
//! A notes file is an ordinary file with an ordinary path that the editor opens, saves, undoes
//! and finds inside exactly as it does a project file. Everything special about it is *where it
//! is* and *how it is reached*: it is the only file the file tree pins a row for, and that row
//! is drawn whether or not the file exists yet.
//!
//! # Where it lives, and why not inside the project
//!
//! `$XDG_STATE_HOME/cide/notes/<key>/notes.md`, beside [`crate::scratch`]'s
//! `scratches/<key>/` and `cide_git`'s `repos/<key>/`, keyed the same way. Four reasons, the
//! last of which is specific to a *pinned* row and is the decisive one:
//!
//!   * **State, not config.** [`crate::persist::state_dir`] states the rule: written by the app
//!     rather than edited by the user, and a config directory invites a file into a dotfile
//!     repository. Notes are written through cide's own editor, on the user's behalf.
//!   * **Not inside the project**, for the reason a scratch is not: the point of the buffer is
//!     that it is *not* part of the project. A `NOTES.md` in the working tree is either an
//!     untracked file the user has to explain to themselves at every `git status`, or a line in
//!     `.gitignore` that cide put there without being asked.
//!   * **cide would be writing into somebody's repository on one click.** The pin is a row the
//!     user may click out of curiosity; a first click that creates an untracked file in their
//!     checkout is a change to their project they did not ask for and cannot see cide make.
//!   * **The row would be drawn twice, or not at all.** `cide-fs`'s walk is gitignore-aware, so
//!     a notes file inside a root is either an ordinary walked row *plus* the pin — two rows for
//!     one file, one of which cannot be renamed — or, once gitignored, the pin and nothing else
//!     while the file is invisible to the panel that is supposed to show it. Both are worse than
//!     one row about a file that lives somewhere the walk never reaches.
//!
//! Not `$XDG_DATA_HOME`: nothing in this application uses it except the freedesktop trash.
//!
//! ## What that costs, named rather than discovered
//!
//! The notes **cannot be committed or shared with a team**, and a project that is *moved or
//! renamed* gets a new key, so its notes appear to vanish. That is exactly what already happens
//! to scratches, changelists and the shelf, and nothing mitigates it for those either; the
//! mitigation here is the same one file, [`origin_path`], so a human who goes looking can tell
//! which hashed directory is theirs. If committable notes are ever wanted, the honest shape is a
//! **setting with two values**, not a change of default — and the pin mechanism supports it
//! unchanged, because [`file_for`] is the only thing in the codebase that decides the path.
//!
//! # The key, and what "per project" means with several roots
//!
//! `blake3(canonical(roots[0]))`, first 32 hex characters — [`crate::scratch::key`] verbatim,
//! **reused rather than re-derived**. `cide_git::repo::repo_key` is already a second copy of
//! that rule and a third would be the one that drifts: the day somebody changed the truncation
//! two of the three would move and the notes would silently relocate. Hoisting `key`/`canonical`
//! into a module both `scratch` and this could delegate to was the tidier alternative and was
//! rejected as churn to a module with five tests keyed on it for no behaviour change.
//!
//! The key is the **project's** primary root and not a repository, so a multi-root project has
//! **one** notes file however many roots it has — which is what "per project" means to the
//! person using it, and is already the identity `RecentProject`, the scratch drawer and
//! `show_scratches` are all keyed by (`Project::roots[0]`, "the primary root"). One file per
//! *root* was the alternative: it would put N *Project Notes* rows in one tree and make "the
//! project's notes" a question with no answer.
//!
//! # The origin record is a *sibling*, not a file inside the directory
//!
//! `notes/<key>.json`, beside `notes/<key>/`. Nothing lists this directory as tree rows — unlike
//! a scratch drawer — so the argument is weaker here than it is one module over, but consistency
//! costs nothing and the day something *does* list it the record is already out of the way.

use std::path::{Path, PathBuf};

use crate::{CoreError, Result};

/// The one file a project's notes live in.
///
/// The basename is what the tab strip draws (`cmd::file::open_file_tab` titles a tab from
/// `file_name`), so it has to read as a filename, and it has to end in `.md` or
/// `ui/src/editor/languages.ts` gives the buffer no markdown grammar and the status bar says
/// *Plain Text*. `notes/<key>.md` flat in one directory was tidier on disk and would have titled
/// the tab `4f2a9c7bd1e0….md`.
const FILE_NAME: &str = "notes.md";

/// `$XDG_STATE_HOME/cide/notes`.
pub fn notes_root() -> PathBuf {
    crate::persist::state_dir().join("notes")
}

/// Where this project's notes directory is. Does not create anything — see [`ensure`].
pub fn dir_for(primary_root: &Path) -> PathBuf {
    notes_root().join(crate::scratch::key(primary_root))
}

/// The project's notes file. Does not create anything — see [`ensure`].
pub fn file_for(primary_root: &Path) -> PathBuf {
    dir_for(primary_root).join(FILE_NAME)
}

/// The sibling record naming the project a notes directory belongs to. See the module header.
pub fn origin_path(primary_root: &Path) -> PathBuf {
    notes_root().join(format!("{}.json", crate::scratch::key(primary_root)))
}

/// Make sure the notes file exists, and answer where it is.
///
/// Called on the **first click of the pinned row**, never at project open: the row is drawn from
/// a constant label and needs no file behind it, and writing a directory and an empty file into
/// `$XDG_STATE_HOME` for every project that is ever opened — including one opened by accident,
/// and one whose notes row is never touched — is a cost with no matching gesture. The same
/// argument `show_scratches` makes for not creating its drawer.
///
/// # The one line in this feature that can destroy data
///
/// `create_new(true)`, and [`std::io::ErrorKind::AlreadyExists`] treated as success. `File::create`,
/// `fs::write` and `OpenOptions::truncate(true)` all **truncate**, so an `ensure` written with any
/// of them would erase everything the user had written the *second* time they clicked the row —
/// silently, with no dirty marker to warn them and no undo to get it back, because the buffer
/// that could have undone it had not been opened yet. `create_new` asks the kernel for the file
/// only if it is not there, which also makes two windows clicking the row in the same millisecond
/// land on one file rather than racing an `exists()` test.
///
/// The file is created **empty**. Seeding it with `# <project>` was the alternative: cide should
/// not put words into a file the user is about to write, and an empty buffer is the honest start
/// — the same call `scratch::create` makes for the same reason.
///
/// The origin record is best-effort and its failure is **ignored**, exactly as
/// [`crate::scratch::ensure_dir`] ignores its own: it is a breadcrumb for a human recovering an
/// orphaned directory, and a state directory that will not take it must not be the reason the
/// user cannot open their notes. The directory and the file are not optional, so those errors
/// are returned.
pub fn ensure(primary_root: &Path) -> Result<PathBuf> {
    let dir = dir_for(primary_root);
    std::fs::create_dir_all(&dir).map_err(|error| {
        CoreError::Io(format!(
            "could not create the notes directory {}: {error}",
            dir.display()
        ))
    })?;
    write_origin(primary_root);

    let path = dir.join(FILE_NAME);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => Ok(path),
        // The ordinary case from the second click onwards. Not an error and not a reason to
        // touch the file: the notes are already there and the editor is about to read them.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(path),
        Err(error) => Err(CoreError::Io(format!(
            "could not create {}: {error}",
            path.display()
        ))),
    }
}

/// Record which project a notes directory belongs to, best-effort. See [`ensure`].
fn write_origin(primary_root: &Path) {
    let record = serde_json::json!({
        "root": std::fs::canonicalize(primary_root)
            .unwrap_or_else(|_| primary_root.to_path_buf())
            .to_string_lossy(),
        "seen": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    });
    let path = origin_path(primary_root);
    if let Err(error) = std::fs::write(&path, format!("{record}\n")) {
        tracing::debug!(path = %path.display(), %error, "could not record a notes directory's origin");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-notes-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(dir_for(root));
        let _ = std::fs::remove_file(origin_path(root));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The key is the *scratch* key, which is the project's identity everywhere else. If these
    /// two ever disagree it is because somebody wrote a third copy of the hashing rule.
    #[test]
    fn a_project_gets_one_notes_file_named_by_the_same_key_its_scratches_are() {
        let root = Path::new("/home/u/work/cide");
        let key = crate::scratch::key(root);
        assert_eq!(dir_for(root), notes_root().join(&key));
        assert_eq!(file_for(root), notes_root().join(&key).join("notes.md"));
        assert_ne!(dir_for(root), dir_for(Path::new("/home/u/work/other")));
        assert_eq!(
            file_for(root).extension().and_then(|e| e.to_str()),
            Some("md"),
            "the extension is what gives the tab a markdown grammar"
        );
    }

    /// Lazy: the row is drawn from a label, so opening a project must write nothing.
    #[test]
    fn nothing_is_written_until_ensure_is_called() {
        let root = temp("lazy");
        assert!(!dir_for(&root).exists());
        assert!(!file_for(&root).exists());
        // Arithmetic over a hash, no I/O.
        let _ = file_for(&root);
        assert!(
            !dir_for(&root).exists(),
            "asking where it is must not make it"
        );
        cleanup(&root);
    }

    /// **The truncation guard.** The second click must not erase what the first one's tab wrote.
    #[test]
    fn ensure_is_idempotent_and_never_truncates_what_is_already_there() {
        let root = temp("truncate");
        let path = ensure(&root).expect("first");
        assert!(path.is_file());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "",
            "a new notes file is empty — cide does not put words in it"
        );

        std::fs::write(&path, "# what I was doing\n- the thing\n").expect("write");
        let again = ensure(&root).expect("second");
        assert_eq!(again, path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "# what I was doing\n- the thing\n",
            "ensure must never truncate: this is the one line in the feature that can lose data"
        );

        // And a third time, because `create_new` is the only thing standing between the user's
        // notes and `File::create`.
        ensure(&root).expect("third");
        assert!(!std::fs::read_to_string(&path).expect("read").is_empty());
        cleanup(&root);
    }

    /// The breadcrumb sits *beside* the directory, so nothing that lists the directory sees it.
    #[test]
    fn the_origin_record_sits_beside_the_directory_and_not_in_it() {
        let root = temp("origin");
        let dir = dir_for(&root);
        let record = origin_path(&root);
        assert_eq!(record.parent(), dir.parent());
        assert!(!record.starts_with(&dir));

        ensure(&root).expect("ensure");
        assert!(record.is_file(), "the breadcrumb is written");
        let written = std::fs::read_to_string(&record).expect("read");
        let canonical = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        assert!(
            written.contains(&canonical.to_string_lossy().to_string()),
            "the record names the project it came from: {written}"
        );

        // The directory holds the notes file and nothing else.
        let names: Vec<String> = std::fs::read_dir(&dir)
            .expect("read_dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["notes.md"]);
        cleanup(&root);
    }
}
