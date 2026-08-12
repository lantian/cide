//! Errors this crate hands to the IPC layer.
//!
//! Tagged variants rather than strings, for the same reason `cide_core::CoreError` is: the
//! frontend reacts differently to "that file is not text" (offer a hex view) and "that path
//! is gone" (drop the row), and telling them apart by matching on prose is how error
//! handling rots.

use std::path::Path;

use serde::Serialize;

// `rename_all_fields` as well as `rename_all`: the former renames the *fields* of the struct
// variants below, which `rename_all` alone leaves in snake_case. Every field here happens to
// be one word, so today it changes nothing — it is here so that the first two-word field
// somebody adds does not leak `snake_case` onto the wire, which has already happened once in
// this workspace.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    content = "detail"
)]
pub enum FsError {
    #[error("{path}: {message}")]
    Io { path: String, message: String },

    /// The project has no index — `fs.index` was never called, or the project is closed.
    #[error("no file index for this project")]
    NoIndex,

    #[error("not a valid path: {0}")]
    InvalidPath(String),

    /// The file is not UTF-8. The editor cannot open it and says so, rather than showing
    /// replacement characters and then writing them back over the user's data.
    #[error("{0} is not valid UTF-8")]
    NotUtf8(String),

    /// Refusing to read something enormous into a webview.
    #[error("{path} is {size} bytes, past the {limit}-byte limit for reading a file")]
    TooLarge { path: String, size: u64, limit: u64 },

    #[error("{0} already exists")]
    Exists(String),

    /// A path outside every project root. The frontend can only ever send a path it was
    /// given, so this is a bug or an attempt at one, and either way it does not proceed.
    #[error("{0} is outside this project")]
    OutsideProject(String),

    /// A move or delete aimed at a project root. Refused; see [`crate::ops::check_not_root`].
    #[error("{0} is a project root and cannot be moved or deleted from here")]
    IsRoot(String),

    /// A directory pasted into itself, or into something inside itself.
    ///
    /// Its own variant rather than prose inside [`FsError::InvalidPath`] because it is the one
    /// refusal in this file that is about a *pair* of paths rather than about one path being
    /// malformed — and because obeying it is not a refused gesture but an infinite recursion
    /// that fills the disk. The message names both halves.
    #[error("{0}")]
    IntoItself(String),

    /// A multi-path paste copied some sources and then failed.
    ///
    /// The same shape as [`FsError::PartialDelete`] and for the same reason: telling the
    /// frontend that nothing happened while three of five directories are already on disk
    /// leaves it drawing a tree it believes is unchanged. What did land is carried so the
    /// panel can say so; it is not rolled back, because rolling back a copy means deleting
    /// files the user can see and a cut has already removed its source.
    #[error("{error} (after pasting {} path(s))", pasted.len())]
    PartialPaste { pasted: Vec<String>, error: String },

    /// A multi-path delete moved some paths and then failed.
    ///
    /// A plain error here would tell the frontend that nothing happened while files were
    /// already in the trash, and it would leave their rows in the tree until the watcher
    /// caught up. The paths that did move are carried so the caller can say what it actually
    /// did — the moves themselves cannot be undone from here, which is the whole reason this
    /// variant exists instead of a bare `Io`.
    #[error("{error} (after moving {} path(s) to the trash)", trashed.len())]
    PartialDelete { trashed: Vec<String>, error: String },

    #[error("the trash already holds too many files named {0}")]
    TrashFull(String),
}

impl FsError {
    pub fn io(path: &Path, err: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            message: err.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, FsError>;
