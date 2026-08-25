//! The Claude CLI version probe, cached per configured binary.
//!
//! `claude --version` is a `fork`/`exec` of a Node program — ~70ms warm on a fast machine,
//! and unbounded under memory pressure, which is exactly when a user is judging whether the
//! app feels responsive. Until this cache existed, `app_get_bootstrap` ran the probe inline,
//! synchronously, on the GTK main loop, once per bootstrap — and the frontend hydrated after
//! nearly every gesture, so the fork's cost landed on file switches and pane clicks.
//!
//! The answer changes in exactly two ways, and both invalidate correctly:
//!
//! * the configured binary path moves — `settings_set` is the only writer of
//!   `settings.claude.cli.binary` and calls [`ClaudeVersion::invalidate`] beside its other
//!   before/after comparisons;
//! * the CLI self-updates behind our back — accepted staleness, corrected at the next launch
//!   or binary change. The string is informational (the header, and telling a protocol
//!   change from a bug); nothing gates behaviour on it.

use std::sync::Arc;

use parking_lot::Mutex;

/// One cached answer: the binary that was probed, and what it said (`None` = not runnable).
type Probed = (String, Option<String>);

/// The cached probe. Cloneable so the answer can be read from a `spawn_blocking` closure —
/// the clone shares the cache, which is the point.
#[derive(Clone, Default)]
pub struct ClaudeVersion(Arc<Mutex<Option<Probed>>>);

impl ClaudeVersion {
    /// The version of `binary`, probing on a miss.
    ///
    /// The probe runs **outside** the lock: a racing double-probe costs two cheap forks and
    /// the second answer wins, whereas a fork under the lock would make every concurrent
    /// bootstrap queue behind one child's startup — the exact stall this cache removes.
    pub fn get(&self, binary: &str, probe: impl FnOnce(&str) -> Option<String>) -> Option<String> {
        if let Some((cached, version)) = &*self.0.lock()
            && cached == binary
        {
            return version.clone();
        }
        let version = probe(binary);
        *self.0.lock() = Some((binary.to_string(), version.clone()));
        version
    }

    /// Forget the cached answer. Called when the configured binary changes, before the
    /// re-probe that announces the new answer to every window.
    pub fn invalidate(&self) {
        *self.0.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hit_never_reprobes() {
        let cache = ClaudeVersion::default();
        assert_eq!(
            cache.get("claude", |_| Some("1.0".into())),
            Some("1.0".into())
        );
        // A second read must answer from the cache — the probe here would panic.
        assert_eq!(
            cache.get("claude", |_| panic!("probed on a hit")),
            Some("1.0".into())
        );
    }

    #[test]
    fn a_different_binary_is_a_miss() {
        let cache = ClaudeVersion::default();
        assert_eq!(cache.get("a", |_| Some("1".into())), Some("1".into()));
        assert_eq!(cache.get("b", |_| Some("2".into())), Some("2".into()));
    }

    #[test]
    fn a_missing_binary_caches_its_absence() {
        // `None` is an answer, not a failure to answer: re-forking a probe for a binary
        // already known to be absent would put the fork back on every bootstrap.
        let cache = ClaudeVersion::default();
        assert_eq!(cache.get("claude", |_| None), None);
        assert_eq!(cache.get("claude", |_| panic!("probed on a hit")), None);
    }

    #[test]
    fn invalidate_forces_a_reprobe() {
        let cache = ClaudeVersion::default();
        assert_eq!(cache.get("claude", |_| Some("1".into())), Some("1".into()));
        cache.invalidate();
        assert_eq!(cache.get("claude", |_| Some("2".into())), Some("2".into()));
    }
}
