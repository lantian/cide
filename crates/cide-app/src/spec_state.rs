//! Project-scoped OpenSpec reads, targeted invalidation and ordered broadcasts.
use crate::spec_cache::{ReadCache, ReadPool};
use cide_ipc::{
    ProjectId, SpecArtifactText, SpecBoard, SpecChange, SpecCheckout, SpecInvalidation,
    SpecSnapshot,
};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const COALESCE: Duration = Duration::from_millis(250);
const COALESCE_CEILING: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct SpecBoards {
    projects: Mutex<HashMap<ProjectId, Arc<ProjectSpecs>>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Changes,
    Specs,
    Detail(PathBuf, String),
    Artifact(PathBuf, PathBuf),
    Progress(PathBuf, String),
    GitCheckouts,
}
#[derive(Clone)]
enum Value {
    Changes(Vec<cide_ipc::ChangeSummary>),
    Specs(Vec<cide_ipc::SpecSummary>),
    Detail(SpecChange),
    Artifact(SpecArtifactText),
    Progress(u32, u32),
    Checkouts(Vec<SpecCheckout>),
}
#[derive(Default)]
struct Burst {
    pending: SpecInvalidation,
    first: Option<Instant>,
    last: Option<Instant>,
    running: bool,
}

impl Burst {
    /// Returns true only for the event which owns starting a worker.
    fn enqueue(&mut self, invalidated: SpecInvalidation, now: Instant) -> bool {
        self.pending.full |= invalidated.full;
        let paths: HashSet<_> = self
            .pending
            .paths
            .iter()
            .chain(&invalidated.paths)
            .cloned()
            .collect();
        self.pending.paths = paths.into_iter().collect();
        self.first.get_or_insert(now);
        self.last = Some(now);
        let start = !self.running;
        self.running = true;
        start
    }
    fn take_ready(&mut self, now: Instant) -> Option<SpecInvalidation> {
        let first = self.first?;
        if self
            .last
            .is_some_and(|last| now.duration_since(last) < COALESCE)
            && now.duration_since(first) < COALESCE_CEILING
        {
            return None;
        }
        self.first = None;
        self.last = None;
        Some(std::mem::take(&mut self.pending))
    }
}

pub struct ProjectSpecs {
    revision: AtomicU64,
    alive: AtomicBool,
    cache: ReadCache<Key, Value>,
    // Board halves and one pair of counts per live checkout must not evict each other as a
    // large checkout list is traversed. Progress entries are pruned against the live roster.
    summaries: ReadCache<Key, Value>,
    pool: ReadPool,
    board_read: Mutex<()>,
    burst: Mutex<Burst>,
    archives: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}
impl Default for ProjectSpecs {
    fn default() -> Self {
        Self {
            revision: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            cache: ReadCache::default(),
            summaries: ReadCache::with_capacity(usize::MAX),
            pool: ReadPool::default(),
            board_read: Mutex::new(()),
            burst: Mutex::new(Burst::default()),
            archives: Mutex::new(HashMap::new()),
        }
    }
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
fn invalidates(key: &Key, root: &Path, invalidation: &SpecInvalidation) -> bool {
    if invalidation.full {
        return true;
    }
    invalidation.paths.iter().any(|path| {
        let path = root.join(path);
        match key {
            Key::Changes => overlaps(&path, &root.join("openspec/changes")),
            Key::Specs => overlaps(&path, &root.join("openspec/specs")),
            Key::GitCheckouts => {
                overlaps(&path, &root.join(".cide/worktrees"))
                    && !path.components().any(|part| part.as_os_str() == "openspec")
            }
            Key::Detail(cwd, name) | Key::Progress(cwd, name) => {
                overlaps(&path, &cwd.join("openspec/changes").join(name))
                    || overlaps(&path, &cwd.join("openspec/changes/archive"))
                    || overlaps(&path, &cwd.join("openspec/specs"))
                    || overlaps(&path, &cwd.join("openspec/config.yaml"))
                    || overlaps(&path, &cwd.join("openspec/schemas"))
            }
            Key::Artifact(_, artifact) => overlaps(&path, artifact),
        }
    })
}

impl ProjectSpecs {
    fn ensure_open(&self) -> Result<(), String> {
        if self.alive.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("This project was closed.".into())
        }
    }
    pub fn invalidate(&self, root: &Path, invalidation: &SpecInvalidation) {
        // Clear entries before publishing the new revision, so a snapshot tagged with that
        // revision cannot observe a still-cached value from before the invalidation.
        self.cache
            .invalidate(|key| invalidates(key, root, invalidation));
        self.summaries
            .invalidate(|key| invalidates(key, root, invalidation));
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
    pub fn invalidate_git(&self) {
        self.summaries
            .invalidate(|key| matches!(key, Key::GitCheckouts));
    }
    pub fn foreground<T>(&self, work: impl FnOnce() -> T) -> T {
        self.pool.run(true, work)
    }
    pub fn archive<T>(
        &self,
        cwd: &Path,
        work: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let lock = self.archives.lock().entry(cwd).or_default().clone();
        let Some(_guard) = lock.try_lock() else {
            return Err(
                "An archive is already running in this checkout. Wait for it to finish.".into(),
            );
        };
        work()
    }
    pub fn snapshot(&self, root: &Path, force: bool) -> SpecSnapshot {
        if force {
            self.invalidate(
                root,
                &SpecInvalidation {
                    full: true,
                    paths: vec![],
                },
            );
        }
        let _reader = self.board_read.lock();
        for _ in 0..3 {
            let revision = self.revision.load(Ordering::SeqCst);
            let board = self.read_board(root);
            if revision == self.revision.load(Ordering::SeqCst)
                || !self.alive.load(Ordering::SeqCst)
            {
                return SpecSnapshot { revision, board };
            }
        }
        SpecSnapshot { revision: self.revision.load(Ordering::SeqCst), board: SpecBoard::Unusable {
            reason: "OpenSpec files kept changing during the read. Retry once the current edits finish.".into(),
        } }
    }
    fn read_board(&self, root: &Path) -> SpecBoard {
        if !self.alive.load(Ordering::SeqCst) {
            return SpecBoard::Unusable {
                reason: "This project was closed.".into(),
            };
        }
        if !cide_spec::present(root) {
            return SpecBoard::Absent {
                hint: ABSENT_HINT.into(),
                path: cide_spec::spec_path(root),
            };
        }
        let read = || -> Result<SpecBoard, String> {
            let os = cide_spec::Openspec::open(root).map_err(|e| e.to_string())?;
            let (changes, specs) = std::thread::scope(|scope| {
                let changes = scope.spawn(|| {
                    self.summaries.get(Key::Changes, || {
                        self.pool.run(false, || {
                            self.ensure_open()?;
                            os.changes().map(Value::Changes).map_err(|e| e.to_string())
                        })
                    })
                });
                let specs = self.summaries.get(Key::Specs, || {
                    self.pool.run(false, || {
                        self.ensure_open()?;
                        os.specs().map(Value::Specs).map_err(|e| e.to_string())
                    })
                });
                (changes.join().expect("change list reader"), specs)
            });
            let (changes, specs) = (changes?, specs?);
            let (Value::Changes(changes), Value::Specs(specs)) = (changes, specs) else {
                unreachable!()
            };
            Ok(SpecBoard::Ready {
                changes,
                specs,
                root: root.to_path_buf(),
                commands: cide_spec::claude::installed(root),
            })
        };
        read().unwrap_or_else(|reason| SpecBoard::Unusable { reason })
    }
    pub fn change(&self, cwd: &Path, name: &cide_ipc::ChangeName) -> Result<SpecChange, String> {
        let result = self
            .cache
            .get(Key::Detail(cwd.to_path_buf(), name.0.clone()), || {
                self.pool.run(true, || {
                    self.ensure_open()?;
                    cide_spec::Openspec::open(cwd)
                        .and_then(|os| os.change(name))
                        .map(Value::Detail)
                        .map_err(|e| e.to_string())
                })
            })?;
        let Value::Detail(value) = result else {
            unreachable!()
        };
        Ok(value)
    }
    pub fn artifact(&self, cwd: &Path, path: &Path) -> Result<SpecArtifactText, String> {
        let result =
            self.cache
                .get(Key::Artifact(cwd.to_path_buf(), path.to_path_buf()), || {
                    self.pool.run(true, || {
                        self.ensure_open()?;
                        let text = cide_spec::Openspec::open(cwd)
                            .and_then(|os| os.artifact(path))
                            .map_err(|e| e.to_string())?;
                        Ok(Value::Artifact(SpecArtifactText {
                            text: text.text,
                            truncated: text.truncated,
                        }))
                    })
                })?;
        let Value::Artifact(value) = result else {
            unreachable!()
        };
        Ok(value)
    }
    pub fn progress(&self, cwd: &Path, name: &cide_ipc::ChangeName) -> Result<(u32, u32), String> {
        let result =
            self.summaries
                .get(Key::Progress(cwd.to_path_buf(), name.0.clone()), || {
                    self.pool.run(false, || {
                        self.ensure_open()?;
                        cide_spec::Openspec::open(cwd)
                            .and_then(|os| os.progress(name))
                            .map(|progress| Value::Progress(progress.completed, progress.total))
                            .map_err(|e| e.to_string())
                    })
                })?;
        let Value::Progress(done, total) = result else {
            unreachable!()
        };
        Ok((done, total))
    }
    pub fn checkout_metadata(
        &self,
        read: impl Fn() -> Vec<SpecCheckout>,
    ) -> Result<Vec<SpecCheckout>, String> {
        let Value::Checkouts(value) = self.summaries.get(Key::GitCheckouts, || {
            self.ensure_open()?;
            Ok(Value::Checkouts(read()))
        })?
        else {
            unreachable!()
        };
        Ok(value)
    }
    pub fn retain_checkouts(&self, paths: &[PathBuf]) {
        let keep: HashSet<_> = paths.iter().collect();
        self.summaries.retain(|key| match key {
            Key::Progress(cwd, _) => keep.contains(cwd),
            _ => true,
        });
    }
}

impl SpecBoards {
    pub fn project(&self, project: ProjectId) -> Arc<ProjectSpecs> {
        self.projects.lock().entry(project).or_default().clone()
    }
    pub fn remove(&self, project: ProjectId) {
        if let Some(state) = self.projects.lock().remove(&project) {
            state.alive.store(false, Ordering::SeqCst);
        }
    }
    pub fn invalidate_git(&self, project: ProjectId) {
        if let Some(state) = self.projects.lock().get(&project) {
            state.invalidate_git();
        }
    }
    pub fn mark_changed(self: &Arc<Self>, app: &AppHandle, project: ProjectId) {
        self.mark(
            app,
            project,
            SpecInvalidation {
                full: true,
                paths: vec![],
            },
        );
    }
    pub fn mark_paths(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        paths: &[PathBuf],
        root: &Path,
    ) {
        let mut invalidated = SpecInvalidation::default();
        for path in paths {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let path = relative.to_string_lossy().replace('\\', "/");
            let relative_spec = if path == "openspec" || path.starts_with("openspec/") {
                path.as_str()
            } else if let Some((_, inner)) = path
                .strip_prefix(".cide/worktrees/")
                .and_then(|rest| rest.split_once('/'))
            {
                if inner != "openspec" && !inner.starts_with("openspec/") {
                    continue;
                }
                inner
            } else {
                continue;
            };
            let spec = relative_spec.strip_prefix("openspec/");
            if spec.is_none_or(|rest| !(rest.starts_with("changes/") || rest.starts_with("specs/")))
            {
                invalidated.full = true;
            }
            invalidated.paths.push(path);
        }
        if invalidated.full || !invalidated.paths.is_empty() {
            self.mark(app, project, invalidated);
        }
    }
    fn mark(self: &Arc<Self>, app: &AppHandle, project: ProjectId, invalidated: SpecInvalidation) {
        let workspace = app.state::<crate::workspace_state::WorkspaceState>();
        let Ok(root) = crate::tasks_state::project_root(&workspace, project) else {
            return;
        };
        let state = self.project(project);
        state.invalidate(&root, &invalidated);
        if !state.burst.lock().enqueue(invalidated, Instant::now()) {
            return;
        }
        let app = app.clone();
        let worker = Arc::clone(&state);
        let spawned = std::thread::Builder::new()
            .name("cide-spec-emit".into())
            .spawn(move || {
                while worker.alive.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(25));
                    let invalidated = {
                        let mut burst = worker.burst.lock();
                        if burst.first.is_none() {
                            burst.running = false;
                            return;
                        }
                        let Some(invalidated) = burst.take_ready(Instant::now()) else {
                            continue;
                        };
                        invalidated
                    };
                    let snapshot = worker.snapshot(&root, false);
                    if worker.alive.load(Ordering::SeqCst) {
                        crate::emit::spec_changed(
                            &app,
                            &cide_ipc::SpecChanged {
                                project,
                                snapshot,
                                invalidated,
                            },
                        );
                    }
                    // Keep running through the read. A new burst is picked up by this worker.
                }
            });
        if let Err(error) = spawned {
            state.burst.lock().running = false;
            tracing::warn!(%error, "could not start spec reader");
        }
    }
}

/// The sentence the panel prints above its Set-up button.
///
/// Here rather than in the frontend because it is *about the project* — the same reason
/// `TaskBoard::Absent` carries its own — and because the frontend must not be the place that
/// decides what OpenSpec is for.
pub const ABSENT_HINT: &str = "OpenSpec keeps this project's requirements in openspec/specs/, and each piece of work in \
     openspec/changes/ — a proposal, a task list, and the requirement edits it makes. It is an \
     open standard: the same folder is read by Claude Code, Cursor and thirty other tools.";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_invalidation_discovers_a_checkout_after_an_empty_scan() {
        let service = ProjectSpecs::default();
        let root = Path::new("/repo");
        assert!(service.checkout_metadata(Vec::new).unwrap().is_empty());
        let checkout = SpecCheckout {
            change: cide_ipc::ChangeName("one".into()),
            branch: "cide/spec-one".into(),
            completed_tasks: None,
            total_tasks: None,
            archived_as: None,
            unmerged: Some(1),
            dirty: false,
        };
        let read = || vec![checkout.clone()];
        // Artifact events refresh the checklist, but cannot discover new git registrations.
        service.invalidate(
            root,
            &SpecInvalidation {
                full: false,
                paths: vec![".cide/worktrees/spec-one/openspec/changes/one/tasks.md".into()],
            },
        );
        assert!(service.checkout_metadata(read).unwrap().is_empty());
        // Apply creation explicitly marks a full change before publishing its snapshot.
        service.invalidate(
            root,
            &SpecInvalidation {
                full: true,
                paths: vec![],
            },
        );
        let rows = service.checkout_metadata(read).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].branch, "cide/spec-one");
        // Integrate's response can precede retirement. Its removal event clears the cached
        // registration so a later frontend scan cannot keep offering Integrate indefinitely.
        service.invalidate(
            root,
            &SpecInvalidation {
                full: true,
                paths: vec![],
            },
        );
        assert!(service.checkout_metadata(Vec::new).unwrap().is_empty());
    }

    #[test]
    fn a_large_checkout_scan_keeps_counts_hot_and_prunes_closed_checkouts() {
        let service = ProjectSpecs::default();
        let paths: Vec<_> = (0..100)
            .map(|n| PathBuf::from(format!("/repo/.cide/worktrees/spec-{n}")))
            .collect();
        for cwd in &paths {
            service
                .summaries
                .get(Key::Progress(cwd.clone(), "one".into()), || {
                    Ok(Value::Progress(1, 2))
                })
                .unwrap();
        }
        service.retain_checkouts(&paths);
        for cwd in &paths {
            assert_eq!(
                service.progress(cwd, &cide_ipc::ChangeName("one".into())),
                Ok((1, 2)),
                "must not spawn the CLI again during a large scan"
            );
        }
        service.retain_checkouts(&paths[1..]);
        let value = service
            .summaries
            .get(Key::Progress(paths[0].clone(), "one".into()), || {
                Ok(Value::Progress(3, 4))
            })
            .unwrap();
        assert!(matches!(value, Value::Progress(3, 4)));
    }

    #[test]
    fn events_during_a_read_stay_with_one_worker_and_the_ceiling_drains_a_storm() {
        let mut burst = Burst::default();
        let start = Instant::now();
        let event = |name: &str| SpecInvalidation {
            full: false,
            paths: vec![name.into()],
        };
        assert!(burst.enqueue(event("one"), start));
        assert!(
            burst
                .take_ready(start + Duration::from_millis(100))
                .is_none()
        );
        assert_eq!(burst.take_ready(start + COALESCE).unwrap().paths, ["one"]);
        assert!(burst.running, "ownership lasts through the actual read");
        assert!(!burst.enqueue(event("two"), start + Duration::from_millis(300)));
        for ms in [400, 600, 800, 1000, 1200] {
            assert!(!burst.enqueue(event("two"), start + Duration::from_millis(ms)));
            assert!(
                burst
                    .take_ready(start + Duration::from_millis(ms))
                    .is_none()
            );
        }
        assert_eq!(
            burst
                .take_ready(start + Duration::from_millis(1300))
                .unwrap()
                .paths,
            ["two"]
        );
        assert!(burst.running);
    }

    #[test]
    fn archive_lock_serializes_mutations_per_checkout() {
        let service = ProjectSpecs::default();
        let root = Path::new("/repo");
        service
            .archive(root, || {
                assert!(service.archive(root, || Ok(())).is_err());
                assert!(service.archive(Path::new("/other"), || Ok(())).is_ok());
                Ok(())
            })
            .unwrap();
        assert!(service.archive(root, || Ok(())).is_ok());
    }

    #[test]
    fn worktree_edits_do_not_invalidate_other_checkouts_or_root_lists() {
        let root = Path::new("/repo");
        let event = SpecInvalidation {
            full: false,
            paths: vec![".cide/worktrees/spec-one/openspec/changes/one/proposal.md".into()],
        };
        assert!(invalidates(
            &Key::Detail(root.join(".cide/worktrees/spec-one"), "one".into()),
            root,
            &event
        ));
        assert!(!invalidates(
            &Key::Detail(root.join(".cide/worktrees/spec-two"), "two".into()),
            root,
            &event
        ));
        assert!(!invalidates(&Key::Changes, root, &event));
        assert!(!invalidates(&Key::Specs, root, &event));
    }
    #[test]
    fn prose_edits_invalidate_detail_even_without_progress_changes() {
        let root = Path::new("/repo");
        let event = SpecInvalidation {
            full: false,
            paths: vec!["openspec/changes/one/proposal.md".into()],
        };
        assert!(invalidates(
            &Key::Detail(root.to_path_buf(), "one".into()),
            root,
            &event
        ));
        assert!(!invalidates(&Key::Specs, root, &event));
    }
}
