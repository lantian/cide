//! Errors this crate hands to the IPC layer.
//!
//! Tagged variants rather than strings, for the same reason `cide_core::CoreError` is: the
//! frontend reacts differently to "that file is not text" (offer a hex view) and "that path
//! is gone" (drop the row), and telling them apart by matching on prose is how error
//! handling rots.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
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
