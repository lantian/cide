//! Move-to-trash, per the freedesktop.org Trash specification (v1.0).
//!
//! # Why this is written out rather than delegated
//!
//! Deleting from a file tree has to be undoable — a permanent `unlink` on a misclick is the
//! kind of thing an editor is not forgiven for. The two alternatives were shelling out to
//! `gio trash` (present only where GLib's tools are installed, and silently absent on a
//! minimal desktop or in a Flatpak sandbox) and adding the `trash` crate for what is, on
//! Linux, a directory rename and a small text file. Neither is a good trade for ~120 lines.
//!
//! # What the spec asks for, and what this does
//!
//! * The home trash is `$XDG_DATA_HOME/Trash`, defaulting to `~/.local/share/Trash`, with
//!   `files/` and `info/` subdirectories.
//! * `rename(2)` cannot cross a filesystem, so a file on another device goes to
//!   `$topdir/.Trash-$uid` instead, where `$topdir` is the mount point it lives on. The spec
//!   also allows an admin-created sticky `$topdir/.Trash`; it is rare and its checks (sticky
//!   bit, not a symlink) are easy to get subtly wrong, so this goes straight to the
//!   per-user directory, which every implementation supports.
//! * The `.trashinfo` file is written **before** the move, so a crash between the two leaves
//!   an orphaned info file (harmless, and what every trash implementation ignores) rather
//!   than a file in the trash that nothing can restore.
//! * `Path=` is percent-encoded, absolute for the home trash and relative to `$topdir` for a
//!   top-directory trash — which is what makes an external drive's trash survive being
//!   mounted somewhere else.

use std::path::{Path, PathBuf};

use crate::error::{FsError, Result};

/// Move a file or directory to the trash. Returns where it landed.
pub fn move_to_trash(path: &Path) -> Result<PathBuf> {
    move_to_trash_in(&data_home(), path)
}

/// The testable form: the home trash's parent directory is given rather than read from the
/// environment, because `$XDG_DATA_HOME` is process-global and tests run in parallel.
pub fn move_to_trash_in(data_home: &Path, path: &Path) -> Result<PathBuf> {
    let path = absolute(path)?;
    let meta = std::fs::symlink_metadata(&path).map_err(|e| FsError::io(&path, e))?;

    let home_trash = data_home.join("Trash");
    let trash_dir = if same_device(&path, &home_trash)? {
        home_trash
    } else {
        let top = top_dir(&path)?;
        top.join(format!(".Trash-{}", uid()))
    };

    let files = trash_dir.join("files");
    let info = trash_dir.join("info");
    std::fs::create_dir_all(&files).map_err(|e| FsError::io(&files, e))?;
    std::fs::create_dir_all(&info).map_err(|e| FsError::io(&info, e))?;

    let name = unique_name(&files, &info, &path)?;
    let recorded = if trash_dir.starts_with(data_home) {
        path.clone()
    } else {
        // Relative to the mount point, so the entry still resolves if the volume is mounted
        // at a different path next time.
        let top = top_dir(&path)?;
        path.strip_prefix(&top).unwrap_or(&path).to_path_buf()
    };

    let info_path = info.join(format!("{name}.trashinfo"));
    let body = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode(&recorded.to_string_lossy()),
        local_iso8601()
    );
    std::fs::write(&info_path, body).map_err(|e| FsError::io(&info_path, e))?;

    let dest = files.join(&name);
    if let Err(err) = std::fs::rename(&path, &dest) {
        // Leaving the info file behind would claim something is in the trash that is not.
        let _ = std::fs::remove_file(&info_path);
        return Err(FsError::io(&path, err));
    }
    tracing::info!(from = %path.display(), to = %dest.display(), dir = ?meta.is_dir(), "trashed");
    Ok(dest)
}

/// `$XDG_DATA_HOME`, or `~/.local/share`.
fn data_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    home.join(".local/share")
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|e| FsError::io(path, e))?;
    Ok(cwd.join(path))
}

/// Whether a path and a trash directory are on the same filesystem.
///
/// The trash directory may not exist yet, so the nearest existing ancestor is what gets
/// compared — creating it first would leave `~/.local/share/Trash` behind on a machine where
/// the answer turns out to be "no".
fn same_device(path: &Path, trash: &Path) -> Result<bool> {
    let a = device_of(path)?;
    let mut candidate = trash;
    loop {
        if candidate.exists() {
            return Ok(device_of(candidate)? == a);
        }
        match candidate.parent() {
            Some(parent) => candidate = parent,
            None => return Ok(false),
        }
    }
}

#[cfg(unix)]
fn device_of(path: &Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path)
        .map(|m| m.dev())
        .map_err(|e| FsError::io(path, e))
}

/// The mount point a path lives on: walk up while the device number stays the same.
#[cfg(unix)]
fn top_dir(path: &Path) -> Result<PathBuf> {
    let dev = device_of(path)?;
    let mut best = path.to_path_buf();
    let mut cur = path;
    while let Some(parent) = cur.parent() {
        match device_of(parent) {
            Ok(d) if d == dev => {
                best = parent.to_path_buf();
                cur = parent;
            }
            // A different device, or an unreadable parent: the previous one was the mount.
            _ => break,
        }
    }
    Ok(best)
}

#[cfg(unix)]
fn uid() -> u32 {
    // SAFETY: `getuid` takes no arguments, touches no memory and cannot fail.
    unsafe { libc::getuid() }
}

/// A name that is free in both `files/` and `info/`.
///
/// Both, because the spec requires the pair to stay in step: a name free in one and taken in
/// the other would either orphan an entry or overwrite someone else's record of it.
fn unique_name(files: &Path, info: &Path, source: &Path) -> Result<String> {
    let base = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| FsError::InvalidPath(source.display().to_string()))?;

    for n in 0..10_000u32 {
        let candidate = if n == 0 {
            base.clone()
        } else {
            match base.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => format!("{stem}.{n}.{ext}"),
                _ => format!("{base}.{n}"),
            }
        };
        if !files.join(&candidate).exists() && !info.join(format!("{candidate}.trashinfo")).exists()
        {
            return Ok(candidate);
        }
    }
    Err(FsError::TrashFull(base))
}

/// Percent-encode for a `.trashinfo` `Path=` value.
///
/// The spec points at RFC 2396: everything outside the unreserved set is escaped, except
/// `/`, which stays a separator.
fn percent_encode(s: &str) -> String {
    const UNRESERVED_EXTRA: &[u8] = b"-_.!~*'()/";
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED_EXTRA.contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Local time as `YYYY-MM-DDThh:mm:ss`, which is the format the spec names.
///
/// UTC would be wrong here in a way that shows: a file trashed at 9am appears in the desktop
/// trash as 7am, and the "restore" order is what a user reads to find the thing they just
/// deleted.
#[cfg(unix)]
fn local_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` writes into `tm` and reads `now`; both are live, and the
    // reentrant form is the one that is safe to call from any thread.
    let ok = unsafe { !libc::localtime_r(&now, &mut tm).is_null() };
    if !ok {
        return "1970-01-01T00:00:00".to_string();
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    #[test]
    fn a_file_moves_into_files_with_a_matching_info_record() {
        let dir = scratch("trash-basic");
        let data_home = dir.join("data");
        let victim = dir.join("notes.txt");
        std::fs::write(&victim, "keep me").unwrap();

        let dest = move_to_trash_in(&data_home, &victim).unwrap();

        assert!(!victim.exists(), "the original should be gone");
        assert_eq!(dest, data_home.join("Trash/files/notes.txt"));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "keep me");

        let info =
            std::fs::read_to_string(data_home.join("Trash/info/notes.txt.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\n"), "{info}");
        assert!(
            info.contains(&format!(
                "Path={}\n",
                percent_encode(&victim.to_string_lossy())
            )),
            "{info}"
        );
        assert!(info.contains("DeletionDate=20"), "{info}");
    }

    #[test]
    fn a_second_file_of_the_same_name_does_not_overwrite_the_first() {
        let dir = scratch("trash-collide");
        let data_home = dir.join("data");
        for _ in 0..3 {
            std::fs::write(dir.join("a.rs"), "x").unwrap();
            move_to_trash_in(&data_home, &dir.join("a.rs")).unwrap();
        }
        let files = data_home.join("Trash/files");
        assert!(files.join("a.rs").exists());
        assert!(files.join("a.1.rs").exists());
        assert!(files.join("a.2.rs").exists());
        assert!(data_home.join("Trash/info/a.2.rs.trashinfo").exists());
    }

    #[test]
    fn a_directory_goes_whole() {
        let dir = scratch("trash-dir");
        let data_home = dir.join("data");
        std::fs::create_dir_all(dir.join("src/inner")).unwrap();
        std::fs::write(dir.join("src/inner/x.rs"), "").unwrap();

        move_to_trash_in(&data_home, &dir.join("src")).unwrap();
        assert!(!dir.join("src").exists());
        assert!(data_home.join("Trash/files/src/inner/x.rs").exists());
    }

    #[test]
    fn a_missing_file_is_an_error_and_not_a_silent_success() {
        let dir = scratch("trash-missing");
        let err = move_to_trash_in(&dir.join("data"), &dir.join("nope")).unwrap_err();
        assert!(matches!(err, FsError::Io { .. }), "{err:?}");
    }

    #[test]
    fn percent_encoding_matches_the_spec_examples() {
        assert_eq!(percent_encode("/home/user/x.rs"), "/home/user/x.rs");
        assert_eq!(percent_encode("/tmp/a b"), "/tmp/a%20b");
        assert_eq!(percent_encode("/tmp/#1"), "/tmp/%231");
        assert_eq!(percent_encode("/tmp/é"), "/tmp/%C3%A9");
    }

    #[test]
    fn the_mount_point_of_a_path_is_an_ancestor_on_the_same_device() {
        let dir = scratch("trash-topdir");
        let top = top_dir(dir.path()).unwrap();
        assert!(dir.starts_with(&top), "{top:?} should contain {dir:?}");
        assert_eq!(device_of(&top).unwrap(), device_of(dir.path()).unwrap());
    }

    #[test]
    fn the_timestamp_is_iso_8601_local_time() {
        let stamp = local_iso8601();
        assert_eq!(stamp.len(), 19, "{stamp}");
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
        assert!(stamp.starts_with("20"), "{stamp}");
    }
}
