//! IDEA-style git: multi-root status, hunk and line staging via `Repository::apply(ApplyLocation::Index)`, changelists, shelf, stash. (M10)
//!
//! # The one rule this crate exists to keep
//!
//! **This is the only code in the project where a bug destroys uncommitted work.** Every
//! design decision below trades convenience for that.
//!
//! ## Whole files never go through patch synthesis
//!
//! [`Selection::Whole`] is executed with the index API — `add_path` / `remove_path` — which
//! is exactly what `git add` does and is byte-exact by construction. Binary files, mode
//! changes, symlinks, submodules, deletions and renames are therefore never described by a
//! patch we wrote, because those are precisely the cases where a synthesized unified diff is
//! *silently* wrong rather than loudly wrong. Ask for a partial selection on one of them and
//! [`patch`] refuses with a typed [`cide_ipc::git::PartialRefusal`] instead of guessing.
//!
//! A selection that happens to cover every line of every hunk is normalised to `Whole`
//! before anything else looks at it, so the common "tick the file row" gesture never touches
//! the dangerous path at all.
//!
//! ## The dangerous path is verified against the real `git`
//!
//! [`patch::synthesize`] emits its lines the way libgit2's own printer does — origin
//! character for context/addition/deletion, content verbatim for the
//! `\ No newline at end of file` marker — and recomputes hunk headers the way `git add -p`
//! does. `tests/patch_props.rs` generates random working trees and random selections, feeds
//! the same synthesized patch to `Repository::apply(ApplyLocation::Index)` and to
//! `git apply --cached`, and asserts the two resulting indexes are byte-identical.
//!
//! ## Two staging models, one mechanism
//!
//! * **Changelists are the truth** (default). `.git/index` is a derived artifact. Committing
//!   resets it to HEAD and rebuilds it from the changelist being committed, so committing
//!   one changelist cannot pick up another's files. See [`commit`].
//! * **Staging area** ([`changelist::Sidecar::use_staging_area`]) is IDEA's own opt-out. The
//!   index is the truth, `stage`/`unstage` edit it, and commit takes it as it stands. cide
//!   never rebuilds a hand-built index in this mode.
//!
//! Both share [`patch::synthesize`]; they differ only in which side is "old" — HEAD for a
//! changelist commit, the index for staging.
//!
//! ## The external-staging guard
//!
//! Bash panes inside cide are exactly where a user runs `git add`. Every write cide makes to
//! the index records a fingerprint of it in the sidecar; every commit compares first and
//! reports [`cide_ipc::git::GitError::IndexChangedExternally`] rather than clobbering. See
//! [`changelist::fingerprint`].
//!
//! ## No cached state
//!
//! Every entry point opens what it needs and drops it. A `Repository` handle cached across
//! calls goes stale the moment anything outside cide touches the repo — and inside cide,
//! something always does. Opening costs a few hundred microseconds; a stale index costs the
//! user their work.

pub mod blame;
pub mod branch;
pub mod changelist;
pub mod commit;
pub mod conflict;
pub mod diff;
pub mod lanes;
pub mod log;
pub mod merge;
pub mod patch;
/// The git half of the properties card: what this repository remembers about one path. (M70)
pub mod properties;
pub mod pull;
pub mod push;
pub mod replay;
pub mod repo;
pub mod reset;
pub mod revision;
pub mod shelf;
pub mod show;
pub mod sidecar;
pub mod stage;
pub mod stash;
pub mod status;
pub mod tag;
pub mod tree_status;
pub mod worktree;

use cide_ipc::git::GitError;

pub use cide_ipc::git::Selection;
pub use worktree::{AgentWorktree, Integration};

pub type Result<T> = std::result::Result<T, GitError>;

/// Convert a foreign error into the wire error type.
///
/// `From` cannot do this: both `GitError` (in `cide-ipc`) and `git2::Error` are foreign to
/// this crate, so the orphan rule rules out the blanket impl and `?` with it. An extension
/// trait keeps the call sites to one word.
pub(crate) trait Wrap<T> {
    fn wrap(self) -> Result<T>;
}

impl<T> Wrap<T> for std::result::Result<T, git2::Error> {
    fn wrap(self) -> Result<T> {
        self.map_err(|e| GitError::Git {
            // The class is half the diagnosis — "not found" from `Reference` and from
            // `Index` are different bugs — and `Display` alone does not print it.
            detail: format!("{:?}/{:?}: {}", e.class(), e.code(), e.message()),
        })
    }
}

impl<T> Wrap<T> for std::io::Result<T> {
    fn wrap(self) -> Result<T> {
        self.map_err(|e| GitError::Io {
            detail: e.to_string(),
        })
    }
}
