//! The app's handle on the domain.
//!
//! `cide-core` owns the workspace tree and knows nothing about Tauri; this is the piece
//! that gives it a home in the running process — one lock, one file path, and the debounce
//! that decides when a burst of mutations becomes a write.
//!
//! Mutation goes through [`WorkspaceState::update`] rather than by handing out the guard,
//! so that advancing `rev`, re-validating and marking the file dirty cannot be forgotten at
//! a call site. A command handler that could skip validation would eventually skip it.

use std::path::PathBuf;

use cide_core::persist::{self, Debouncer};
use cide_core::workspace;
use cide_ipc::Workspace;
use parking_lot::Mutex;
use std::sync::OnceLock;
use tauri::AppHandle;

pub struct WorkspaceState {
    inner: Mutex<Workspace>,
    path: PathBuf,
    debounce: Debouncer,
    /// Set once, during setup, so mutations can broadcast to every window.
    ///
    /// A `OnceLock` rather than a constructor argument because the state is created before
    /// the Tauri app exists — it is loaded from disk so that the very first
    /// `app.get_bootstrap` answers from a real tree rather than from defaults it then has to
    /// correct.
    app: OnceLock<AppHandle>,
}

impl WorkspaceState {
    /// Load from disk, or start from defaults.
    ///
    /// Never fails: `persist::load` answers with defaults for anything it cannot read, so a
    /// broken file costs a layout rather than a launch.
    pub fn load() -> Self {
        let path = persist::workspace_path();
        let mut workspace = persist::load(&path);

        // A file that parses can still be internally inconsistent — hand-edited, or written
        // by a build whose invariants differed. Starting fresh is recoverable; running on a
        // tree that fails its own validator is not.
        if let Err(error) = workspace::validate(&workspace) {
            tracing::warn!(%error, "loaded workspace failed validation; starting from defaults");
            workspace = Workspace::default();
        }

        Self {
            inner: Mutex::new(workspace),
            path,
            debounce: Debouncer::new(persist::SAVE_DEBOUNCE),
            app: OnceLock::new(),
        }
    }

    /// Give the state a handle to broadcast through. Called once, from `setup`.
    pub fn attach_app(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    /// Read the tree. Clones, because a `MutexGuard` must not be held across an await or
    /// handed to serde while another command waits on it.
    pub fn snapshot(&self) -> Workspace {
        self.inner.lock().clone()
    }

    /// Decide something about the tree without another mutation landing mid-decision.
    ///
    /// `snapshot` is enough for anything that only reads. This exists for decisions that
    /// also consult state outside the workspace and must agree with it — window reconciling
    /// compares the workspace's window roles against the OS windows that actually exist, and
    /// between a snapshot and that enumeration a detach can commit and open a window, which
    /// then looks like one the workspace never named.
    ///
    /// The closure must not mutate, block, or call back into this state. It runs under the
    /// lock, so anything that re-enters deadlocks.
    pub fn with<T>(&self, f: impl FnOnce(&Workspace) -> T) -> T {
        f(&self.inner.lock())
    }

    pub fn rev(&self) -> u64 {
        self.inner.lock().rev
    }

    /// Mutate under the lock, then validate and mark the file dirty.
    ///
    /// A mutation that leaves the tree invalid is rolled back and reported: `cide-core`'s
    /// operations are individually invariant-preserving, so reaching here means either a
    /// compound edit that was not, or a bug — and neither should be persisted.
    pub fn update<T>(
        &self,
        f: impl FnOnce(&mut Workspace) -> cide_core::Result<T>,
    ) -> cide_core::Result<T> {
        let mut guard = self.inner.lock();
        let before = guard.clone();

        let outcome = f(&mut guard);
        if outcome.is_ok()
            && let Err(error) = workspace::validate(&guard)
        {
            *guard = before;
            tracing::error!(%error, "rejected a mutation that broke a workspace invariant");
            return Err(error);
        }
        if outcome.is_err() {
            // An operation that reports failure should not have changed anything, but
            // restoring costs one clone and removes the question.
            *guard = before;
            return outcome;
        }

        self.debounce.note_change();

        // Broadcast while still holding the lock, so two concurrent mutations cannot emit
        // their snapshots in the opposite order to the one they were applied in. The clone
        // is the price of that; the alternative is a frontend that can be told about rev 8
        // and then rev 7 and has to sort it out from the number alone.
        let snapshot = guard.clone();
        drop(guard);
        if let Some(app) = self.app.get() {
            crate::emit::workspace_changed(app, &snapshot);
        }
        outcome
    }

    /// Write if the debounce has elapsed. Called from the app's tick and before quitting.
    pub fn flush_if_due(&self) {
        if self.debounce.take() {
            self.write_now();
        }
    }

    /// Write unconditionally. Used on quit, where waiting out a debounce would lose the
    /// last change the user made.
    pub fn write_now(&self) {
        let workspace = self.inner.lock().clone();
        if let Err(error) = persist::save_atomic(&self.path, &workspace) {
            tracing::error!(path = %self.path.display(), %error, "failed to save the workspace");
        }
    }
}
