//! File operations: read, write, create, rename, delete.
//!
//! Everything here takes an absolute path and checks it against the project's roots first.
//! The frontend can only send back a path the backend gave it, so a path from outside is
//! either a bug or someone trying one, and neither is a reason to write to `/etc`.

use std::path::{Path, PathBuf};

use crate::error::{FsError, Result};

/// The largest file this will read into memory.
///
/// The milestone after this one opens a 5 MB Rust file as an acceptance test, so the limit
/// has to sit comfortably above that. It exists to keep a stray `git pack` or a core dump
/// from being serialised through the IPC channel into a webview.
pub const MAX_READ: u64 = 32 * 1024 * 1024;

/// Reject a path that is not inside one of the roots.
///
/// Textual containment, deliberately: `canonicalize` would resolve symlinks, and a project
/// that legitimately contains a symlinked directory would fail its own writes. The
/// `..`-climbing case is handled by rejecting the component outright, which is stricter than
/// normalising it and easier to be sure of.
pub fn check_within(roots: &[PathBuf], path: &Path) -> Result<()> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(FsError::InvalidPath(path.display().to_string()));
    }
    if !path.is_absolute() {
        return Err(FsError::InvalidPath(path.display().to_string()));
    }
    if roots.iter().any(|root| path.starts_with(root)) {
        return Ok(());
    }
    Err(FsError::OutsideProject(path.display().to_string()))
}

/// Reject a path that *is* one of the roots.
///
/// [`check_within`] admits the root itself, which is right for reading and revealing and
/// catastrophic for the operations that move things. In a multi-root project each root is a
/// row of its own, so "select a row, press delete" reaches this with the root's path and
/// would put the entire project directory in the trash — and the tree would not even redraw,
/// because [`crate::Index`] deliberately refuses to remove a root node. The undo exists but
/// it is a desktop trash the user has to go and find, which is not an undo anyone wants to
/// need. Cheaper to refuse: nothing in the UI asks to delete a root on purpose.
pub fn check_not_root(roots: &[PathBuf], path: &Path) -> Result<()> {
    if roots.iter().any(|root| root == path) {
        return Err(FsError::IsRoot(path.display().to_string()));
    }
    Ok(())
}

pub fn read_to_string(path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path).map_err(|e| FsError::io(path, e))?;
    if meta.len() > MAX_READ {
        return Err(FsError::TooLarge {
            path: path.display().to_string(),
            size: meta.len(),
            limit: MAX_READ,
        });
    }
    let bytes = std::fs::read(path).map_err(|e| FsError::io(path, e))?;
    String::from_utf8(bytes).map_err(|_| FsError::NotUtf8(path.display().to_string()))
}

/// Write a file, atomically.
///
/// Through a temporary file in the same directory and a `rename(2)`, so a crash mid-write
/// leaves the previous contents rather than a truncated file. The cost is real and worth
/// naming: the destination's inode changes, which breaks a hard link and drops any ACL or
/// xattr that was set on it. Permissions are carried over explicitly; the rest is the price
/// of never showing a user a half-written file.
pub fn write(path: &Path, contents: &str) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))?;
    let name = path
        .file_name()
        .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))?;
    let tmp = dir.join(format!(
        ".{}.cide-{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));

    std::fs::write(&tmp, contents).map_err(|e| FsError::io(&tmp, e))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(FsError::io(path, err));
    }
    Ok(())
}

/// Check a name the user typed for a new entry, before it is joined to anything.
///
/// Spelled out here rather than left to the OS. "The OS will reject it" is not an answer a
/// file tree can give: `open(2)` fails with `EISDIR` or `ENOENT` or nothing at all depending
/// on which rule was broken, and the user finds out *after* the gesture, from a message about
/// a syscall. The frontend runs the same rules in `ui/src/sidebar/newEntry.ts` so it can say
/// so while the name is still being typed; this copy is the load-bearing one, because a name
/// arriving here has crossed the IPC boundary and nothing about it can be assumed.
///
/// The reason travels inside [`FsError::InvalidPath`] rather than in a variant of its own. A
/// new variant would be the tidier shape and it would also be a change to the error enum every
/// other surface in the app matches on, for a message only this one gesture can produce.
pub fn check_name(name: &str) -> Result<()> {
    let reason = if name.is_empty() {
        "a name cannot be empty"
    } else if name.trim().is_empty() {
        "a name cannot be only whitespace"
    } else if name != name.trim() {
        // A trailing space is legal on Linux and invisible in a 24px row, so a file called
        // "main.rs " reads as one that is simply missing from every command that names it.
        // The frontend trims before it sends, so this only ever fires on a caller that did not.
        "a name cannot start or end with whitespace"
    } else if name.contains('/') || name.contains(std::path::MAIN_SEPARATOR) {
        // Not silently replaced with `_` the way the rename box does it — rename is editing a
        // name in place, where a stray `/` is a typo, and this is *choosing* a location, where
        // `src/foo.rs` is a thing the user plainly meant and would not get.
        "a name cannot contain a path separator — create the folder first, then the file in it"
    } else if name.contains('\0') {
        "a name cannot contain a NUL byte"
    } else if name == "." || name == ".." {
        "\".\" and \"..\" already name this directory and its parent"
    } else {
        return Ok(());
    };
    Err(FsError::InvalidPath(format!("{name:?}: {reason}")))
}

/// Create `name` inside `parent`, and answer with the path that now exists.
///
/// Distinct from [`create`], which takes a whole path and calls `create_dir_all` on its way
/// there. That is right for a caller that has computed a path it knows should exist, and wrong
/// for this one: the gesture starts from a row in a tree that was drawn some time ago, so the
/// directory it names may have been deleted since — and `create_dir_all` would put it back.
/// A *New File* that silently resurrects a folder the user deleted is worse than one that
/// refuses, so the parent has to already be a directory.
pub fn create_in(roots: &[PathBuf], parent: &Path, name: &str, directory: bool) -> Result<PathBuf> {
    check_within(roots, parent)?;
    check_name(name)?;
    if !parent.is_dir() {
        return Err(FsError::Io {
            path: parent.display().to_string(),
            message: "is no longer a directory — it may have been deleted or renamed".to_string(),
        });
    }

    let target = parent.join(name);
    // The containment check again, on the joined path. `check_name` already refuses every
    // component that could climb out, so this can only fire on a bug in the two lines above
    // it — which is exactly the bug worth a string comparison.
    check_within(roots, &target)?;

    // `symlink_metadata` as well as `exists`: a *broken* symlink is a real directory entry
    // that `exists()` reports as absent, and reporting "already exists" beats `create_new`'s
    // raw `EEXIST` for something the user can see in their tree.
    if target.exists() || target.symlink_metadata().is_ok() {
        return Err(FsError::Exists(target.display().to_string()));
    }

    if directory {
        std::fs::create_dir(&target).map_err(|e| FsError::io(&target, e))?;
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| FsError::io(&target, e))?;
    }
    Ok(target)
}

/// Create an empty file, or a directory. Never clobbers.
pub fn create(path: &Path, directory: bool) -> Result<()> {
    if path.exists() {
        return Err(FsError::Exists(path.display().to_string()));
    }
    if directory {
        return std::fs::create_dir_all(path).map_err(|e| FsError::io(path, e));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| FsError::io(parent, e))?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|e| FsError::io(path, e))
}

/// Rename, refusing to overwrite.
///
/// `rename(2)` replaces the destination silently, which for a file tree means a typo in a
/// rename can destroy a file. The check is not atomic — nothing available here is — but it
/// turns the common accident into an error message.
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        return Err(FsError::Exists(to.display().to_string()));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| FsError::io(parent, e))?;
    }
    std::fs::rename(from, to).map_err(|e| FsError::io(from, e))
}

/// Move to the trash. See [`crate::trash`] for why this is not `remove_file`.
pub fn delete(path: &Path) -> Result<PathBuf> {
    crate::trash::move_to_trash(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    #[test]
    fn a_path_outside_the_roots_is_refused() {
        let roots = vec![PathBuf::from("/home/u/project")];
        assert!(check_within(&roots, Path::new("/home/u/project/src/a.rs")).is_ok());
        assert!(matches!(
            check_within(&roots, Path::new("/etc/passwd")),
            Err(FsError::OutsideProject(_))
        ));
        assert!(matches!(
            check_within(&roots, Path::new("/home/u/project/../../etc/passwd")),
            Err(FsError::InvalidPath(_))
        ));
        assert!(matches!(
            check_within(&roots, Path::new("relative/a.rs")),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn a_root_is_inside_the_project_but_still_refuses_to_be_moved() {
        let roots = vec![
            PathBuf::from("/home/u/project"),
            PathBuf::from("/home/u/other"),
        ];
        // `check_within` says yes to a root — that is what makes `fs.read`/`fs.reveal` work
        // on it — which is exactly why the move operations need the second check.
        assert!(check_within(&roots, Path::new("/home/u/project")).is_ok());
        assert!(matches!(
            check_not_root(&roots, Path::new("/home/u/project")),
            Err(FsError::IsRoot(_))
        ));
        assert!(matches!(
            check_not_root(&roots, Path::new("/home/u/other")),
            Err(FsError::IsRoot(_))
        ));
        assert!(check_not_root(&roots, Path::new("/home/u/project/src")).is_ok());
    }

    #[test]
    fn writing_replaces_contents_and_leaves_no_temporary_behind() {
        let dir = scratch("ops-write");
        let file = dir.join("a.rs");
        write(&file, "fn main() {}").unwrap();
        write(&file, "fn other() {}").unwrap();
        assert_eq!(read_to_string(&file).unwrap(), "fn other() {}");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn writing_keeps_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("ops-perms");
        let file = dir.join("script.sh");
        write(&file, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        write(&file, "#!/bin/sh\necho hi\n").unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "{mode:o}");
    }

    #[test]
    fn create_and_rename_refuse_to_clobber() {
        let dir = scratch("ops-create");
        create(&dir.join("a.rs"), false).unwrap();
        assert!(matches!(
            create(&dir.join("a.rs"), false),
            Err(FsError::Exists(_))
        ));
        create(&dir.join("sub/deep"), true).unwrap();
        assert!(dir.join("sub/deep").is_dir());

        create(&dir.join("b.rs"), false).unwrap();
        assert!(matches!(
            rename(&dir.join("a.rs"), &dir.join("b.rs")),
            Err(FsError::Exists(_))
        ));
        rename(&dir.join("a.rs"), &dir.join("c.rs")).unwrap();
        assert!(dir.join("c.rs").exists());
        assert!(!dir.join("a.rs").exists());
    }

    /// Every name rule the tree's *New File…* box promises, held where the promise is kept.
    ///
    /// The frontend has the same list in `newEntry.ts` and `check-new-entry.mjs` pins it
    /// there. Two copies on purpose: that one exists to tell the user *while they type*, this
    /// one exists because a name reaching this function has been over IPC.
    #[test]
    fn a_name_is_refused_with_a_reason_rather_than_left_to_the_os() {
        let bad = [
            ("", "empty"),
            ("   ", "only whitespace"),
            (" main.rs", "a leading space"),
            ("main.rs ", "a trailing space"),
            ("src/main.rs", "a path separator"),
            ("..", "the parent directory"),
            (".", "this directory"),
            ("a\0b", "a NUL byte"),
        ];
        for (name, what) in bad {
            let err = check_name(name);
            assert!(
                matches!(err, Err(FsError::InvalidPath(_))),
                "{what}: {err:?}"
            );
            // The reason has to be *in* the error. A tagged variant with no prose would leave
            // the panel showing "not a valid path", which is the message this rule replaced.
            let Err(FsError::InvalidPath(message)) = err else {
                unreachable!()
            };
            assert!(message.len() > name.len() + 4, "{what}: {message}");
        }

        // A leading dot is a legitimate filename and is NOT refused. `.gitignore` is the
        // single most likely thing anybody creates from this menu.
        assert!(check_name(".gitignore").is_ok());
        assert!(
            check_name("...").is_ok(),
            "three dots is a legal name, unlike one or two"
        );
        assert!(check_name("main.rs").is_ok());
        // A backslash is an ordinary character on Unix, and refusing it here would refuse a
        // name the platform accepts. `MAIN_SEPARATOR` covers the platform where it is not.
        #[cfg(unix)]
        assert!(check_name("a\\b").is_ok());
    }

    #[test]
    fn creating_in_a_directory_that_vanished_is_refused_rather_than_recreating_it() {
        let dir = scratch("ops-create-in-gone");
        let roots = vec![dir.path().to_path_buf()];
        let parent = dir.join("sub");
        std::fs::create_dir(&parent).unwrap();
        std::fs::remove_dir(&parent).unwrap();

        // The distinction that matters: `create` would have called `create_dir_all` and put
        // `sub` back, so the user's deleted folder would reappear holding one new file.
        assert!(matches!(
            create_in(&roots, &parent, "a.rs", false),
            Err(FsError::Io { .. })
        ));
        assert!(
            !parent.exists(),
            "the vanished directory must stay vanished"
        );
    }

    #[test]
    fn creating_a_name_that_exists_is_refused_and_leaves_the_file_alone() {
        let dir = scratch("ops-create-in-exists");
        let roots = vec![dir.path().to_path_buf()];
        std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();

        assert!(matches!(
            create_in(&roots, dir.path(), "a.rs", false),
            Err(FsError::Exists(_))
        ));
        assert_eq!(read_to_string(&dir.join("a.rs")).unwrap(), "fn main() {}");

        // A directory over an existing file, and a file over an existing directory, are the
        // same refusal — `create_dir` and `create_new` fail differently and the tree does not
        // care which.
        std::fs::create_dir(dir.join("d")).unwrap();
        assert!(matches!(
            create_in(&roots, dir.path(), "d", true),
            Err(FsError::Exists(_))
        ));
        assert!(matches!(
            create_in(&roots, dir.path(), "d", false),
            Err(FsError::Exists(_))
        ));
    }

    /// A **broken symlink** is a name that is taken, and has to be reported as one.
    ///
    /// The `symlink_metadata` half of the existence check exists only for this, and until this
    /// test it was a line with a comment and nothing holding it: `Path::exists` follows the
    /// link and answers `false` for a dangling one, so dropping the extra call left every
    /// assertion in this file passing while the user got `create_new`'s raw `EEXIST` — an
    /// `Io` about a file the tree is plainly showing them.
    #[cfg(unix)]
    #[test]
    fn a_broken_symlink_is_a_name_that_is_taken_not_an_eexist_from_the_syscall() {
        let dir = scratch("ops-create-in-dangling");
        let roots = vec![dir.path().to_path_buf()];
        std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("link.rs")).unwrap();
        assert!(!dir.join("link.rs").exists(), "the link must be dangling");

        assert!(
            matches!(
                create_in(&roots, dir.path(), "link.rs", false),
                Err(FsError::Exists(_))
            ),
            "a dangling symlink reported as anything but Exists"
        );
        assert!(matches!(
            create_in(&roots, dir.path(), "link.rs", true),
            Err(FsError::Exists(_))
        ));
        // And the link itself is untouched — nothing was written through it to `nowhere`.
        assert!(dir.join("link.rs").symlink_metadata().is_ok());
        assert!(!dir.join("nowhere").exists());
    }

    /// A parent that is a *file* is refused like one that vanished, not opened as a directory.
    ///
    /// Reachable from the frontend the moment `targetFor` is asked about a row that changed
    /// kind under it — a directory replaced by a file between the right-click and the click.
    #[test]
    fn a_parent_that_is_a_file_is_refused() {
        let dir = scratch("ops-create-in-file-parent");
        let roots = vec![dir.path().to_path_buf()];
        std::fs::write(dir.join("notadir"), "x").unwrap();

        // The *message* is the assertion, not the variant. `Err(Io)` also comes out of this
        // function when nothing checked the parent at all and `open(2)` returned `ENOTDIR` —
        // the panel then says "Not a directory (os error 20)" about a row the user can see,
        // which is the syscall-shaped refusal `check_name` and this branch exist to replace.
        let Err(FsError::Io { message, .. }) =
            create_in(&roots, &dir.join("notadir"), "a.rs", false)
        else {
            panic!("a file was accepted as a parent directory");
        };
        assert!(
            message.contains("no longer a directory"),
            "the refusal has to say what is wrong with the parent, not quote errno: {message}"
        );
        assert_eq!(read_to_string(&dir.join("notadir")).unwrap(), "x");
    }

    #[test]
    fn creating_outside_the_project_root_is_refused_rather_than_obeyed() {
        let dir = scratch("ops-create-in-outside");
        let outside = scratch("ops-create-in-elsewhere");
        let roots = vec![dir.path().to_path_buf()];

        assert!(matches!(
            create_in(&roots, outside.path(), "a.rs", false),
            Err(FsError::OutsideProject(_))
        ));
        assert!(!outside.join("a.rs").exists());

        // And the climb, which is the version a frontend bug would actually produce: a parent
        // assembled from a row's path and a `..` somebody typed into a name box.
        assert!(matches!(
            create_in(&roots, &dir.join(".."), "a.rs", false),
            Err(FsError::InvalidPath(_))
        ));
        assert!(matches!(
            create_in(&roots, dir.path(), "../escaped.rs", false),
            Err(FsError::InvalidPath(_))
        ));
        assert!(
            !dir.path().parent().unwrap().join("escaped.rs").exists(),
            "a `..` in the name must not write beside the project"
        );

        // A separator in the name with the directory it names **already there**. This is the
        // case where dropping `check_name` is silent rather than noisy: every other test above
        // would still fail with an `Io` from `ENOENT`, and this one would quietly succeed —
        // the user asks for a file called `sub/a.rs` beside `main.rs` and gets one inside a
        // folder, which is a different place with no message.
        std::fs::create_dir(dir.join("sub")).unwrap();
        assert!(matches!(
            create_in(&roots, dir.path(), "sub/a.rs", false),
            Err(FsError::InvalidPath(_))
        ));
        assert!(
            !dir.join("sub/a.rs").exists(),
            "a name with a separator was obeyed as a path"
        );
    }

    #[test]
    fn creating_answers_with_the_path_it_made() {
        let dir = scratch("ops-create-in-ok");
        let roots = vec![dir.path().to_path_buf()];

        let file = create_in(&roots, dir.path(), "a.rs", false).unwrap();
        assert_eq!(file, dir.join("a.rs"));
        assert!(file.is_file());
        assert_eq!(
            read_to_string(&file).unwrap(),
            "",
            "a new file starts empty"
        );

        let folder = create_in(&roots, dir.path(), "sub", true).unwrap();
        assert_eq!(folder, dir.join("sub"));
        assert!(folder.is_dir());

        // The path comes back so the caller can select the new row. Deriving it by re-joining
        // in the frontend is the same string twice, and the two drift.
        let nested = create_in(&roots, &folder, ".gitignore", false).unwrap();
        assert_eq!(nested, dir.join("sub/.gitignore"));
    }

    #[test]
    fn a_binary_file_is_refused_rather_than_mangled() {
        let dir = scratch("ops-binary");
        let file = dir.join("blob.bin");
        std::fs::write(&file, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        assert!(matches!(read_to_string(&file), Err(FsError::NotUtf8(_))));
    }
}
