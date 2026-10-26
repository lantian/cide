//! Process-wide root authorities, shared by projects and all their windows.
use cide_core::local_history::{
    HistoryCoordinator, HistoryEligibility, HistoryStore, RootIdentity,
};
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
    fallback: Option<cide_fs::Watcher>,
    subscribed: HashSet<ProjectId>,
}
static ROOTS: LazyLock<Mutex<BTreeMap<String, Entry>>> = LazyLock::new(Mutex::default);

/// Called only when project/root membership changes, never for editor dirty-state updates.
pub fn sync(app: &tauri::AppHandle, workspace: &Workspace) {
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
            std::thread::spawn(move || {
                drop(entry.fallback);
                entry.coordinator.close();
            });
        }
    }
    for (key, (identity, owners)) in wanted {
        let nested: Vec<PathBuf> = identities
            .iter()
            .filter(|r| r.path != identity.path && r.path.starts_with(&identity.path))
            .map(|r| r.path.clone())
            .collect();
        if let Some(entry) = roots.get_mut(&key) {
            entry.subscribed.retain(|p| owners.contains(p));
            entry.owners = owners;
            if entry.subscribed.is_empty() && entry.fallback.is_none() {
                entry.fallback = Some(fallback(&entry.identity, &entry.coordinator));
            }
            entry.coordinator.set_nested(nested);
        } else {
            let events = app.clone();
            let coordinator = HistoryCoordinator::start_observed(
                identity.clone(),
                HistoryStore::profile_path(&identity),
                nested,
                move |status, paths| {
                    crate::emit::local_history_changed(
                        &events,
                        &cide_ipc::local_history::HistoryInvalidation {
                            root: status.root,
                            paths,
                            generation: status.generation,
                            revision: status.revision,
                        },
                    )
                },
            );
            let fallback = Some(fallback(&identity, &coordinator));
            roots.insert(
                key,
                Entry {
                    identity,
                    owners,
                    coordinator,
                    fallback,
                    subscribed: HashSet::new(),
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

pub fn mutate<T, E: HistoryMutationError>(
    path: &Path,
    source: cide_ipc::local_history::HistorySource,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let _lease = write_lease(std::slice::from_ref(&path.to_path_buf())).map_err(E::history_busy)?;
    match for_path(path) {
        Some(coordinator) => coordinator.mutate(path, source, operation),
        None => operation(),
    }
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
        drop(entry.fallback);
        entry.coordinator.close();
    }
}

struct WatchPolicy {
    coordinator: Arc<HistoryCoordinator>,
    policy: Mutex<HistoryEligibility>,
}
impl WatchPolicy {
    fn accepts(&self, path: &Path, directory: bool) -> bool {
        let current = self.coordinator.status().policy;
        if !current.enabled {
            return false;
        }
        let mut policy = self.policy.lock();
        if policy.policy != current {
            if let Ok(next) = HistoryEligibility::new(policy.root.clone(), current) {
                *policy = next;
            }
        }
        if path == policy.root.path && directory {
            return true;
        }
        policy
            .path(path, directory)
            .ok()
            .is_some_and(|(physical, _)| !self.coordinator.is_nested(&physical))
    }
}
fn subscription(
    entries: Vec<(RootIdentity, Arc<HistoryCoordinator>)>,
    project: Option<ProjectId>,
) -> Option<Arc<cide_fs::watch::HistorySubscription>> {
    if entries.is_empty() {
        return None;
    }
    let policies = Arc::new(
        entries
            .into_iter()
            .filter_map(|(root, coordinator)| {
                Some(WatchPolicy {
                    policy: Mutex::new(
                        HistoryEligibility::new(root, coordinator.status().policy).ok()?,
                    ),
                    coordinator,
                })
            })
            .collect::<Vec<_>>(),
    );
    for policy in policies.iter() {
        policy.coordinator.attach_observer();
    }
    let ready = policies.clone();
    let notified = std::sync::atomic::AtomicBool::new(false);
    let dirs = policies.clone();
    let admits = policies.clone();
    let changed = policies.clone();
    Some(Arc::new(cide_fs::watch::HistorySubscription {
        ready: Arc::new(move |paths| {
            for policy in ready.iter() {
                policy.coordinator.confirm_watches(&paths);
            }
            if let Some(project) = project {
                if ready.iter().all(|p| {
                    p.coordinator
                        .watch_directories()
                        .iter()
                        .all(|d| !p.accepts(d, true) || paths.contains(d))
                }) && !notified.swap(true, std::sync::atomic::Ordering::AcqRel)
                {
                    tree_subscribed(project);
                }
            }
        }),
        directories: Arc::new(move || {
            dirs.iter()
                .flat_map(|p| p.coordinator.watch_directories())
                .collect()
        }),
        admits: Arc::new(move |path, directory| admits.iter().any(|p| p.accepts(path, directory))),
        changed: Arc::new(move |change| {
            for policy in changed.iter() {
                let paths = change
                    .paths
                    .iter()
                    .filter(|p| policy.accepts(p, p.is_dir()))
                    .cloned()
                    .collect();
                policy.coordinator.observe(paths, change.truncated);
            }
        }),
        degraded: Arc::new(move |reason| {
            for policy in policies.iter() {
                policy.coordinator.gap(&reason);
            }
        }),
    }))
}
fn fallback(identity: &RootIdentity, coordinator: &Arc<HistoryCoordinator>) -> cide_fs::Watcher {
    let config = cide_fs::WatchConfig {
        roots: vec![identity.path.clone()],
        dirs: vec![identity.path.clone()],
        history: subscription(vec![(identity.clone(), coordinator.clone())], None),
        ..Default::default()
    };
    let filter = Arc::new(cide_fs::Filter::build(
        &config.roots,
        vec![identity.path.as_path()],
        cide_fs::Visibility::CONSERVATIVE,
    ));
    cide_fs::Watcher::start(config, filter, |_| {})
}
pub fn history_subscription(
    project: ProjectId,
) -> Option<Arc<cide_fs::watch::HistorySubscription>> {
    let entries = ROOTS
        .lock()
        .values()
        .filter(|e| e.owners.contains(&project))
        .map(|e| (e.identity.clone(), e.coordinator.clone()))
        .collect();
    subscription(entries, Some(project))
}
pub fn tree_subscribed(project: ProjectId) {
    let mut roots = ROOTS.lock();
    for entry in roots.values_mut().filter(|e| e.owners.contains(&project)) {
        entry.subscribed.insert(project);
        if let Some(observer) = entry.fallback.take() {
            std::thread::spawn(move || drop(observer));
        }
    }
}
pub fn tree_unsubscribed(project: ProjectId) {
    let mut roots = ROOTS.lock();
    for entry in roots.values_mut() {
        entry.subscribed.remove(&project);
        if entry.subscribed.is_empty() && entry.fallback.is_none() {
            entry.fallback = Some(fallback(&entry.identity, &entry.coordinator));
        }
    }
}

/// Hold every affected root in stable key order, including roots nested in a moved directory.
pub fn mutate_tree<T, E: HistoryMutationError>(
    paths: &[PathBuf],
    operation: impl FnOnce() -> Result<T, E>,
    summarize: &impl Fn(&Result<T, E>) -> Vec<cide_core::local_history::HistoryTreeChange>,
) -> Result<T, E> {
    let mut regions = paths.to_vec();
    regions.extend(
        paths
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf)),
    );
    let _lease = write_lease(&regions).map_err(E::history_busy)?;
    let coordinators: Vec<_> = ROOTS
        .lock()
        .values()
        .filter(|e| {
            paths.iter().any(|p| {
                p.starts_with(&e.identity.path)
                    || e.identity.path.starts_with(p)
                    || e.identity.resolve(p).is_ok()
            })
        })
        .map(|e| e.coordinator.clone())
        .collect();
    fn run<T, E>(
        roots: &[Arc<HistoryCoordinator>],
        paths: &[PathBuf],
        operation: impl FnOnce() -> Result<T, E>,
        summarize: &impl Fn(&Result<T, E>) -> Vec<cide_core::local_history::HistoryTreeChange>,
    ) -> Result<T, E> {
        match roots.split_first() {
            Some((first, rest)) => {
                first.mutate_tree(paths, || run(rest, paths, operation, summarize), summarize)
            }
            None => operation(),
        }
    }
    run(&coordinators, paths, operation, summarize)
}

pub static EDITOR_GATES: LazyLock<cide_core::local_history::RestoreGates> =
    LazyLock::new(Default::default);
pub trait HistoryMutationError {
    fn history_busy(reason: String) -> Self;
}
impl HistoryMutationError for cide_core::CoreError {
    fn history_busy(reason: String) -> Self {
        Self::Io(reason)
    }
}
impl HistoryMutationError for cide_fs::FsError {
    fn history_busy(reason: String) -> Self {
        Self::InvalidPath(reason)
    }
}
impl HistoryMutationError for std::io::Error {
    fn history_busy(reason: String) -> Self {
        Self::other(reason)
    }
}
impl HistoryMutationError for String {
    fn history_busy(reason: String) -> Self {
        reason
    }
}
pub fn canonical_path(path: &Path) -> Option<PathBuf> {
    ROOTS
        .lock()
        .values()
        .filter_map(|e| {
            e.identity
                .resolve(path)
                .ok()
                .map(|(physical, _)| (e.identity.path.components().count(), physical))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, physical)| physical)
}
fn write_lease(paths: &[PathBuf]) -> Result<cide_core::local_history::WriteLease, String> {
    EDITOR_GATES.write(
        paths
            .iter()
            .map(|p| {
                canonical_path(p)
                    .or_else(|| std::fs::canonicalize(p).ok())
                    .unwrap_or_else(|| p.clone())
            })
            .collect(),
    )
}

pub fn view_for(
    project: ProjectId,
    path: Option<&Path>,
    root: Option<&HistoryRootId>,
) -> Result<cide_ipc::local_history::LocalHistoryView, String> {
    let roots = ROOTS.lock();
    let entry = roots
        .values()
        .filter(|e| e.owners.contains(&project) && root.is_none_or(|r| r == &e.identity.id))
        .filter(|e| path.is_none_or(|p| e.identity.resolve(p).is_ok()))
        .max_by_key(|e| {
            if path.is_some() {
                e.identity.path.components().count()
            } else {
                0
            }
        })
        .ok_or("This path is outside the project's open history roots")?;
    Ok(cide_ipc::local_history::LocalHistoryView {
        root: entry.identity.id.clone(),
        path: path
            .map(|p| entry.identity.resolve(p).map(|(_, p)| p))
            .transpose()?,
    })
}
