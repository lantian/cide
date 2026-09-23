//! One `.cide/tasks.json` store per open project, and the thread that ticks them. (M18)
//!
//! # What lives here and what does not
//!
//! [`cide_tasks::TaskStore`] is the single owning actor for one project's tracker: every
//! mutation in the process — the panel's, and later every subagent's over the agent-RPC socket
//! — funnels through its one `Mutex`. What it deliberately does *not* own is a thread and an
//! `AppHandle`, for the reason its own module header gives: a timer in a domain crate is an
//! async runtime in a domain crate, and an `AppHandle` there is the signal that the logic is in
//! the wrong crate. This module is the app-side half that supplies both.
//!
//! So the shape is `IdeServers`/`DiagnosticsRegistry`'s: a `DashMap<ProjectId, _>` behind
//! managed state, with `ensure`/`get`/`close`, plus the two collective operations a registry of
//! files needs and a registry of processes does not — [`TasksStores::flush_all`] on the way out,
//! and the flusher.
//!
//! # Why the flusher exists, and the mistake it is written not to repeat
//!
//! There is no app tick, and for three milestones nothing noticed: `WorkspaceState::flush_if_due`
//! documented itself as "called from the app's tick and before quitting" and was called by
//! **neither**, so `workspace.json` was written exactly once per run, on shutdown, and its 500 ms
//! debounce was inert. `positions_state::PositionsState` answered that by starting its own thread
//! rather than by pretending a tick exists; this does the same, and in M73 the workspace itself
//! finally did too (`WorkspaceState::start_flusher`, which carries what the defect cost). Three
//! stores, three threads, no tick — which is the arrangement to keep, because each one's timer is
//! chosen for what its own file is worth.
//!
//! It matters more here than for either of those, because this store is the only one in the
//! process whose file **has writers cide does not control**. A `git pull`, a teammate's commit,
//! a rebase or a hand edit moves `.cide/tasks.json` under us, and
//! [`cide_tasks::TaskStore::write_now`] answers by re-reading and merging by rule. That merge is
//! the one moment the board changes with nobody in this process having asked for it, which is
//! exactly why `flush_if_due`/`write_now` hand back `Some(TaskFile)` when it happens — and why
//! this loop turns that `Some` into a `cide://tasks-changed`. Without the emit, a teammate's
//! three new tasks would sit in memory, correct and invisible, until something unrelated made
//! the panel re-ask.
//!
//! The *prompt* channel for those external moves is [`crate::dotcide`], which turns the
//! `.cide/tasks.json` watch event into `TaskStore::refresh_from_disk` within a debounce or two
//! of the write. This loop is the belt to that suspender — `write_now` reconciles before
//! *every* flush, so a change the watcher missed (degraded watcher, coalesced event) is still
//! folded in on the back of the next local mutation, exactly as before the router existed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cide_core::CoreError;
use cide_ipc::{ProjectId, TaskBoard};
use cide_tasks::TaskStore;
use dashmap::DashMap;
use tauri::AppHandle;

use crate::workspace_state::WorkspaceState;

/// How often the flusher wakes to ask every store whether a write is owed.
///
/// 250 ms, half `persist::SAVE_DEBOUNCE`, so a due write waits at most a quarter of a second
/// past its deadline — the same relationship `POSITION_DEBOUNCE`/`POLL` has one file over, and
/// the same reason for polling rather than arming a timer per mutation: mutations arrive from
/// several windows on several threads, and rescheduling a timer per mutation is exactly the
/// per-gesture cost the debounce exists to avoid.
const POLL: Duration = Duration::from_millis(250);

/// Every open project's task tracker.
#[derive(Default)]
pub struct TasksStores {
    stores: DashMap<ProjectId, Arc<TaskStore>>,
}

impl TasksStores {
    /// The tracker for a project, opening it if this is the first ask.
    ///
    /// Idempotent, like every other `ensure` in this crate, and called from four places for the
    /// reason `IdeServers::ensure` is: `project_open`, `project_close`'s sibling, the shutdown
    /// flush, and — the one that gets forgotten — `lib.rs`'s restore loop.
    ///
    /// `root` is the project's **first** root and not all of them: one project has one tracker,
    /// because the file is the medium a project's agents exchange state through and two of them
    /// in one project would be two conversations neither side can see. `roots[0]` is the
    /// directory the header names, so it is the one whose `.cide/` a user goes looking in.
    ///
    /// Never fails: [`TaskStore::open`] answers an unparseable file with an empty tracker and a
    /// `TaskBoard::Unreadable` the panel can explain, which is `persist::load`'s discipline — a
    /// broken tracker must not become a project that cannot open.
    pub fn ensure(&self, project: ProjectId, root: &Path) -> Arc<TaskStore> {
        if let Some(existing) = self.get(project) {
            return existing;
        }
        let store = Arc::new(TaskStore::open(root));
        tracing::debug!(%project, path = %store.path().display(), "task tracker opened");
        // `entry` rather than `insert`, so two windows opening the same project at once cannot
        // end up with two stores over one file — which would be two mutexes over one path, and
        // the whole single-owning-actor argument gone with them.
        self.stores.entry(project).or_insert(store).value().clone()
    }

    /// The tracker for a project, if one is open.
    pub fn get(&self, project: ProjectId) -> Option<Arc<TaskStore>> {
        self.stores.get(&project).map(|entry| entry.value().clone())
    }

    /// Drop a project's tracker, writing whatever it still owes first.
    ///
    /// The write is unconditional rather than `flush_if_due`, for `lifecycle`'s reason one scope
    /// down: a task created 200 ms before the project was closed is inside the debounce, and
    /// waiting it out on a store that is about to be dropped loses it silently.
    ///
    /// A merge discovered on the way out is logged and **not** broadcast. `project_close` has
    /// already removed the project from the workspace and told every window, so there is no
    /// panel left to receive a board for it; the bytes are on disk, which is the part that had
    /// to survive.
    pub fn close(&self, project: ProjectId) {
        let Some((_, store)) = self.stores.remove(&project) else {
            return;
        };
        if store.write_now().is_some() {
            tracing::info!(%project, "folded an out-of-process change into the task tracker while closing it");
        }
    }

    /// Write every tracker, unconditionally. The quit path.
    ///
    /// Unconditional for the argument `run_teardown` already makes about the workspace and the
    /// view positions: waiting out a debounce on the way to exit is how the user's last change
    /// gets lost. It is a stronger argument here than for either of those, because what is in
    /// the window is not this user's own scroll position — it is a comment a subagent wrote as
    /// its turn ended, in a file the team reads.
    ///
    /// Nothing is broadcast: the windows are going away, and an emit into a closing app reaches
    /// nobody by construction.
    pub fn flush_all(&self) {
        // Collected before any write. `write_now` takes the store's mutex and does disk I/O,
        // and holding a `DashMap` shard guard across that would block an `ensure` landing in the
        // same shard for the length of a file write.
        let stores: Vec<(ProjectId, Arc<TaskStore>)> = self
            .stores
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();
        for (project, store) in stores {
            if store.write_now().is_some() {
                tracing::info!(%project, "folded an out-of-process change into the task tracker while quitting");
            }
        }
    }

    /// Start the background flusher. Call once, from `setup`.
    ///
    /// A thread rather than a Tauri async task, exactly as `PositionsState::start_flusher`: the
    /// work is a blocking file write, and this crate's one runtime belongs to the IDE server. It
    /// is a daemon — the process exits without joining it — which is safe because
    /// [`crate::lifecycle`]'s teardown performs the final write itself through [`Self::flush_all`]
    /// rather than relying on this loop to notice.
    pub fn start_flusher(self: &Arc<Self>, app: AppHandle) {
        let stores = Arc::clone(self);
        thread::Builder::new()
            .name("cide-tasks".into())
            .spawn(move || {
                // Once, before the loop: a staged attachment (a screenshot pasted into a
                // composer that was then closed) is meant to live for seconds, and one a day
                // old belongs to nobody. Here rather than in `setup` because it is a directory
                // walk, and this is the thread that already exists for the tracker's disk work.
                // (M39)
                let swept = cide_tasks::attachments::sweep_staging(std::time::Duration::from_secs(
                    24 * 60 * 60,
                ));
                if swept > 0 {
                    tracing::info!(swept, "removed stale staged attachments");
                }
                loop {
                    thread::sleep(POLL);
                    stores.tick(&app);
                }
            })
            // A failure to spawn costs the debounce and the out-of-process broadcast, not the
            // feature: every mutation still answers with the whole board, and the shutdown flush
            // still writes. Said out loud because the symptom otherwise is "a teammate's tasks
            // only appear when I touch something", which reads as a panel bug.
            .map(|_| ())
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "no task flusher; trackers will be written on quit only");
            });
    }

    /// One pass over every store: write what is due, broadcast what the disk changed underneath.
    fn tick(&self, app: &AppHandle) {
        let stores: Vec<(ProjectId, Arc<TaskStore>)> = self
            .stores
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();
        for (project, store) in stores {
            // `Some` exactly when the file on disk had moved and the store merged it in — see
            // the module header. A plain debounced write of our own changes answers `None`, and
            // broadcasting those would be an event per burst to every window announcing a board
            // the caller already has.
            if store.flush_if_due().is_some() {
                tracing::info!(%project, "the task tracker changed outside cide; broadcasting the merged board");
                broadcast(app, project, &store);
            }
        }
    }
}

/// The `(rev, board)` pair every answer and every broadcast is built from.
///
/// Two reads rather than one because [`TaskBoard`] carries `rev` only in its `Ready` variant,
/// while `cide://tasks-changed` must carry one in every case: the receiver's rule is "drop
/// anything not newer than what I hold", and a board that arrives with no revision cannot be
/// ordered against the one before it. An `Absent` or `Unreadable` board still has a revision —
/// the in-memory file's — and it still has to be comparable.
pub fn versioned_board(store: &TaskStore) -> (u64, TaskBoard) {
    (store.snapshot().rev, store.board())
}

/// Tell every window that one project's tracker moved.
pub fn broadcast(app: &AppHandle, project: ProjectId, store: &TaskStore) {
    let (rev, board) = versioned_board(store);
    crate::emit::tasks_changed(app, project, rev, &board);
    // Every board change passes here or through `cmd::tasks::answer`, so this is where an idle
    // run whose task just closed is ended. See `AgentRegistry::retire_done`.
    if let Some(registry) =
        tauri::Manager::try_state::<std::sync::Arc<crate::agents::AgentRegistry>>(app)
    {
        registry.retire_done(app, project, &board);
    }
}

/// The directory a project's tracker lives under, or [`CoreError::NoSuchProject`].
///
/// One copy, because the commands and the restore loop must agree about which root holds the
/// file: two answers would be two trackers for one project, and the second one to be written
/// would be invisible to every agent reading the first.
pub fn project_root(state: &WorkspaceState, project: ProjectId) -> Result<PathBuf, CoreError> {
    state.with(|ws| {
        cide_core::workspace::project(ws, project)?
            .roots
            .first()
            .map(|root| root.path.clone())
            // A project with no roots fails `workspace::validate`, so this is belt-and-braces —
            // but the alternative to answering here is `roots[0]` on an empty vec, and a panic
            // in a command worker takes the process down under `panic = "abort"`.
            .ok_or(CoreError::NoRoots)
    })
}
