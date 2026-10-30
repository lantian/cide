//! Local History's store: every version of a project's files, kept apart from git.
//!
//! This crate is the disk. It decides what is durable and in what order, what a version is, and
//! what retention removes. It owns no thread and reads no project file: `cide-app` watches the
//! project, reads what changed and hands the bytes here. That split is `cide-tasks`' and it pays
//! the same way, since every rule below can be driven from an ordinary `#[test]`.
//!
//! The decisions a refactor would otherwise undo are in `docs/adr/0016-local-history-store.md`.
//!
//! # Where a store lives
//!
//! `<state>/local-history/<key>/`, where `key` is `cide_core::scratch::key`, the blake3 of the
//! canonical primary root. **Not** in the project, and not because that would be harder. The
//! moments this store exists for are the moments a project directory cannot be trusted: a
//! `git clean -fdx`, a deleted folder, an agent that ran a command it should not have. A copy
//! inside the thing that was just destroyed is not a copy.
//!
//! The cost is that moving a project starts a new history, because the key follows the path. The
//! old store stays where it was, beside an origin record that names the project it belonged to,
//! exactly as a scratch drawer does.

mod log;
mod pack;
mod retention;
mod store;

use std::path::{Path, PathBuf};

pub use cide_ipc::local_history::{VersionKind, VersionSource};
pub use pack::BlobHandle;
pub use retention::{BASELINE_SHARE, Policy};
pub use store::{
    Committed, Compacted, Entry, GroupSummary, Head, PlannedFile, RootId, Stamp, Stats, Store,
    TargetState, Version, exists,
};

/// A blake3 digest.
pub type Hash = [u8; 32];

/// The row ceiling. A row costs memory whether or not it holds bytes, so this is a bound beside
/// the storage cap: at about 60 bytes each plus interned paths, two million is a few hundred MB
/// for one project, which is the most this should ever be allowed to cost.
pub const MAX_ROWS: usize = 2_000_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Another process holds the store. It can still be read.
    #[error("Local History for this project is open elsewhere, so it can only be read here")]
    ReadOnly,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// This version's bytes are not the ones stored, and restoring them would write garbage.
    #[error("this version's content is damaged: {0}")]
    Corrupt(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Where every store lives. Under a profile this is `cide-<profile>`, because `state_dir` says so.
pub fn root_dir() -> PathBuf {
    cide_core::persist::state_dir().join("local-history")
}

/// A project's store directory. Does not create it: see [`exists`].
pub fn dir_for(primary_root: &Path) -> PathBuf {
    root_dir().join(cide_core::scratch::key(primary_root))
}

/// The record beside a store naming the project it belongs to.
fn origin_path(primary_root: &Path) -> PathBuf {
    root_dir().join(format!("{}.json", cide_core::scratch::key(primary_root)))
}

/// Leave a breadcrumb for a human who finds an orphaned store. Failing is ignored: it is a
/// courtesy, and it must not be the reason versions stop being kept.
pub fn write_origin(primary_root: &Path) {
    let record = serde_json::json!({ "root": primary_root.display().to_string() });
    let path = origin_path(primary_root);
    if let Ok(bytes) = serde_json::to_vec_pretty(&record)
        && let Err(error) = cide_core::persist::write_atomic(&path, &bytes)
    {
        tracing::debug!(%error, "could not write the Local History origin record");
    }
}

/// A timestamp's local calendar day, as a number that increases by one per day.
///
/// Local, because that is what "a day you worked on it" means to the person reading the list, and
/// what a day group in it is drawn from.
pub fn local_day(ts_ms: u64) -> i32 {
    use chrono::{Datelike as _, TimeZone as _};
    chrono::Local
        .timestamp_millis_opt(ts_ms as i64)
        .earliest()
        .map_or((ts_ms / 86_400_000) as i32, |time| {
            time.date_naive().num_days_from_ce()
        })
}
