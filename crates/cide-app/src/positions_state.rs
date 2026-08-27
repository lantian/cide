//! Per-file view memory, and the four stages that keep a scroll gesture off the disk. (M12)
//!
//! # The write-amplification ladder
//!
//! A scroll fires continuously. A disk write does not. Between them are four stages, each an
//! order of magnitude cheaper than the next, and every one of them is load-bearing:
//!
//! 1. **Per frame, nothing crosses anything.** A CodeMirror `ViewPlugin` listens on
//!    `view.scrollDOM` and coalesces into one `requestAnimationFrame`. See
//!    `ui/src/editor/EditorSurface.tsx`.
//! 2. **Per gesture, one IPC call.** `panes/EditorPane.tsx` trailing-debounces that at 500 ms
//!    and calls `file_note_position` — the same shape, and the same reasoning, as the
//!    selection notification beside it.
//! 3. **Per few seconds, one disk write.** This module. Its own [`Debouncer`] at
//!    [`POSITION_DEBOUNCE`], its own `write_atomic`, and — the part that matters —
//!    **no `rev` bump and no event**. Going through `WorkspaceState::update` instead would
//!    clone the whole workspace twice, re-run `workspace::validate`, and broadcast the tree to
//!    every window, where each webview replaces its mirror. Per scroll. That is not a tuning
//!    problem, it is a different application.
//! 4. **On quit, a flush.** `lifecycle::shutdown`, beside `WorkspaceState::write_now`.
//!
//! Two seconds rather than the workspace's 500 ms because this data is worth less: a lost
//! layout has to be rebuilt by hand, a lost scroll position is re-derived by looking at the
//! file. It is the cheaper thing to lose, so it is the one that waits longer.
//!
//! # Why the store polls itself instead of hanging off a tick
//!
//! There is no app tick. `WorkspaceState::flush_if_due` documents itself as "called from the
//! app's tick and before quitting" and **is called from neither** — `grep -rn flush_if_due`
//! finds the definition and one mention inside a comment. So `workspace.json` is in fact
//! written exactly once per run, on shutdown, and its debounce is inert. That is a real defect
//! (recorded in `docs/journal.md`) and it is *not* fixed by pretending a tick exists here: this store
//! flushes from the thread that already owns the write, on a timer it starts itself, so its
//! debounce works whether or not anyone ever builds that tick.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cide_core::persist::{self, Debouncer};
use cide_ipc::ViewPosition;
use parking_lot::Mutex;

/// How long a burst of position notes accumulates before it reaches the disk.
///
/// Four times the workspace's debounce, deliberately — see the module header.
pub const POSITION_DEBOUNCE: Duration = Duration::from_secs(2);

/// How often the flusher thread wakes to ask whether a write is owed.
///
/// A poll rather than a per-note timer: notes arrive from any of several webviews on any of
/// several threads, and spawning or rescheduling a timer per note is exactly the per-gesture
/// cost this whole module exists to avoid. Half the debounce, so a due write waits at most
/// 1 s past its deadline.
const POLL: Duration = Duration::from_secs(1);

pub struct PositionsState {
    inner: Mutex<Vec<ViewPosition>>,
    path: PathBuf,
    debounce: Debouncer,
}

impl PositionsState {
    /// Load from disk, or start empty. Never fails — see `persist::load_positions`.
    pub fn load() -> Self {
        let path = persist::positions_path();
        let list = persist::load_positions(&path);
        Self {
            inner: Mutex::new(list),
            path,
            debounce: Debouncer::new(POSITION_DEBOUNCE),
        }
    }

    /// Start the background flusher. Call once, from `setup`.
    ///
    /// A thread rather than a Tauri async task because the work is a blocking file write and
    /// this crate's one runtime belongs to the IDE server. It is a daemon: the process exits
    /// without joining it, which is safe because [`crate::lifecycle::shutdown`] performs the
    /// final write itself rather than relying on this loop to notice.
    pub fn start_flusher(self: &Arc<Self>) {
        let state = Arc::clone(self);
        thread::Builder::new()
            .name("cide-positions".into())
            .spawn(move || {
                loop {
                    thread::sleep(POLL);
                    state.flush_if_due();
                }
            })
            // A failure to spawn costs the debounce, not the feature: the shutdown flush still
            // runs, so a whole session's positions are written once at the end.
            .map(|_| ())
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "no positions flusher; positions will be written on quit only");
            });
    }

    /// Record where the user is in a file.
    ///
    /// Fire-and-forget from the frontend's point of view: nothing here can fail in a way the
    /// caller could act on, and a scroll must not be able to raise a dialog.
    pub fn note(&self, at: ViewPosition) {
        {
            let mut list = self.inner.lock();
            persist::remember_position(&mut list, at, persist::now_ms());
        }
        self.debounce.note_change();
    }

    /// The remembered place for `path`, or `None`.
    pub fn get(&self, path: &Path) -> Option<ViewPosition> {
        persist::position_for(&self.inner.lock(), path).cloned()
    }

    /// Follow files that moved on disk — `(from, to)` pairs, matched exactly or as a directory
    /// prefix. (M25)
    ///
    /// The record is keyed on an absolute path, so a rename orphans it: the tab that
    /// `workspace::retarget_paths` just moved asks for the *new* name, is told nothing is
    /// remembered, and the editor rebuilds at line 1. Renaming a file you are reading at line 400
    /// and being thrown to the top of it is the visible half of that; the invisible half is the
    /// orphan, which survives in `positions.json` until the LRU evicts it.
    ///
    /// Same rule as the tabs, via [`cide_core::workspace::moved_path`], because a second spelling
    /// of "is this path inside that folder" would be a second answer.
    ///
    /// No event, for the reason the module header gives about every write here: a position is
    /// read when a pane mounts and never pushed, and the pane that is about to ask has not been
    /// told the path moved yet either.
    pub fn rename(&self, moves: &[(PathBuf, PathBuf)]) {
        let mut changed = false;
        {
            let mut list = self.inner.lock();
            for at in list.iter_mut() {
                if let Some(next) = moves
                    .iter()
                    .find_map(|(from, to)| cide_core::workspace::moved_path(&at.path, from, to))
                {
                    at.path = next;
                    changed = true;
                }
            }
        }
        if changed {
            // Noted so the move reaches the disk on the ordinary debounce. Without it a rename
            // followed by a quit with no further scrolling would write the old paths back out.
            // Guarded, because most renames are of files nobody has ever scrolled: an
            // unconditional note would put a disk write behind every one of them.
            self.debounce.note_change();
        }
    }

    /// Write if the debounce has elapsed.
    pub fn flush_if_due(&self) {
        if self.debounce.take() {
            self.write_now();
        }
    }

    /// Write unconditionally. Used on quit, where waiting out the debounce would lose the last
    /// thing the user looked at — which, on the way out, is the one they are most likely to
    /// come back to.
    pub fn write_now(&self) {
        let list = self.inner.lock().clone();
        if let Err(error) = persist::save_positions(&self.path, &list) {
            tracing::error!(path = %self.path.display(), %error, "failed to save view positions");
        }
    }
}
