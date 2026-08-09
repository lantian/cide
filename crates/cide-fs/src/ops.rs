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

    #[test]
    fn a_binary_file_is_refused_rather_than_mangled() {
        let dir = scratch("ops-binary");
        let file = dir.join("blob.bin");
        std::fs::write(&file, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        assert!(matches!(read_to_string(&file), Err(FsError::NotUtf8(_))));
    }
}
