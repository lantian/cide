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

use std::{collections::HashMap, sync::Arc};

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

type HarnessProbes = HashMap<(cide_ipc::Harness, String), Option<String>>;

/// Informational console versions. Settings changes select another key; CLI updates are
/// observed at the next app launch. Missing binaries are cached too, avoiding repeated forks.
#[derive(Clone, Default)]
pub struct HarnessVersions(Arc<Mutex<HarnessProbes>>);

impl HarnessVersions {
    pub fn probe(&self, harness: cide_ipc::ConsoleHarness, binary: &str) -> Option<String> {
        self.get(harness, binary, |binary| {
            let program = cide_core::claude_cli::resolve(binary).ok()?;
            cide_claude::version::probe(&program)
        })
    }

    pub fn get(
        &self,
        harness: cide_ipc::ConsoleHarness,
        binary: &str,
        probe: impl FnOnce(&str) -> Option<String>,
    ) -> Option<String> {
        let key = (harness.harness(), binary.to_string());
        if let Some(version) = self.0.lock().get(&key) {
            return version.clone();
        }
        // As with ClaudeVersion, a slow child must never hold the cache lock.
        let version = probe(binary);
        self.0.lock().insert(key, version.clone());
        version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_versions_cache_each_harness_binary_and_missing_result() {
        use cide_ipc::ConsoleHarness::{Claude, Codex, Opencode};
        let cache = HarnessVersions::default();
        assert_eq!(
            cache.get(Codex, "wrapper", |_| Some("codex-cli 0.1.0".into())),
            Some("codex-cli 0.1.0".into())
        );
        assert_eq!(
            cache.get(Claude, "wrapper", |_| Some("2.1.291 (Claude Code)".into())),
            Some("2.1.291 (Claude Code)".into())
        );
        assert_eq!(
            cache.get(Codex, "other", |_| Some("0.2.0".into())),
            Some("0.2.0".into())
        );
        assert_eq!(cache.get(Opencode, "missing", |_| None), None);
        assert_eq!(
            cache.get(Opencode, "missing", |_| panic!(
                "missing binary was re-probed"
            )),
            None
        );
        assert_eq!(
            cache.get(Codex, "wrapper", |_| panic!("cached Codex was re-probed")),
            Some("codex-cli 0.1.0".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn harness_version_probes_only_the_configured_program_version() {
        use cide_ipc::ConsoleHarness::{Claude, Codex, Opencode};
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("cide-harness-version-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let program = dir.join("custom cli");
        std::fs::write(&program, "#!/bin/sh\n[ \"$#\" = 1 ] && [ \"$1\" = --version ] || exit 2\nprintf ' 0.1.0-beta.2 \\n'\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let binary = program.to_str().unwrap();
        let cache = HarnessVersions::default();
        for harness in [Claude, Codex, Opencode] {
            assert_eq!(cache.probe(harness, binary), Some("0.1.0-beta.2".into()));
        }
        std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        // Re-reading cached information never re-runs a CLI after another gesture.
        assert_eq!(cache.probe(Codex, binary), Some("0.1.0-beta.2".into()));
        assert_eq!(HarnessVersions::default().probe(Codex, binary), None);
        assert_eq!(
            cache.probe(Codex, dir.join("missing").to_str().unwrap()),
            None
        );
        assert_eq!(cache.probe(Codex, ""), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

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
