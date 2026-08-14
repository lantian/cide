//! Errors this crate hands to the IPC layer.
//!
//! Shaped exactly like [`cide_fs::FsError`] and for the same reason: tagged variants rather than
//! strings, so the frontend reacts to a *tag* and never to prose.
//!
//! # Why this does not derive `TS`
//!
//! `FsError` does not either, and the reason is worth restating rather than rediscovering:
//! `ts-rs` exports per crate, and `cargo xtask codegen` reads **only**
//! `crates/cide-ipc/bindings`. A `#[derive(TS)]` here would write into
//! `crates/cide-lang/bindings`, which nothing ever reads — dead output that looks like a wire
//! contract. So the frontend hand-writes a narrow structural predicate against the tag, the way
//! `ui/src/store/fileIndex.ts::isNoIndex` already does for `FsError::NoIndex`.
//!
//! [`SymbolError::NotIndexed`] is deliberately that same shape, because a symbol picker needs
//! the identical distinction the file picker needs: "the walk has not started" is not "the walk
//! found nothing", and a picker that renders the first as the second tells the user their
//! project has no functions in it.

use serde::Serialize;

// `rename_all_fields` as well as `rename_all`, following `FsError`: the former renames struct
// variants' fields, which `rename_all` alone leaves in snake_case.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    content = "detail"
)]
pub enum SymbolError {
    /// Nothing has built this project's symbol index yet — the walk was never started, or the
    /// project is closed.
    ///
    /// Not a failure. It is the "nobody looked" state, and it is what makes the picker show
    /// *Indexing…* instead of an empty list.
    #[error("no symbol index for this project")]
    NotIndexed,

    /// The path is outside every root of the project it was asked about.
    #[error("{0} is outside this project")]
    Outside(String),

    #[error("{path}: {message}")]
    Io { path: String, message: String },
}
