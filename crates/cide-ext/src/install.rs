//! Copying an extension out of a clone, and removing it again.
//!
//! # Why a copy and not a symlink
//!
//! A symlink into the clone is one line and is wrong in a way that is invisible until it bites.
//! `refresh` hard-resets the clone onto the remote head, so a marketplace publishing a commit
//! would change the code of every installed extension **under a running app** — new panels, a new
//! grammar, and, worst, a new `capabilities` list that the user never approved. Copying makes the
//! installed tree a thing the user chose, at a commit that is recorded, and an update an explicit
//! gesture with a diff behind it.
//!
//! It also makes uninstall answerable. Removing a symlink leaves the code; removing a directory
//! this module created removes exactly what this module wrote and nothing else.
//!
//! # What is copied
//!
//! Everything under the extension's directory except `.git` and dot-directories, up to a size and
//! a file count. The limits are not a security boundary — the code is about to run in a worker
//! with no DOM and no `invoke`, which is where the boundary is — they are a guard against a
//! mistake: a marketplace that checks a `node_modules` into an extension folder should fail to
//! install with a sentence, not fill `$XDG_STATE_HOME` and then fail to start.

use std::path::{Path, PathBuf};

/// The most one extension may weigh.
///
/// Generous for what an extension is — a manifest, a worker module, a README — and far under
/// anything that would be a surprise on disk. An extension that needs more than this is shipping
/// something that is not source.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// The most files one extension may have.
pub const MAX_FILES: usize = 2_000;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0}")]
    Io(String),
    #[error(
        "this extension is {0} bytes, and cide installs at most {MAX_BYTES}. That is usually a \
         build directory or a `node_modules` committed by accident."
    )]
    TooBig(u64),
    #[error(
        "this extension has {0} files, and cide installs at most {MAX_FILES}. That is usually a \
         build directory or a `node_modules` committed by accident."
    )]
    TooMany(usize),
    #[error("`{0}` is not a directory in this marketplace.")]
    Missing(String),
}

type Result<T> = std::result::Result<T, InstallError>;

fn io(error: impl std::fmt::Display) -> InstallError {
    InstallError::Io(error.to_string())
}

/// Copy `src` to `dest`, replacing whatever is there.
///
/// Not atomic, and it does not pretend to be: the write goes to a sibling `<dest>.incoming` and is
/// renamed over `dest` only once the whole tree is on disk, so a failure part way through leaves
/// the previous install intact rather than a half-replaced one. That is the same shape
/// `persist::write_atomic` uses for a single file and it is worth the twenty lines here for the
/// same reason — an extension whose manifest is the new version and whose worker is the old one
/// fails in a way nobody can read.
pub fn install(src: &Path, dest: &Path) -> Result<()> {
    if !src.is_dir() {
        return Err(InstallError::Missing(src.display().to_string()));
    }
    let (bytes, files) = measure(src)?;
    if bytes > MAX_BYTES {
        return Err(InstallError::TooBig(bytes));
    }
    if files > MAX_FILES {
        return Err(InstallError::TooMany(files));
    }

    let staging = staging_path(dest);
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(io)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    if let Err(error) = copy_tree(src, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    // The old tree is moved aside rather than deleted, so the window in which neither exists is
    // a rename rather than a recursive delete.
    let outgoing = outgoing_path(dest);
    let _ = std::fs::remove_dir_all(&outgoing);
    let had_previous = dest.exists();
    if had_previous {
        std::fs::rename(dest, &outgoing).map_err(io)?;
    }
    if let Err(error) = std::fs::rename(&staging, dest) {
        if had_previous {
            let _ = std::fs::rename(&outgoing, dest);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(io(error));
    }
    let _ = std::fs::remove_dir_all(&outgoing);
    Ok(())
}

/// Remove an installed extension.
///
/// A missing directory is success, not an error: uninstalling something that is already gone is
/// what the user asked for, and reporting it as a failure would leave a row nobody can clear.
pub fn uninstall(dest: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dest) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(error)),
    }
}

fn staging_path(dest: &Path) -> PathBuf {
    sibling(dest, ".incoming")
}

fn outgoing_path(dest: &Path) -> PathBuf {
    sibling(dest, ".outgoing")
}

/// A sibling of `dest` with a suffix — same directory, so the rename is within one filesystem.
fn sibling(dest: &Path, suffix: &str) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "extension".to_string());
    dest.with_file_name(format!("{name}{suffix}"))
}

/// Whether a directory entry is copied at all.
///
/// `.git` above all — copying a submodule's repository into `$XDG_STATE_HOME` would be surprising
/// and large — and every other dot-entry with it, because an extension's `.github/`, `.vscode/`
/// and `.eslintrc` are its author's tooling and are of no use to cide.
fn skip(name: &str) -> bool {
    name.starts_with('.')
}

fn measure(src: &Path) -> Result<(u64, usize)> {
    let mut bytes = 0u64;
    let mut files = 0usize;
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if skip(&name) {
                continue;
            }
            // `file_type` and not `metadata`: it does not follow a symlink, so a link pointing at
            // `/` is one skipped entry rather than a walk of the whole filesystem.
            let kind = entry.file_type().map_err(io)?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                files += 1;
                bytes = bytes.saturating_add(entry.metadata().map_err(io)?.len());
                if bytes > MAX_BYTES {
                    return Ok((bytes, files));
                }
                if files > MAX_FILES {
                    return Ok((bytes, files));
                }
            }
        }
    }
    Ok((bytes, files))
}

fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).map_err(io)?;
    for entry in std::fs::read_dir(src).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if skip(&name) {
            continue;
        }
        let kind = entry.file_type().map_err(io)?;
        // Symlinks are not copied and not followed. A link is the one entry whose *target* is
        // decided by the marketplace rather than by this walk, and `cide-ext://` resolves against
        // the installed tree — a link out of it would be a hole in the jail that the jail itself
        // could not see, because the path it checks would still be inside.
        if kind.is_symlink() {
            continue;
        }
        let to = dest.join(&name);
        if kind.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &to).map_err(io)?;
        }
    }
    Ok(())
}
