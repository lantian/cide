//! One project's synthetic groups: the row space they share, and the bookkeeping that keeps
//! each of them lazy.
//!
//! `cide_fs::groups::Groups` is the *mechanism* — a header row plus a lazily materialised
//! subtree, with no opinion about what is in it. This type is where a project's groups are held
//! together, and the two policies that fill them live one file away each:
//!
//! | file | group | filled by |
//! | --- | --- | --- |
//! | [`crate::libraries`] | *External Libraries* | `cargo metadata` / `go list`, on a thread |
//! | [`crate::scratches`] | *Scratches* | one `read_dir` of a directory cide owns |
//!
//! # Why one type rather than one per group
//!
//! Because there is exactly one row space. `cmd::fs::compose` lays the index's rows and the
//! groups' rows end to end and serves one window across the seam, so every group has to draw
//! into the same [`Groups`] behind the same lock, or the row arithmetic has three sources and
//! two of them are wrong. A `Libraries` that owned its own `Groups` was the shape this started
//! as, and adding a second group to it meant either a second row space (which the composition
//! cannot serve) or the second group living inside a type named after the first.
//!
//! So the split is: the *state* is here, and the *policy* — which id, which label, when it is
//! shown at all, what its placeholder says, what a failure reads like — is an `impl` block in
//! each policy's own module. Adding a third group is a third file plus one call in
//! [`ProjectGroups::prepare`] and touches nothing that already works.
//!
//! # What is deliberately not here
//!
//! No watching, no indexing, no ignore rules, no picker candidates. That is structural rather
//! than a rule somebody has to keep: `Groups` sits beside `cide_fs::Index`, so `dir_paths()`
//! (the watcher's list), the walk's sink (the picker's injector) and `Filter::build` cannot
//! reach a group's rows because there is no code that could. `cide_fs::groups`'s own header
//! enumerates the four couplings and what grafting a synthetic root into the index would have
//! cost.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cide_fs::groups::{Entry, Expanded, Groups};
use cide_ipc::{TreeRow, TreeRowKind};
use parking_lot::{Mutex, RwLock};

/// The row space, plus one field per policy that needs to remember something between calls.
///
/// The fields are `pub(crate)` and the two modules allowed to touch them are named in the
/// header above. Accessors would have been tidier and would have meant a method per field
/// whose only caller is the policy that owns it.
pub struct ProjectGroups {
    /// Every group's rows, in draw order. The one row space — see the header.
    pub(crate) rows: RwLock<Groups>,

    // --- External Libraries (see `crate::libraries`) -----------------------------------
    /// Has the manifest probe run? One atomic on every tree read after the first.
    pub(crate) probed: AtomicBool,
    /// A resolver thread is running. Stops a second expand — a second window, a collapse and
    /// re-expand — from forking a second `cargo`.
    pub(crate) resolving: AtomicBool,
    /// The manifests and lockfiles the last answer was computed from, and their `(len, mtime)`.
    /// See `cide_deps::stamp` for why this is not a watcher subscription.
    pub(crate) stamp: Mutex<cide_deps::Stamp>,

    // --- Scratches (see `crate::scratches`) --------------------------------------------
    /// Where this project's scratch files live, once the group has been shown.
    ///
    /// Remembered rather than recomputed because it is also the answer to "may `fs_rename`
    /// touch this path" — see [`ProjectGroups::writable_dirs`] — which is asked on every
    /// mutating file command, and recomputing it is a `canonicalize` and a hash each time.
    pub(crate) scratch_dir: Mutex<Option<PathBuf>>,
}

impl Default for ProjectGroups {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectGroups {
    pub fn new() -> Self {
        Self {
            rows: RwLock::new(Groups::new()),
            probed: AtomicBool::new(false),
            resolving: AtomicBool::new(false),
            stamp: Mutex::new(Vec::new()),
            scratch_dir: Mutex::new(None),
        }
    }

    /// Has [`Self::prepare`] not run yet?
    ///
    /// Asked *before* it so the ordinary call — every tree read after the first for this
    /// project — is one atomic load and does not even build the root list, which is a `Vec` of
    /// `PathBuf` clones per read. The race between this and the swap inside `prepare` is
    /// benign: `Groups::show` is idempotent and so is the drawer listing.
    pub fn needs_prepare(&self) -> bool {
        !self.probed.load(Ordering::Acquire)
    }

    /// Everything a tree read has to do before it answers, for every group, on every read.
    ///
    /// Guarded by one atomic so two windows racing the first tree read of a project draw one
    /// set of headers between them rather than two. It is here rather than in `fs_index`
    /// because a second window attaching to an already-indexed project never calls `fs_index`
    /// at all: its first contact with the project is `fs_tree_count`, and a `file.reveal` on a
    /// restored session's tab reaches `fs_reveal` first.
    ///
    /// **Order is draw order.** `Groups::show` appends, so *External Libraries* is asked first
    /// and *Scratches* sits under it — IDEA's order, and the right one here for a second
    /// reason: Scratches is the mutable group and belongs next to the user's own files at the
    /// bottom edge of the panel.
    pub fn prepare(&self, roots: &[PathBuf]) {
        if self.probed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.probe_libraries(roots);
        self.show_scratches(roots.first());
    }

    /// Total rows: every header, plus everything expanded under one.
    pub fn count(&self) -> usize {
        self.rows.read().count()
    }

    pub fn rows(&self, offset: usize, len: usize) -> Vec<TreeRow> {
        self.rows.read().rows(offset, len)
    }

    /// Speed search's second source. See [`Groups::match_rows`].
    pub fn match_rows(
        &self,
        needle: &cide_fs::speed::Needle,
        limit: usize,
    ) -> (Vec<cide_ipc::TreeMatch>, bool) {
        self.rows.read().match_rows(needle, limit)
    }

    pub fn kind_of(&self, path: &Path) -> Option<TreeRowKind> {
        self.rows.read().kind_of(path)
    }

    pub fn collapse(&self, path: &Path) -> Option<()> {
        self.rows.write().collapse(path)
    }

    pub fn reveal(&self, path: &Path) -> Option<usize> {
        self.rows.write().reveal(path)
    }

    /// Every path a group can hang a row from. See [`Groups::reveal_roots`].
    ///
    /// A read lock and a `Vec` of clones, and no resolution: this is asked once per attach and
    /// again on the `cide://fs-status` the resolver emits, and it must stay affordable enough
    /// that neither of those is a decision.
    pub fn reveal_roots(&self) -> Vec<PathBuf> {
        self.rows.read().reveal_roots()
    }

    /// One group's top-level rows, with their labels. See [`Groups::entries`].
    ///
    /// A read lock and a `Vec` of clones, and no resolution. The caller that wants a resolution
    /// asks for one — [`Self::resolve_now`] — and is the only place that decides it is worth
    /// blocking for.
    pub fn entries(&self, id: &str) -> Vec<Entry> {
        self.rows.read().entries(id)
    }

    /// Expand a header or a directory inside one, answering whether a resolution is now owed.
    pub fn expand(&self, path: &Path) -> Option<Expanded> {
        self.rows.write().expand(path)
    }

    /// Directories outside every project root that this project may still change things in.
    ///
    /// Today that is the scratch drawer and nothing else: *External Libraries* draws other
    /// people's source, and a rename in `~/.cargo/registry` would break every project on the
    /// machine that depends on the crate. `ProjectFs::writable_paths` concatenates this with
    /// the roots, and that list is what the mutating `fs_*` handlers check against instead of
    /// the roots alone.
    ///
    /// Empty until the scratch group has been shown, which is the first tree read. A mutation
    /// arriving before then is refused, and that is the safe direction: the only way to name a
    /// scratch path is to have been shown one.
    pub fn writable_dirs(&self) -> Vec<PathBuf> {
        self.scratch_dir.lock().iter().cloned().collect()
    }
}
