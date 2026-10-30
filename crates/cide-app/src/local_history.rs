//! Local History in the running app: which projects capture, and the hooks that feed them.
//!
//! The store is `cide-local-history`, and the thread keeping it up to date is [`worker`]. This
//! file is what joins them to projects, settings and Tauri.
//!
//! # Keyed by store, not by project
//!
//! Two `ProjectId`s can be one store. `open_project` compares roots exactly and `scratch::key`
//! canonicalizes, so a project opened through a symlink and through its target are two projects
//! with one key. Two workers on one store is what the store's lock exists to refuse, so the
//! registry never starts a second one: the second project joins the first's capture, and the
//! capture outlives whichever of them closes first.

mod worker;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::thread::JoinHandle;

use cide_ipc::{FsChange, ProjectId};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager as _};

use crate::files::ProjectFs;
use crate::workspace_state::WorkspaceState;
pub use worker::{
    Before, Capture, Limits, RestoreRequest, Restored, Scope, Timing, TreeReport, TreeRequest,
};

struct Attached {
    capture: Arc<Capture>,
    projects: HashSet<ProjectId>,
    /// The index generation last seen, so a settings change does not count as a new one.
    generation: u64,
}

#[derive(Default)]
struct Inner {
    by_key: HashMap<String, Attached>,
    key_of: HashMap<ProjectId, String>,
    /// Workers told to stop and not yet joined, for the next worker on that store to wait for.
    stopping: HashMap<String, JoinHandle<()>>,
}

/// Every running capture. Managed by the app.
#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
    /// Bumped by every content search, so a query the user has typed past stops reading blobs.
    pub search_epoch: AtomicU64,
}

impl Registry {
    /// Make sure this project is captured, starting or rescoping as needed.
    #[allow(clippy::too_many_arguments)]
    fn ensure(
        &self,
        project: ProjectId,
        key: String,
        dir: PathBuf,
        primary_root: PathBuf,
        scope: Scope,
        limits: Limits,
        generation: u64,
        gap: bool,
        paused: bool,
        notify: Box<dyn Fn() + Send + Sync>,
    ) {
        let mut inner = self.inner.lock();
        // Moved to another key: `remove_root` can rekey a project onto another store.
        if let Some(previous) = inner.key_of.get(&project).cloned()
            && previous != key
        {
            inner.release(project, &previous);
        }
        inner.key_of.insert(project, key.clone());
        if let Some(attached) = inner.by_key.get_mut(&key) {
            attached.projects.insert(project);
            attached.capture.set_limits(limits);
            if attached.capture.is_paused() != paused {
                attached.capture.set_paused(paused);
            }
            if attached.generation != generation || gap {
                attached.generation = generation;
                attached.capture.rescope(scope, gap);
            }
            return;
        }
        let after = inner.stopping.remove(&key);
        let capture = Capture::start(
            dir,
            primary_root,
            scope,
            limits,
            Timing::default(),
            paused,
            notify,
            after,
        );
        inner.by_key.insert(
            key,
            Attached {
                capture,
                projects: HashSet::from([project]),
                generation,
            },
        );
    }

    /// Stop capturing a project. Answers the worker to join, if this was its last project.
    ///
    /// The handle is also left behind for a restart to wait on, which is why this does not join:
    /// the caller decides whether it can afford to.
    pub fn detach(&self, project: ProjectId) -> Option<Arc<Capture>> {
        let mut inner = self.inner.lock();
        let key = inner.key_of.remove(&project)?;
        inner.release(project, &key)
    }

    /// Set the stop flag on everything. For teardown, which must not wait: a store is crash-only
    /// and the next start's reconcile recovers whatever was still queued.
    pub fn stop_all(&self) {
        for attached in self.inner.lock().by_key.values() {
            attached.capture.stop();
        }
    }

    /// Every capture whose roots hold this path. Usually one, but nested projects are two stores
    /// that each own the file, and each must hear about it.
    pub fn captures_for(&self, path: &Path) -> Vec<Arc<Capture>> {
        self.inner
            .lock()
            .by_key
            .values()
            .filter(|attached| attached.capture.covers(path))
            .map(|attached| Arc::clone(&attached.capture))
            .collect()
    }

    pub fn capture(&self, project: ProjectId) -> Option<Arc<Capture>> {
        let inner = self.inner.lock();
        let key = inner.key_of.get(&project)?;
        inner.by_key.get(key).map(|a| Arc::clone(&a.capture))
    }
}

impl Inner {
    fn release(&mut self, project: ProjectId, key: &str) -> Option<Arc<Capture>> {
        let attached = self.by_key.get_mut(key)?;
        attached.projects.remove(&project);
        if !attached.projects.is_empty() {
            return None;
        }
        let attached = self.by_key.remove(key)?;
        if let Some(thread) = attached.capture.stop_and_take() {
            self.stopping.insert(key.to_owned(), thread);
        }
        Some(attached.capture)
    }
}

/// Start or refresh a project's capture, or stop it if the switch says so.
///
/// Called from the watcher's thread when its backend comes up, and from a settings change. `gap`
/// is true for the first: events may have been missed, and only a pass can say which.
pub fn attach(app: &AppHandle, project: ProjectId, fs: &Arc<ProjectFs>, gap: bool) {
    let (Some(registry), Some(state)) = (
        app.try_state::<Arc<Registry>>(),
        app.try_state::<WorkspaceState>(),
    ) else {
        return;
    };
    let resolved = state.with(|ws| {
        let enabled = cide_core::local_history_settings::effective(ws, project);
        let rules = cide_core::indexing_settings::effective(ws, project);
        (enabled, rules, Limits::of(&ws.settings.local_history))
    });
    let (enabled, rules, limits) = resolved;
    let roots = fs.root_paths();
    let Some(primary_root) = roots.first().cloned() else {
        return;
    };
    let dir = cide_local_history::dir_for(&primary_root);
    // Off with versions on disk keeps a paused worker, so they stay browsable and restorable.
    // Off with none starts nothing: a project that never captured gets no directory.
    if !enabled && !cide_local_history::exists(&dir) {
        registry.detach(project);
        return;
    }
    let filter = fs.filter();
    let scope = match Scope::new(roots, filter, &rules, Arc::clone(&fs.budget)) {
        Ok(scope) => scope,
        Err(error) => {
            tracing::warn!(%error, "could not scope Local History");
            return;
        }
    };
    let key = cide_core::scratch::key(&primary_root);
    let events = app.clone();
    registry.ensure(
        project,
        key,
        dir,
        primary_root,
        scope,
        limits,
        fs.generation(),
        gap,
        !enabled,
        Box::new(move || crate::emit::local_history_changed(&events, project)),
    );
}

/// A switch or a limit moved: bring every open project into line, with no restart.
pub fn settings_changed(app: &AppHandle) {
    let files = app.state::<crate::files::FsRegistry>();
    for (project, _) in files.indexed() {
        if let Some(fs) = files.get(project) {
            attach(app, project, &fs, false);
        }
    }
}

/// Everything the watcher reports. Must only enqueue: see [`Capture::changed`].
pub fn changed(app: &AppHandle, project: ProjectId, change: &FsChange) {
    if let Some(registry) = app.try_state::<Arc<Registry>>()
        && let Some(capture) = registry.capture(project)
    {
        capture.changed(change);
    }
}

/// The two halves of a save, split so the old bytes are read before the write replaces them.
pub struct PendingSave {
    path: PathBuf,
    captures: Vec<(Arc<Capture>, Option<Before>)>,
}

impl PendingSave {
    /// Call on the blocking thread, before writing.
    pub fn begin(registry: &Registry, path: &Path) -> Self {
        let captures = registry
            .captures_for(path)
            .into_iter()
            .map(|capture| {
                let before = capture.before_save(path);
                (capture, before)
            })
            .collect();
        Self {
            path: path.to_path_buf(),
            captures,
        }
    }

    /// Call once the write succeeded. A refused write records nothing.
    pub fn finish(self, bytes: Vec<u8>) {
        let Self { path, captures } = self;
        let last = captures.len().saturating_sub(1);
        let mut bytes = Some(bytes);
        for (index, (capture, before)) in captures.into_iter().enumerate() {
            let content = if index == last {
                bytes.take().unwrap_or_default()
            } else {
                bytes.clone().unwrap_or_default()
            };
            capture.saved(path.clone(), before, content);
        }
    }
}

/// cide renamed or moved these.
pub fn moved(registry: &Registry, moves: &[(PathBuf, PathBuf)]) {
    let Some((from, _)) = moves.first() else {
        return;
    };
    for capture in registry.captures_for(from) {
        capture.moved(moves.to_vec());
    }
}

#[cfg(test)]
mod tests;
