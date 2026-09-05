//! `.cide/config.json`: whether this project may run subagents, and under what. (M18)
//!
//! # The default that everything else hangs off
//!
//! **`enabled` is false in the absence of the file.** A project that has never heard of this
//! feature can never spawn anything — not after an upgrade, not because a global setting was
//! flipped for a different repository, not because a config file was half-written. Every read
//! path below funnels a missing, unreadable, truncated or unparseable file to
//! [`AgentsConfig::default`], and that default is off.
//!
//! Reading therefore **cannot fail**, and the direction of the failure is the point. Guessing
//! `true` on a file cide could not parse means unattended `claude` processes in somebody's
//! repository, editing files, spending their quota, with the user having done nothing to ask for
//! it. Guessing `false` means a button does not work until they look at the log line. Those are
//! not comparable costs, so a bad parse is `enabled: false` plus a loud `tracing::warn!` naming
//! the file and the serde error — loud because the *other* failure mode of this decision is a
//! user whose config is silently ignored, and a warning with a path in it is what closes that.
//!
//! # Why this is a file in the repository and not a `Settings` field
//!
//! `cide_ipc::OrchestrationConfig`'s doc argues it at length and it is not repeated here; the
//! short of it is that "this project runs subagents" is a property of the **checkout**. It is
//! reviewable in a pull request, where a teammate can see that a repository has started
//! dispatching agents, and above all it must not follow the user into an unrelated project.
//!
//! Two consequences this module is responsible for:
//!
//! * **Nothing here is mirrored into `Workspace`.** The file is read fresh on demand, because a
//!   teammate's commit or a `git checkout` can change it under the running app, and cide's own
//!   state file holding a stale copy of something git owns is a bug with no upper bound on how
//!   long it lasts. (`cide_fs::filter` already watches `<root>/.cide`, so the change arrives.)
//! * **The disk shape is not the wire shape.** `OrchestrationConfig` is what the panel draws —
//!   three fields. [`AgentsConfig`] is what the file holds, and it carries three more,
//!   [`AgentsConfig::isolation`], [`AgentsConfig::allow_dangerous_permissions`] and
//!   [`AgentsConfig::nudge_orchestrator`], which are dispatch-time and turn-end facts with
//!   nothing for a roster row to show. Modelling them here rather than widening the DTO keeps a
//!   switch the panel cannot draw out of the panel's vocabulary — and, for all three, means a
//!   webview round trip cannot reset a key the user hand-edited.

use std::io;
use std::path::{Path, PathBuf};

use cide_ipc::{Harness, OrchestrationConfig, OrchestrationPatch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The project directory cide keeps its own files in.
///
/// Spelled here as well as in `cide_fs::filter` because these two crates do not depend on each
/// other and a `pub` constant in either would be a dependency edge added for four characters.
/// It is not a value that will change: it is in users' repositories.
pub const CIDE_DIR: &str = ".cide";

/// `.cide/config.json`'s schema version.
///
/// Written on every save and **not** currently enforced on read: a file from a future version
/// still loads, because `serde(default)` fills what this build does not know and refusing it
/// would break a checkout for a user whose teammate upgraded first. The number is here so a
/// migration, when one is needed, has something to switch on.
pub const SCHEMA_VERSION: u32 = 1;

/// How an agent's work is kept apart from the user's tree.
///
/// # Why a second value exists at all when the answer is always `Worktree`
///
/// Because a project that is not a git repository has nothing to build a worktree on, and the
/// alternative to naming that state is a **silent fallback to the shared tree** — which is how
/// two agents clobber one file with nobody told. `agents_config_set` refuses to enable
/// orchestration with `Worktree` isolation outside a repository, with a sentence; a user who
/// genuinely wants agents editing the working tree directly has to write `"isolation": "shared"`
/// and can be assumed to have read what it means.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Isolation {
    /// One git worktree per (role, task), at `.cide/worktrees/<role>-<task>` on branch
    /// `cide/<role>-<task>` — `crate::run_checkout` is the rule. A run dispatched with no task
    /// stands in the project root even here (M40): a quick check or a small piece of direct
    /// work has nowhere to be merged back *from*.
    #[default]
    Worktree,
    /// Every agent edits the project's own checkout. Nothing separates two concurrent runs.
    Shared,
}

/// The `agents` block of `.cide/config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentsConfig {
    /// Whether this project may dispatch subagents at all. See the module header.
    pub enabled: bool,
    /// How many runs may be live across the whole project, whatever a role's own
    /// `max-concurrent` says.
    ///
    /// 2, matching `OrchestrationConfig::default`, and for the reason its doc gives: every run is
    /// a full `claude` with its own context window and its own bill, and a default that started
    /// six of them the first time somebody pressed the button is a default nobody forgives.
    pub max_concurrent: u16,
    /// The harness a role gets when its own definition does not name one.
    ///
    /// Per project rather than global, because which CLI a team runs is a property of the team's
    /// repository — and because a role definition is committed beside this file, so the two are
    /// reviewed together.
    pub harness: Harness,
    /// Whether agents get their own git worktree. Not on the wire; see [`Isolation`].
    pub isolation: Isolation,
    /// Whether a role declaring `permission-mode: bypassPermissions` may actually be dispatched.
    ///
    /// # Why this is a separate switch and not just the role's own setting
    ///
    /// `bypassPermissions` means an unattended process edits, deletes and runs whatever it likes
    /// with nothing to stop it. A role's own definition file is *not* a sufficient place to
    /// authorise that: definition files are committed, so one arrives with a `git pull`, and they
    /// are routinely written by models. Requiring a second, explicit opt-in **in the project's
    /// own config** means the dangerous combination takes two deliberate acts by the person whose
    /// machine it is.
    ///
    /// The refusal is at **dispatch**, never at load — see `crate::dispatch_refusal`. Refusing at
    /// load would make the role vanish from the roster, and a user staring at a missing agent has
    /// no thread to pull; a greyed row that names this key is a fix they can act on.
    ///
    /// [`Self::skip_permissions`] is the stated exception: while it is on (the default), the
    /// project has already declared that every unattended child runs promptless, so refusing a
    /// role for *writing down* the same stance would be a refusal about nothing. The two-acts
    /// rule bites only in a project that switched skipping off — which is exactly the project
    /// that meant to be asked.
    pub allow_dangerous_permissions: bool,
    /// Whether a subagent finishing its turn types one line into the product owner's terminal.
    ///
    /// # The most invasive thing in this design, and it ships on
    ///
    /// A run reports back only through `.cide/tasks.json`, and there is **no out-of-band channel
    /// into a running `claude`** — nothing that can hand a live session a message which is not a
    /// keystroke. So the only way to tell a project's primary session that one of its subagents
    /// has finished a turn is to write into that session's PTY, exactly as a dispatch writes the
    /// opening prompt into a run's: a line appears in the user's own conversation and is
    /// submitted as a turn, with nobody at the keyboard.
    ///
    /// That is worth being uncomfortable about, and the discomfort is the whole reason this key
    /// exists. It is nevertheless **true by default**, because the loop it closes — decompose,
    /// dispatch, read what came back, dispatch the next thing — is the feature M18 was asked for,
    /// and a loop whose last step is *and then the user happens to notice* is not a loop. **The
    /// setting is the way out, not the way in.** A project that wants subagents but does not want
    /// its console typed into writes `"nudgeOrchestrator": false` here and loses nothing it
    /// cannot get back by asking: `mcp__cide__cide_agent_runs` answers the same question, and
    /// answers it as of the moment it is called rather than as of the last thing that happened.
    ///
    /// Disk-only, like [`Self::isolation`] and [`Self::allow_dangerous_permissions`] and for the
    /// same reason — it is deliberately absent from `OrchestrationConfig`, so no webview gesture
    /// can set it and no round trip through the panel can silently reset it either. It is also
    /// read **fresh at the moment of each nudge** rather than cached when a run is dispatched:
    /// this file is committed, so a teammate's commit or a `git checkout` can switch it off under
    /// a running app, and a cached copy would go on typing into somebody who had already said no.
    pub nudge_orchestrator: bool,
    /// Whether assigning a task to a role — or @mentioning one in a task's body or a comment —
    /// starts that role working on it.
    ///
    /// `cide_agents::autodispatch` is the policy (who may trigger, which statuses, what a
    /// mention does); this key is the master switch over all of it. It exists for
    /// [`Self::nudge_orchestrator`]'s reason, sharpened: that key types a line into a terminal,
    /// this one **spawns a billed `claude` process off a task edit**. On by default because the
    /// loop it closes — plan on the board, assign, work starts — is what assignment is *for*;
    /// the setting is the way out, not the way in. A project that wants assignment to stay pure
    /// bookkeeping writes `"autoDispatch": false` here and keeps `cide_agent_dispatch` as the
    /// explicit road.
    ///
    /// Disk-only and read fresh at each trigger, exactly like `nudge_orchestrator` and for the
    /// same two reasons: no panel round trip can silently reset it, and a teammate's commit
    /// switching it off is honoured from the next edit onward.
    pub auto_dispatch: bool,
    /// Whether a dispatched child runs with permission prompts switched off — `--permission-mode
    /// bypassPermissions` for a `claude` run whose role names no mode of its own, `--auto` for
    /// an `opencode` run.
    ///
    /// # On by default, which reverses a stance — and the reversal was measured, not argued
    ///
    /// [`Self::allow_dangerous_permissions`] exists so that a *role file* asking for bypass takes
    /// two deliberate acts, and that rule stands unchanged for what a role asks. This key is
    /// about what happens when the role asks nothing: the child is **unattended**, and an
    /// unattended child cannot answer a prompt. What that costs was measured live, per harness:
    /// `opencode run` auto-rejects the request and the **turn ends right there** — two real runs
    /// died mid-investigation on their first out-of-project command, task never reported, which
    /// reads on the board as an agent that did nothing — and a `claude` run parks in
    /// `AwaitingPermission`, holding its slot and its role's only worktree, until a human
    /// happens to open its pane. Neither is a safety property; both are the autonomy failing
    /// silently. The blast-radius containment for an unattended child is worktree isolation,
    /// which is on by default and refused outside a git repository.
    ///
    /// The role's own `permission-mode:` always wins over this default — an author who wrote a
    /// mode meant it. Disk-only like its three neighbours, read fresh at each spawn (a `git
    /// checkout` flipping it is honoured from the next dispatch), and the way back to prompts is
    /// `"skipPermissions": false` in this file.
    pub skip_permissions: bool,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            // The single most important line in this crate.
            enabled: false,
            max_concurrent: 2,
            harness: Harness::Claude,
            isolation: Isolation::Worktree,
            allow_dangerous_permissions: false,
            // On, and the field's own doc argues it: the setting is the way out, not the way in.
            nudge_orchestrator: true,
            // Same argument, same default: assignment that does nothing is not assignment.
            auto_dispatch: true,
            // On, and the field's own doc carries the measurement that decided it: an
            // unattended child cannot answer a prompt, and both harnesses fail silently on one.
            skip_permissions: true,
        }
    }
}

impl AgentsConfig {
    /// The three fields the panel draws.
    pub fn to_wire(&self) -> OrchestrationConfig {
        OrchestrationConfig {
            enabled: self.enabled,
            max_concurrent: self.max_concurrent,
            harness: self.harness,
        }
    }

    /// Apply a wire patch. `None` means "leave this alone", per `OrchestrationPatch`'s contract.
    ///
    /// The three disk-only fields are untouched by any patch, deliberately: `isolation`,
    /// `allow_dangerous_permissions` and `nudge_orchestrator` are not on the wire, so a UI
    /// gesture cannot set them and a round trip through the panel cannot silently reset them
    /// either. They are hand-edited, and [`write`] preserves what it did not change.
    pub fn apply(&mut self, patch: OrchestrationPatch) {
        if let Some(enabled) = patch.enabled {
            self.enabled = enabled;
        }
        if let Some(max) = patch.max_concurrent {
            // Clamped rather than refused: this arrives from a webview, and the whole point of
            // `cmd::settings::apply_patch`'s clamps is that a nonsense number should land as a
            // sane one rather than as an error the frontend discards. 0 would define a project
            // that can never dispatch, which no user means.
            self.max_concurrent = max.max(1);
        }
        if let Some(harness) = patch.harness {
            self.harness = harness;
        }
    }
}

/// `.cide/config.json`, whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CideConfig {
    pub version: u32,
    pub agents: AgentsConfig,
}

impl Default for CideConfig {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            agents: AgentsConfig::default(),
        }
    }
}

/// `<root>/.cide`.
pub fn cide_dir(project_root: &Path) -> PathBuf {
    project_root.join(CIDE_DIR)
}

/// `<root>/.cide/config.json`.
///
/// Named in full in `AgentRoster::Disabled`, and *before* the enable button, because turning
/// this on writes a file the user's repository will then contain — and a feature toggle that
/// quietly adds a committed file is a surprise commit.
pub fn config_path(project_root: &Path) -> PathBuf {
    cide_dir(project_root).join("config.json")
}

/// Read a project's config. Never fails; see the module header for why that is not laziness.
pub fn load(project_root: &Path) -> CideConfig {
    load_file(&config_path(project_root))
}

/// [`load`] against an explicit path.
pub fn load_file(path: &Path) -> CideConfig {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // The overwhelmingly common case: no file, no feature, nothing to say. Not even a
            // debug line — it would fire for every project the user has ever opened.
            return CideConfig::default();
        }
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "could not read .cide/config.json; subagents stay disabled for this project"
            );
            return CideConfig::default();
        }
    };
    match serde_json::from_slice::<CideConfig>(&bytes) {
        Ok(config) => config,
        Err(err) => {
            // Loud, with the path and the line serde found the problem on. The whole file is
            // discarded rather than partially applied: serde has already stopped, and a config
            // that is half the user's intent and half the defaults is the shape nobody can
            // reason about.
            tracing::warn!(
                path = %path.display(),
                %err,
                "\
                .cide/config.json could not be parsed; subagents stay disabled for this project"
            );
            CideConfig::default()
        }
    }
}

/// Write a project's config, creating `.cide/` if it is not there.
///
/// # What this preserves, and why it is not just `to_vec_pretty(config)`
///
/// The file is **committed and hand-edited**, so it is not cide's to own outright. Two things
/// follow. Keys this build does not know — a field from a newer cide that a teammate's commit
/// introduced — are kept, by merging into the file's existing JSON rather than replacing it;
/// dropping them would mean two people with different versions silently reverting each other's
/// config in alternate commits. And key order is kept, because `serde_json` is built here with
/// `preserve_order`, so a save does not reshuffle a file somebody arranged.
///
/// (A file that could not be parsed at all is replaced. There is nothing in it to preserve that
/// could be preserved *correctly*, and it is already being ignored by [`load_file`].)
///
/// # Why the mode is 0644 and not `persist::write_atomic`'s 0600
///
/// `cide_core::persist::write_atomic` creates at **0600**, and its doc explains why at length:
/// `workspace.json` holds `Settings::proxy`, which can be `http://user:hunter2@proxy:3128`, and
/// a 0644 there is that password readable by every account on the machine. **None of that
/// applies here.** This file lands in the user's repository, is committed, is read by their
/// teammates and is checked out by CI; a 0600 would show up as a mode change in `git status` on
/// a repository where every other file is 0644, and would make the file unreadable to a
/// different user running a build in the same checkout. So the mode differs deliberately, and
/// that is exactly why this cannot call `write_atomic` — the one thing that function's doc says
/// nobody should be re-deciding is the part that has to change.
///
/// Everything else about `write_atomic`'s shape is kept: a sibling temp file (rename is only
/// atomic within one filesystem), `sync_all` before the rename, the directory synced after it,
/// and the temp removed on every failure path. A torn write here would be a config file
/// committed half-written.
pub fn write(project_root: &Path, config: &CideConfig) -> io::Result<()> {
    let path = config_path(project_root);

    let mut doc = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));

    let ours = serde_json::to_value(config).map_err(io::Error::other)?;
    merge_object(&mut doc, &ours);

    let mut body = serde_json::to_vec_pretty(&doc).map_err(io::Error::other)?;
    // A committed text file without a trailing newline shows as "\ No newline at end of file" in
    // every diff of it, for ever.
    body.push(b'\n');
    write_0644(&path, &body)
}

/// Overwrite the keys of `ours` into `doc`, one level deep on nested objects.
///
/// One level is exactly what this file needs (`version`, and the `agents` object) and stopping
/// there is deliberate: a general deep merge would have to decide what to do about arrays, and
/// this file has none. If it grows one, that decision gets made then, with the array in front of
/// whoever makes it.
fn merge_object(doc: &mut Value, ours: &Value) {
    let (Some(target), Some(source)) = (doc.as_object_mut(), ours.as_object()) else {
        *doc = ours.clone();
        return;
    };
    for (key, value) in source {
        match (target.get_mut(key), value) {
            (Some(existing), Value::Object(_)) if existing.is_object() => {
                merge_object(existing, value);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Publish `body` at `path`, world-readable. See [`write`]'s doc for the mode.
fn write_0644(path: &Path, body: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;

    // The pid keeps two cide processes apart; the counter keeps two threads of one apart, which
    // the pid alone does not. Sharing a temp name is not a lost race but a corrupt file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.tmp-{}-{seq}", std::process::id()));

    let write = (|| -> io::Result<()> {
        let mut file = create_shared(&tmp)?;
        file.write_all(body)?;
        // Without this the rename can publish an intact name over contents that never reached
        // the disk — which is the crash this dance exists to survive.
        file.sync_all()
    })();
    if let Err(err) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        // Leaving it behind would accumulate one stray file per failed save, inside a directory
        // the user commits.
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    // The rename is itself a directory modification, and an unsynced one is lost in the same
    // crash.
    let _ = std::fs::File::open(dir).map(|d| d.sync_all());
    Ok(())
}

/// Create the temp file at 0644, whatever the umask says.
///
/// Two steps, and the second is the one that is easy to leave out. `O_CREAT`'s mode is **masked
/// by the process umask** — `open(…, 0o644)` under a umask of 0077 produces 0600 — so the
/// `.mode()` below is a ceiling and not a value. The explicit `set_permissions` is what actually
/// fixes it. `cide_core::persist::create_private` needs no such follow-up precisely because it
/// asks for 0600, which no umask can make *more* permissive; asking for a looser mode is the
/// case where the difference bites.
///
/// Both are applied to the **temp file**, before the rename, because the mode travels with the
/// inode: a chmod after the rename would leave a window in which the published file has whatever
/// the umask produced, and that window is exactly when a `git status` or a CI checkout might
/// look at it.
///
/// Overriding the user's umask is deliberate and is the one place this module does not defer to
/// the environment. A umask of 0077 says "my files are private"; this file is not the user's, it
/// is the repository's — committed, pulled by teammates, and read by a build running as another
/// account in the same checkout. A 0600 config.json is a file the next person cannot read for a
/// reason nothing on their screen would explain.
#[cfg(unix)]
fn create_shared(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
    Ok(file)
}

/// Whatever the platform's default is. cide is a Linux app; this arm exists so the module still
/// compiles elsewhere and claims nothing about permissions.
#[cfg(not(unix))]
fn create_shared(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::create(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch project root, built the way `cide-core`'s tests build theirs — this workspace
    /// has no temp-dir dependency and is not gaining one.
    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-agents-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn put(root: &Path, text: &str) {
        std::fs::create_dir_all(cide_dir(root)).expect("cide dir");
        std::fs::write(config_path(root), text).expect("config");
    }

    /// The default that keeps an upgrade from spawning anything in somebody's repository.
    #[test]
    fn a_project_with_no_config_file_is_disabled() {
        let root = temp("no-config");
        let config = load(&root);
        assert!(!config.agents.enabled);
        assert_eq!(config, CideConfig::default());
        assert_eq!(config.agents.max_concurrent, 2);
        assert_eq!(config.agents.harness, cide_ipc::Harness::Claude);
        assert_eq!(config.agents.isolation, Isolation::Worktree);
        assert!(!config.agents.allow_dangerous_permissions);
        // The one default in this struct that is *on*. See the field's doc: the loop it closes is
        // the feature, and the key is the way out of it rather than the way into it.
        assert!(config.agents.nudge_orchestrator);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The failure mode of guessing `true` is unattended processes in somebody's repository, so
    /// a file cide cannot read means off — never "assume the rest of it said yes".
    #[test]
    fn a_config_that_cannot_be_parsed_is_disabled() {
        let root = temp("bad-config");
        for text in [
            "{ \"agents\": { \"enabled\": true, ",
            "not json at all",
            "",
            "{ \"agents\": { \"enabled\": \"yes\" } }",
            "[]",
        ] {
            put(&root, text);
            let config = load(&root);
            assert!(!config.agents.enabled, "enabled by `{text}`");
            assert_eq!(config, CideConfig::default(), "partially applied `{text}`");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The disk shape, including the two fields the wire has no room for.
    #[test]
    fn a_full_config_reads_back_field_for_field() {
        let root = temp("full-config");
        put(
            &root,
            r#"{ "version": 1,
                "agents": { "enabled": true, "maxConcurrent": 4, "harness": "opencode",
                            "isolation": "shared", "allowDangerousPermissions": true,
                            "nudgeOrchestrator": false } }"#,
        );
        let config = load(&root);
        assert!(config.agents.enabled);
        assert_eq!(config.agents.max_concurrent, 4);
        assert_eq!(config.agents.harness, cide_ipc::Harness::Opencode);
        assert_eq!(config.agents.isolation, Isolation::Shared);
        assert!(config.agents.allow_dangerous_permissions);
        assert!(!config.agents.nudge_orchestrator);

        // The wire shape is the three fields the panel draws, and no more.
        let wire = config.agents.to_wire();
        assert!(wire.enabled);
        assert_eq!(wire.max_concurrent, 4);
        assert_eq!(wire.harness, cide_ipc::Harness::Opencode);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file written by an older cide, or by hand with only the key somebody cared about, still
    /// loads — `serde(default)` fills the rest. That is what keeps a `git pull` from breaking a
    /// checkout.
    #[test]
    fn a_partial_config_takes_the_defaults_for_the_rest() {
        let root = temp("partial-config");
        put(&root, r#"{ "agents": { "enabled": true } }"#);
        let config = load(&root);
        assert!(config.agents.enabled);
        assert_eq!(config.agents.max_concurrent, 2);
        assert_eq!(config.agents.isolation, Isolation::Worktree);
        assert!(config.agents.nudge_orchestrator);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The enable gesture: a file lands in the user's repository, so it has to be one they would
    /// be content to see in a diff.
    #[test]
    fn the_enable_gesture_writes_a_reviewable_file() {
        let root = temp("write-config");
        let mut config = CideConfig::default();
        config.agents.enabled = true;
        write(&root, &config).expect("write");

        let text = std::fs::read_to_string(config_path(&root)).expect("read back");
        assert!(
            text.contains("\n  \"agents\": {"),
            "pretty-printed:\n{text}"
        );
        assert!(text.contains("\"maxConcurrent\": 2"), "camelCase:\n{text}");
        assert!(
            text.ends_with("}\n"),
            "a trailing newline, or every diff says so"
        );
        assert!(
            !text.contains("//"),
            "JSON has no comments; the key names are the documentation"
        );
        assert_eq!(load(&root), config, "what was written is what is read");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(config_path(&root))
                .expect("stat")
                .permissions()
                .mode()
                & 0o777;
            // 0644 and not `persist::write_atomic`'s 0600: this file is committed and read by
            // the team, and there is no proxy password anywhere near it.
            assert_eq!(
                mode, 0o644,
                "a committed file the team has to be able to read"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The file is not cide's to own outright. A key from a newer version, and a hand-edited
    /// field the panel cannot draw, both survive a save.
    #[test]
    fn writing_preserves_what_this_build_does_not_know_about() {
        let root = temp("preserve-config");
        put(
            &root,
            r#"{ "version": 1,
                "somethingElse": { "kept": true },
                "agents": { "enabled": false, "isolation": "shared", "futureKey": 7,
                            "nudgeOrchestrator": false } }"#,
        );

        let mut config = load(&root);
        assert_eq!(config.agents.isolation, Isolation::Shared);
        assert!(!config.agents.nudge_orchestrator);
        config.agents.apply(cide_ipc::OrchestrationPatch {
            enabled: Some(true),
            ..Default::default()
        });
        write(&root, &config).expect("write");

        let text = std::fs::read_to_string(config_path(&root)).expect("read back");
        assert!(text.contains("somethingElse"), "{text}");
        assert!(text.contains("futureKey"), "{text}");
        assert!(text.contains("\"enabled\": true"), "{text}");
        // A hand-edited, wire-invisible field is not reset by a round trip through the panel.
        assert_eq!(load(&root).agents.isolation, Isolation::Shared);
        // And the one whose default is *on*: a project that said no stays saying no across a
        // save, or turning subagents on from the panel would start typing into the console of a
        // team that had explicitly opted out.
        assert!(
            !load(&root).agents.nudge_orchestrator,
            "enabling orchestration re-enabled the nudge somebody had switched off"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file so broken there is nothing to preserve is replaced, and says so by being valid
    /// afterwards.
    #[test]
    fn writing_over_an_unparseable_file_replaces_it() {
        let root = temp("replace-config");
        put(&root, "{{{ not json");
        let mut config = CideConfig::default();
        config.agents.enabled = true;
        write(&root, &config).expect("write");
        assert!(load(&root).agents.enabled);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `None` means "leave this alone", and a nonsense number lands as a sane one rather than as
    /// an error the frontend discards.
    #[test]
    fn a_patch_touches_only_what_it_names() {
        let mut config = AgentsConfig {
            enabled: true,
            max_concurrent: 5,
            harness: cide_ipc::Harness::Opencode,
            isolation: Isolation::Shared,
            allow_dangerous_permissions: true,
            nudge_orchestrator: false,
            auto_dispatch: false,
            skip_permissions: false,
        };
        config.apply(cide_ipc::OrchestrationPatch::default());
        assert_eq!(config.max_concurrent, 5);
        assert!(config.enabled);

        config.apply(cide_ipc::OrchestrationPatch {
            max_concurrent: Some(0),
            ..Default::default()
        });
        assert_eq!(config.max_concurrent, 1, "0 would never dispatch anything");

        // No disk-only field is on the wire, so no patch can reach any of them.
        assert_eq!(config.isolation, Isolation::Shared);
        assert!(config.allow_dangerous_permissions);
        assert!(
            !config.nudge_orchestrator,
            "a round trip through the panel turned the orchestrator nudge back on"
        );
        assert!(
            !config.auto_dispatch,
            "a round trip through the panel turned assignment-starts-work back on"
        );
        assert!(
            !config.skip_permissions,
            "a round trip through the panel turned prompt-skipping back on — this is the one \
             switch where a silent reset re-arms unattended children"
        );
    }

    #[test]
    fn the_config_lives_where_the_rest_of_dot_cide_does() {
        let root = Path::new("/repo");
        assert_eq!(config_path(root), Path::new("/repo/.cide/config.json"));
        assert_eq!(cide_dir(root), Path::new("/repo/.cide"));
    }
}
