//! Process-wide root authorities, shared by projects and all their windows.
use cide_core::local_history::{HistoryCoordinator, HistoryStore, RootIdentity};
use cide_ipc::{ProjectId, Workspace, local_history::HistoryRootId};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

struct Entry {
    identity: RootIdentity,
    owners: HashSet<ProjectId>,
    coordinator: Arc<HistoryCoordinator>,
}
static ROOTS: LazyLock<Mutex<BTreeMap<String, Entry>>> = LazyLock::new(Mutex::default);

/// Called only when project/root membership changes, never for editor dirty-state updates.
pub fn sync(_app: &tauri::AppHandle, workspace: &Workspace) {
    let mut wanted: BTreeMap<String, (RootIdentity, HashSet<ProjectId>)> = BTreeMap::new();
    for project in workspace.projects.values() {
        for root in &project.roots {
            match RootIdentity::new(&root.path) {
                Ok(identity) => {
                    wanted
                        .entry(identity.id.0.clone())
                        .or_insert_with(|| (identity, HashSet::new()))
                        .1
                        .insert(project.id);
                }
                Err(reason) => {
                    tracing::warn!(%reason, root = %root.path.display(), "history root is unavailable")
                }
            }
        }
    }
    let identities: Vec<_> = wanted.values().map(|(r, _)| r.clone()).collect();
    let mut roots = ROOTS.lock();
    let removed: Vec<_> = roots
        .keys()
        .filter(|key| !wanted.contains_key(*key))
        .cloned()
        .collect();
    for key in removed {
        if let Some(entry) = roots.remove(&key) {
            std::thread::spawn(move || entry.coordinator.close());
        }
    }
    for (key, (identity, owners)) in wanted {
        let nested: Vec<PathBuf> = identities
            .iter()
            .filter(|r| r.path != identity.path && r.path.starts_with(&identity.path))
            .map(|r| r.path.clone())
            .collect();
        if let Some(entry) = roots.get_mut(&key) {
            entry.owners = owners;
            entry.coordinator.set_nested(nested);
        } else {
            let coordinator = HistoryCoordinator::start(
                identity.clone(),
                HistoryStore::profile_path(&identity),
                nested,
                |_, _| {},
            );
            roots.insert(
                key,
                Entry {
                    identity,
                    owners,
                    coordinator,
                },
            );
        }
    }
}

pub fn scoped(project: ProjectId, root: &HistoryRootId) -> Result<Arc<HistoryCoordinator>, String> {
    ROOTS
        .lock()
        .get(&root.0)
        .filter(|e| e.owners.contains(&project))
        .map(|e| e.coordinator.clone())
        .ok_or_else(|| "Select an open project history root".into())
}

pub fn for_path(path: &Path) -> Option<Arc<HistoryCoordinator>> {
    let roots = ROOTS.lock();
    roots
        .values()
        .filter_map(|e| {
            e.identity
                .resolve(path)
                .ok()
                .map(|(physical, _)| (e, physical))
        })
        .filter(|(e, p)| p.starts_with(&e.identity.path))
        .max_by_key(|(e, _)| e.identity.path.components().count())
        .map(|(e, _)| e.coordinator.clone())
}

pub fn project_roots(project: ProjectId) -> Vec<Arc<HistoryCoordinator>> {
    ROOTS
        .lock()
        .values()
        .filter(|e| e.owners.contains(&project))
        .map(|e| e.coordinator.clone())
        .collect()
}

pub fn close_all() {
    let entries = std::mem::take(&mut *ROOTS.lock());
    for entry in entries.into_values() {
        entry.coordinator.close();
    }
}
