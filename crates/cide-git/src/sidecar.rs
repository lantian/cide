//! Where per-repository state lives on disk, and how it gets there intact.
//!
//! `$XDG_STATE_HOME/cide/repos/<blake3(root)>/`
//! ├── `changelists.json`  — changelists, the active one, the index fingerprint
//! └── `shelf/` — `index.json` plus one `<id>.patch` per shelved change
//!
//! State rather than config: these are written by the app, keyed by a hash nobody types, and
//! do not belong in a dotfile repository. The base directory comes from `cide-core::persist`
//! so there is one answer to "where does cide keep things" rather than two that can drift.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cide_ipc::git::GitError;

use crate::Result;
use crate::repo::repo_key;

/// `$XDG_STATE_HOME/cide/repos/<blake3(root)>`.
pub fn repo_dir(root: &Path) -> PathBuf {
    cide_core::persist::state_dir()
        .join("repos")
        .join(repo_key(root))
}

pub fn changelists_path(root: &Path) -> PathBuf {
    repo_dir(root).join("changelists.json")
}

pub fn shelf_dir(root: &Path) -> PathBuf {
    repo_dir(root).join("shelf")
}

/// Read a file, treating absence as `None` rather than as an error.
pub fn read_opt(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(sidecar_err(path, e)),
    }
}

/// Write `bytes` to `path` so a crash leaves either the old file or the new one.
///
/// The temp file is a sibling of the target because `rename` is only atomic within one
/// filesystem, and the system temp directory is routinely a different one. The pid and
/// counter in its name keep two writers from truncating each other — sharing a temp name is
/// not a lost race but a corrupt file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|e| sidecar_err(dir, e))?;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sidecar".into());
    let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_file_name(format!(".{name}.{}.{nonce}.tmp", std::process::id()));

    let write = (|| -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        // Without this the rename can publish an intact name over contents that never
        // reached the disk — exactly the crash this function exists to survive.
        file.sync_all()
    })();
    if let Err(e) = write {
        let _ = fs::remove_file(&tmp);
        return Err(sidecar_err(path, e));
    }

    if let Err(e) = fs::rename(&tmp, path) {
        // Leaving it behind would accumulate one temp file per failed write.
        let _ = fs::remove_file(&tmp);
        return Err(sidecar_err(path, e));
    }
    Ok(())
}

pub fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(sidecar_err(path, e)),
    }
}

pub fn sidecar_err(path: &Path, e: impl std::fmt::Display) -> GitError {
    GitError::Sidecar {
        path: path.display().to_string(),
        detail: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_round_trips_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("cide-sidecar-{}", std::process::id()));
        let path = dir.join("thing.json");
        write_atomic(&path, b"{\"a\":1}").unwrap();
        assert_eq!(read_opt(&path).unwrap().as_deref(), Some(&b"{\"a\":1}"[..]));

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files were left behind");

        write_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(read_opt(&path).unwrap().as_deref(), Some(&b"{\"a\":2}"[..]));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        assert!(
            read_opt(Path::new("/tmp/cide-definitely-not-here.json"))
                .unwrap()
                .is_none()
        );
    }
}
