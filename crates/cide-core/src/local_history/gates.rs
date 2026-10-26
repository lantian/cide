//! Live editor acknowledgements and write admission, independent of a window runtime.
use parking_lot::{Condvar, Mutex};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EditorIdentity {
    pub window: String,
    pub editor: String,
    pub mount: String,
}
struct Mounted {
    identity: EditorIdentity,
    path: PathBuf,
}
struct Gate {
    request: String,
    epoch: u64,
    editors: Vec<EditorIdentity>,
    acknowledgements: BTreeMap<EditorIdentity, bool>,
}
#[derive(Default)]
struct State {
    epoch: u64,
    mounted: BTreeMap<(String, String), Mounted>,
    gates: BTreeMap<PathBuf, Gate>,
    writers: BTreeMap<PathBuf, usize>,
}
#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    wake: Condvar,
}
#[derive(Clone, Default)]
pub struct RestoreGates {
    inner: Arc<Inner>,
}
pub struct RestorePermit {
    gates: RestoreGates,
    pub path: PathBuf,
    pub request: String,
    pub editors: Vec<EditorIdentity>,
}
pub struct WriteLease {
    gates: RestoreGates,
    paths: Vec<PathBuf>,
}

impl RestoreGates {
    pub fn register(&self, window: String, editor: String, path: PathBuf) -> EditorIdentity {
        let identity = EditorIdentity {
            window,
            editor,
            mount: uuid::Uuid::new_v4().to_string(),
        };
        let mut state = self.inner.state.lock();
        state.epoch = state.epoch.wrapping_add(1);
        state.mounted.insert(
            (identity.window.clone(), identity.editor.clone()),
            Mounted {
                identity: identity.clone(),
                path,
            },
        );
        self.inner.wake.notify_all();
        identity
    }
    pub fn unregister(&self, identity: &EditorIdentity) {
        let mut state = self.inner.state.lock();
        let key = (identity.window.clone(), identity.editor.clone());
        if state
            .mounted
            .get(&key)
            .is_some_and(|m| m.identity == *identity)
        {
            state.mounted.remove(&key);
            state.epoch = state.epoch.wrapping_add(1);
            self.inner.wake.notify_all();
        }
    }
    pub fn unregister_window(&self, window: &str) {
        let mut state = self.inner.state.lock();
        let before = state.mounted.len();
        state.mounted.retain(|(owner, _), _| owner != window);
        if state.mounted.len() != before {
            state.epoch = state.epoch.wrapping_add(1);
            self.inner.wake.notify_all();
        }
    }
    pub fn write(&self, paths: Vec<PathBuf>) -> Result<WriteLease, String> {
        let mut state = self.inner.state.lock();
        if paths.iter().any(|p| {
            state
                .gates
                .keys()
                .any(|g| p.starts_with(g) || g.starts_with(p))
        }) {
            return Err(
                "Local History is preparing a restore for this path; retry after it finishes"
                    .into(),
            );
        }
        for path in &paths {
            *state.writers.entry(path.clone()).or_default() += 1;
        }
        Ok(WriteLease {
            gates: self.clone(),
            paths,
        })
    }
    pub fn prepare(
        &self,
        path: PathBuf,
        expected: &BTreeSet<String>,
    ) -> Result<RestorePermit, String> {
        let mut state = self.inner.state.lock();
        if state.gates.contains_key(&path)
            || state
                .writers
                .keys()
                .any(|p| p.starts_with(&path) || path.starts_with(p))
        {
            return Err("A save or restore is in progress; retry after it finishes".into());
        }
        let editors: Vec<_> = state
            .mounted
            .values()
            .filter(|m| m.path == path)
            .map(|m| m.identity.clone())
            .collect();
        if !expected
            .iter()
            .all(|id| editors.iter().any(|e| &e.editor == id))
        {
            return Err("An editor is still opening or does not support restore preparation. Close it or wait, then retry.".into());
        }
        let request = uuid::Uuid::new_v4().to_string();
        let epoch = state.epoch;
        state.gates.insert(
            path.clone(),
            Gate {
                request: request.clone(),
                epoch,
                editors: editors.clone(),
                acknowledgements: BTreeMap::new(),
            },
        );
        Ok(RestorePermit {
            gates: self.clone(),
            path,
            request,
            editors,
        })
    }
    pub fn acknowledge(
        &self,
        identity: &EditorIdentity,
        request: &str,
        dirty: bool,
    ) -> Result<(), String> {
        let mut state = self.inner.state.lock();
        if !state
            .mounted
            .get(&(identity.window.clone(), identity.editor.clone()))
            .is_some_and(|m| m.identity == *identity)
        {
            return Err("This editor mount is no longer current".into());
        }
        let gate = state
            .gates
            .values_mut()
            .find(|g| g.request == request)
            .ok_or("Restore preparation has expired")?;
        if !gate.editors.contains(identity) {
            return Err("This editor is not part of the restore".into());
        }
        gate.acknowledgements.insert(identity.clone(), dirty);
        self.inner.wake.notify_all();
        Ok(())
    }
}
impl RestorePermit {
    fn check(&self, state: &State) -> Result<bool, String> {
        let gate = state
            .gates
            .get(&self.path)
            .filter(|g| g.request == self.request)
            .ok_or("Restore preparation expired")?;
        if gate.epoch != state.epoch {
            return Err("An editor or window changed during preparation; refresh and retry".into());
        }
        if gate.acknowledgements.values().any(|dirty| *dirty) {
            return Err(
                "An editor has unsaved changes. Save or discard them before restoring.".into(),
            );
        }
        Ok(gate
            .editors
            .iter()
            .all(|e| gate.acknowledgements.contains_key(e)))
    }
    pub fn wait(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut state = self.gates.inner.state.lock();
        loop {
            if self.check(&state)? {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("An editor did not acknowledge restore preparation. Close or reload that window, then retry.".into());
            }
            self.gates.inner.wake.wait_for(&mut state, remaining);
        }
    }
    /// Membership and acknowledgement checks remain held across the final file mutation.
    pub fn commit<T>(&self, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let state = self.gates.inner.state.lock();
        if !self.check(&state)? {
            return Err("Editors have not finished preparing".into());
        }
        operation()
    }
}
impl Drop for RestorePermit {
    fn drop(&mut self) {
        let mut state = self.gates.inner.state.lock();
        if state
            .gates
            .get(&self.path)
            .is_some_and(|g| g.request == self.request)
        {
            state.gates.remove(&self.path);
        }
        self.gates.inner.wake.notify_all();
    }
}
impl Drop for WriteLease {
    fn drop(&mut self) {
        let mut state = self.gates.inner.state.lock();
        for path in &self.paths {
            if let Some(n) = state.writers.get_mut(path) {
                *n -= 1;
                if *n == 0 {
                    state.writers.remove(path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destroyed_windows_abort_preparation_and_do_not_leave_ghost_participants() {
        let gates = RestoreGates::default();
        let path = PathBuf::from("/project/file");
        let editor = gates.register("detached".into(), "pane".into(), path.clone());
        let permit = gates
            .prepare(path.clone(), &BTreeSet::from(["pane".into()]))
            .unwrap();
        gates.acknowledge(&editor, &permit.request, false).unwrap();
        gates.unregister_window("detached");
        assert!(permit.commit(|| Ok(())).is_err());
        drop(permit);
        let permit = gates.prepare(path, &BTreeSet::new()).unwrap();
        assert!(permit.editors.is_empty());
        permit.wait(Duration::from_millis(1)).unwrap();
    }
    #[test]
    fn dirty_detached_missing_and_replaced_editors_abort_and_drop_releases_gates() {
        let gates = RestoreGates::default();
        let path = PathBuf::from("/project/file");
        let main = gates.register("main".into(), "main-pane".into(), path.clone());
        let detached = gates.register("detached".into(), "detached-pane".into(), path.clone());
        let expected = BTreeSet::from([main.editor.clone(), detached.editor.clone()]);
        let permit = gates.prepare(path.clone(), &expected).unwrap();
        gates.acknowledge(&main, &permit.request, false).unwrap();
        assert!(permit.wait(Duration::from_millis(1)).is_err());
        gates.acknowledge(&detached, &permit.request, true).unwrap();
        assert!(
            permit
                .wait(Duration::from_millis(1))
                .unwrap_err()
                .contains("unsaved")
        );
        assert!(gates.write(vec![path.clone()]).is_err());
        drop(permit);
        assert!(gates.write(vec![path.clone()]).is_ok());
        let permit = gates.prepare(path.clone(), &expected).unwrap();
        gates.acknowledge(&main, &permit.request, false).unwrap();
        gates
            .acknowledge(&detached, &permit.request, false)
            .unwrap();
        permit.wait(Duration::from_millis(1)).unwrap();
        let replacement = gates.register("detached".into(), "detached-pane".into(), path.clone());
        gates.unregister(&detached);
        assert!(permit.commit(|| Ok(())).is_err());
        drop(permit);
        gates.unregister(&replacement);
        assert!(gates.prepare(path, &expected).is_err());
    }
}
