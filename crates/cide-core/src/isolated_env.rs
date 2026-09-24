//! A worktree's own copies of the per-user directories a project's tools write into.
//!
//! Worktrees isolate the source tree and nothing else. A Godot game's `user://` is
//! `$XDG_DATA_HOME/godot/app_userdata/<name>`, a test runner's cache is under `$XDG_CACHE_HOME`,
//! and every concurrent run and every verify of a project shares them. One run's test writing
//! there is another branch's verify failing: the incident that asked for this module was a
//! data-only branch (selfcraft's `cide/content-designer-t-295`, 2026-09-23) refused twice because
//! the guard in the project's own `check.sh` saw Godot drop a `.recovery_mode_lock` and rotate its
//! logs in the shared user dir while two other worktrees were running tests. The refusal then
//! told the orchestrator to hand a correct branch back to an author who could not fix it.
//!
//! So a project may name, in `agents.isolateEnv`, which of [`VARS`] each worktree gets its own
//! copy of. The directory is keyed by the worktree's path, so a run and the verify of that run's
//! branch see the same one — a green run then means a green verify, and not merely "green on a
//! machine that happened to be quiet".
//!
//! # What must stay shared
//!
//! A tool that keeps its login under one of these directories would be logged out by an empty
//! one, and some of them are the harness itself: `opencode` keeps `auth.json` and every session
//! under `$XDG_DATA_HOME/opencode` (mimo under `mimocode`), `gh` and git keep theirs under
//! `$XDG_CONFIG_HOME`. Each isolated directory therefore gets a symlink to the real `<entry>` for
//! every entry of [`DEFAULT_SHARED`] plus the project's `agents.isolateEnvShare` that exists, so
//! the harness, its resume, and cide's own directory keep working and only what nobody listed is
//! private. An entry may be a nested path (`godot/export_templates`), so a project can share one
//! subdirectory of a tool while isolating the rest of it.
//!
//! `HOME` is never on the list: `~/.claude`, ssh keys and every dotfile that is not XDG would
//! go with it.
//!
//! # Where, and for how long
//!
//! Under cide's **cache** directory (`isolated-env/<worktree slug>`), not inside the worktree:
//! anything under the worktree that git does not ignore is an untracked file, and
//! `before_integrate` refuses a checkout with untracked files — the isolation would have made
//! every isolated branch unmergeable. Cache rather than state because everything in there is a
//! test's scratch and deleting it costs a rerun at most.
//!
//! Nothing in cide removes a worktree on a schedule (`cide_git::worktree::remove` has no caller
//! in the app; checkouts go by hand or through `ensure`'s prune), so there is no removal event to
//! hang cleanup on. Each directory carries a [`MARKER`] naming its worktree instead, and
//! [`sweep`] deletes the ones whose worktree is gone. [`prepare`] sweeps, so the leftovers of a
//! deleted worktree last until the next run or verify of any isolating project.
//!
//! # Best effort, loudly
//!
//! A directory that cannot be made is a warning and the variable is left alone. Refusing to
//! start a run because a scratch directory could not be created would turn a hygiene setting
//! into an outage; the warning says which variable was left shared.

use std::path::{Component, Path, PathBuf};

/// The variables a project may isolate, with where each one defaults to under `HOME`.
/// `TMPDIR` has no home default and nothing under it is shared.
pub const VARS: &[(&str, Option<&str>)] = &[
    ("XDG_DATA_HOME", Some(".local/share")),
    ("XDG_CACHE_HOME", Some(".cache")),
    ("XDG_STATE_HOME", Some(".local/state")),
    ("XDG_CONFIG_HOME", Some(".config")),
    ("TMPDIR", None),
];

/// Entries every isolated directory links back to the real one, whatever the project says: the
/// harnesses' own logins and state, and the tools a run needs to push and review. cide's own leaf
/// ([`crate::profile::dir_leaf`]) is added to these by [`prepare`].
///
/// `mimocode` is on it because mimo, the opencode fork, renamed opencode's directory along with
/// its variables: without the link every mimo run would start logged out and its sessions would
/// be filed where `continue` never looks.
pub const DEFAULT_SHARED: &[&str] = &[
    "opencode",
    "mimocode",
    "codex",
    "qwen",
    "claude",
    "claude-cli-nodejs",
    "gh",
    "git",
];

/// The file in each isolated directory naming the worktree it belongs to, which is what
/// [`sweep`] reads — the directory's own name is a lossy slug and cannot be turned back.
pub const MARKER: &str = "worktree";

/// `name` as one of [`VARS`], or the sentence refusing it. The one spelling of the rule, for
/// the reader here and the `cide_agents_config` tool that writes the list — so a model is told
/// at the call, not by a warning in a log it never sees.
pub fn check_var(name: &str) -> Result<&'static str, String> {
    let name = name.trim();
    if name == "HOME" {
        return Err(
            "`HOME` is never isolated: ~/.claude, ssh keys and every dotfile that is not XDG \
             would go with it"
                .to_string(),
        );
    }
    VARS.iter()
        .map(|(var, _)| *var)
        .find(|var| *var == name)
        .ok_or_else(|| {
            format!(
                "`{name}` is not a variable cide isolates; the ones it does are {}",
                VARS.iter().map(|(v, _)| *v).collect::<Vec<_>>().join(", ")
            )
        })
}

/// Whether `entry` may be an `isolateEnvShare` entry: a relative path of plain components, so
/// a link can never point outside the real directory. Same two readers as [`check_var`].
pub fn check_share(entry: &str) -> Result<(), String> {
    let relative = Path::new(entry.trim());
    let plain = !relative.as_os_str().is_empty()
        && relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(format!(
            "`{}` is not a relative path of plain names (like `gh` or `godot/export_templates`)",
            entry.trim()
        ))
    }
}

/// Where every worktree's isolated directories live.
fn base() -> PathBuf {
    crate::persist::cache_dir().join("isolated-env")
}

/// Where the isolated directories of `worktree` live.
#[must_use]
pub fn root_for(worktree: &Path) -> PathBuf {
    base().join(slug(worktree))
}

/// Make `worktree`'s directories for `vars` and answer the environment that points a child at
/// them. Idempotent: called before every run and every verify, it repairs what a person deleted
/// and adds a share entry the project listed since.
///
/// A name not in [`VARS`] is skipped with a warning, so a typo in the config says so instead of
/// isolating nothing silently.
#[must_use]
pub fn prepare(worktree: &Path, vars: &[String], share: &[String]) -> Vec<(String, String)> {
    if vars.is_empty() {
        return Vec::new();
    }
    let base = base();
    sweep_in(&base);
    let mut shared: Vec<&str> = DEFAULT_SHARED.to_vec();
    shared.push(crate::profile::dir_leaf());
    shared.extend(share.iter().map(String::as_str));
    prepare_in(&base, worktree, vars, &shared, real_base)
}

/// [`prepare`] against an explicit base and a given idea of where the real directories are, so
/// a test touches neither the user's cache nor their home.
fn prepare_in(
    base: &Path,
    worktree: &Path,
    vars: &[String],
    shared: &[&str],
    real: impl Fn(&str, &str) -> Option<PathBuf>,
) -> Vec<(String, String)> {
    let root = base.join(slug(worktree));
    let mut env = Vec::new();
    for var in vars {
        let var = var.trim();
        let Some((name, home_default)) = VARS.iter().find(|(name, _)| *name == var) else {
            if let Err(why) = check_var(var) {
                tracing::warn!("agents.isolateEnv: {why}; skipped");
            }
            continue;
        };
        let dir = root.join(name.to_ascii_lowercase());
        if let Err(error) = std::fs::create_dir_all(&dir) {
            tracing::warn!(var = name, dir = %dir.display(), %error, "cannot make an isolated directory; this variable stays shared");
            continue;
        }
        if let Some(real) = home_default.and_then(|fallback| real(name, fallback)) {
            for entry in shared {
                link_shared(&real, &dir, entry);
            }
        }
        env.push(((*name).to_string(), dir.to_string_lossy().into_owned()));
    }
    if !env.is_empty() {
        // Written every time rather than once: cheap, and a marker a person deleted with half
        // the directory would otherwise leave the rest for ever.
        let _ = std::fs::write(
            root.join(MARKER),
            canonical(worktree).to_string_lossy().as_bytes(),
        );
    }
    env
}

/// Delete `worktree`'s isolated directories. For a caller that removes a worktree while it still
/// exists; everything else is [`sweep`]'s.
pub fn remove(worktree: &Path) {
    let root = root_for(worktree);
    if root.is_dir()
        && let Err(error) = std::fs::remove_dir_all(&root)
    {
        tracing::warn!(dir = %root.display(), %error, "cannot remove a worktree's isolated directories");
    }
}

/// Delete the isolated directories of every worktree that no longer exists.
pub fn sweep() {
    sweep_in(&base());
}

fn sweep_in(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        // No marker is a directory mid-`prepare` or one this module did not write: left alone,
        // since deleting a directory another thread is about to fill is the race to avoid.
        let Ok(owner) = std::fs::read_to_string(dir.join(MARKER)) else {
            continue;
        };
        if Path::new(owner.trim()).is_dir() {
            continue;
        }
        tracing::info!(dir = %dir.display(), worktree = owner.trim(), "removing the isolated directories of a worktree that is gone");
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), %error, "cannot remove a gone worktree's isolated directories");
        }
    }
}

/// The directory `name` means for cide's own process: the variable when it is absolute (the XDG
/// rule), else `HOME/<fallback>`.
fn real_base(name: &str, fallback: &str) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(name) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Some(path);
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback))
}

/// Link `dir/<entry>` to `real/<entry>` when the real one exists and nothing is at the link yet.
/// Something already there — a link from an earlier call, or a directory a tool made before the
/// entry was listed — is left alone: replacing a directory would delete a run's files.
fn link_shared(real: &Path, dir: &Path, entry: &str) {
    if let Err(why) = check_share(entry) {
        tracing::warn!("agents.isolateEnvShare: {why}; skipped");
        return;
    }
    let relative = Path::new(entry.trim());
    let target = real.join(relative);
    let link = dir.join(relative);
    if std::fs::symlink_metadata(&target).is_err() || std::fs::symlink_metadata(&link).is_ok() {
        return;
    }
    if let Some(parent) = link.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    #[cfg(unix)]
    if let Err(error) = std::os::unix::fs::symlink(&target, &link) {
        tracing::warn!(link = %link.display(), %error, "cannot link a shared entry into an isolated directory");
    }
}

/// `worktree`'s path, canonical when it exists, so the run (which may be handed the path through
/// a symlinked root) and the verify agree on one directory.
fn canonical(worktree: &Path) -> PathBuf {
    std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf())
}

/// A file name for `worktree`'s path.
fn slug(worktree: &Path) -> String {
    let slug: String = canonical(worktree)
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    slug.trim_matches('_').to_string()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cide-isolated-env-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn vars(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn nothing_listed_is_nothing_changed() {
        assert!(prepare(Path::new("/nonexistent/wt"), &[], &[]).is_empty());
    }

    #[test]
    fn two_worktrees_get_two_directories_and_one_worktree_gets_the_same_one_twice() {
        let scratch = scratch("pair");
        let base = scratch.join("base");
        let (a, b) = (scratch.join("a"), scratch.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let vars = vars(&["XDG_DATA_HOME", "TMPDIR"]);
        let none = |_: &str, _: &str| None;
        let env_a = prepare_in(&base, &a, &vars, &[], none);
        let env_b = prepare_in(&base, &b, &vars, &[], none);
        assert_eq!(env_a.len(), 2, "{env_a:?}");
        assert_ne!(
            env_a, env_b,
            "a run on another worktree must not see this one's files"
        );
        assert_eq!(
            env_a,
            prepare_in(&base, &a, &vars, &[], none),
            "a run and the verify of its branch share one directory"
        );
        let data_a = PathBuf::from(&env_a[0].1);
        std::fs::write(data_a.join("scratch"), b"x").unwrap();
        assert!(!PathBuf::from(&env_b[0].1).join("scratch").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// The acceptance the feature was asked for, through the same `check::run_logged` a verify
    /// uses: two runs write their app's user data, and a verify on a third worktree sees neither.
    #[test]
    fn two_runs_writing_user_data_are_invisible_to_each_other_and_to_a_third_verify() {
        let scratch = scratch("accept");
        let base = scratch.join("base");
        let vars = vars(&["XDG_DATA_HOME"]);
        let write =
            "mkdir -p \"$XDG_DATA_HOME/app\" && touch \"$XDG_DATA_HOME/app/$(basename \"$PWD\")\"";
        let mut envs = Vec::new();
        for name in ["run-a", "run-b", "verify-c"] {
            let tree = scratch.join(name);
            std::fs::create_dir_all(&tree).unwrap();
            let env: Vec<(String, Option<String>)> =
                prepare_in(&base, &tree, &vars, &[], |_, _| None)
                    .into_iter()
                    .map(|(k, v)| (k, Some(v)))
                    .collect();
            envs.push((tree, env));
        }
        for (tree, env) in &envs[..2] {
            let wrote = crate::check::run_logged(
                tree,
                write,
                std::time::Duration::from_secs(20),
                None,
                env,
            );
            assert!(wrote.passed, "{wrote:?}");
        }
        let (tree, env) = &envs[2];
        let seen = crate::check::run_logged(
            tree,
            "ls -A \"$XDG_DATA_HOME/app\" 2>/dev/null | wc -l",
            std::time::Duration::from_secs(20),
            None,
            env,
        );
        assert_eq!(
            seen.tail.trim(),
            "0",
            "the third worktree saw another's files"
        );
        let a = PathBuf::from(envs[0].1[0].1.as_deref().unwrap());
        assert!(a.join("app/run-a").is_file() && !a.join("app/run-b").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_gone_worktree_is_swept_and_a_live_one_is_kept() {
        let scratch = scratch("sweep");
        let base = scratch.join("base");
        let (live, gone) = (scratch.join("live"), scratch.join("gone"));
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(&gone).unwrap();
        let vars = vars(&["XDG_CACHE_HOME"]);
        let none = |_: &str, _: &str| None;
        let live_env = prepare_in(&base, &live, &vars, &[], none);
        let gone_env = prepare_in(&base, &gone, &vars, &[], none);
        // A directory with no marker is somebody else's, or one being made right now.
        std::fs::create_dir_all(base.join("unmarked")).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        sweep_in(&base);
        assert!(PathBuf::from(&live_env[0].1).is_dir(), "live worktree kept");
        assert!(
            !PathBuf::from(&gone_env[0].1).exists(),
            "a removed worktree's directories go with it"
        );
        assert!(base.join("unmarked").is_dir());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn an_unknown_variable_is_skipped_not_isolated_by_accident() {
        let scratch = scratch("unknown");
        let env = prepare_in(
            &scratch.join("base"),
            &scratch,
            &vars(&["HOME", "XDG_DATA"]),
            &[],
            |_, _| None,
        );
        assert!(env.is_empty(), "{env:?}");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn the_harness_logins_are_shared_by_default() {
        for harness in ["opencode", "mimocode", "codex", "qwen", "claude"] {
            assert!(DEFAULT_SHARED.contains(&harness), "{harness}");
        }
        let scratch = scratch("logins");
        let real = scratch.join("real");
        std::fs::create_dir_all(real.join("mimocode")).unwrap();
        let env = prepare_in(
            &scratch.join("base"),
            &scratch,
            &vars(&["XDG_DATA_HOME"]),
            DEFAULT_SHARED,
            |_, _| Some(real.clone()),
        );
        assert_eq!(
            std::fs::read_link(PathBuf::from(&env[0].1).join("mimocode")).unwrap(),
            real.join("mimocode")
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_shared_entry_is_a_link_to_the_real_one_and_a_bad_one_is_refused() {
        let base = scratch("share");
        let (real, dir) = (base.join("real"), base.join("iso"));
        std::fs::create_dir_all(real.join("opencode")).unwrap();
        std::fs::create_dir_all(real.join("godot/export_templates")).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        link_shared(&real, &dir, "opencode");
        link_shared(&real, &dir, "godot/export_templates");
        link_shared(&real, &dir, "../escape");
        link_shared(&real, &dir, "missing");
        assert_eq!(
            std::fs::read_link(dir.join("opencode")).unwrap(),
            real.join("opencode")
        );
        assert!(
            dir.join("godot").is_dir()
                && std::fs::read_link(dir.join("godot/export_templates")).is_ok(),
            "a nested entry shares only that subdirectory"
        );
        assert!(std::fs::symlink_metadata(base.join("escape")).is_err());
        assert!(std::fs::symlink_metadata(dir.join("missing")).is_err());
        // Idempotent, and a directory already there is never replaced.
        link_shared(&real, &dir, "opencode");
        std::fs::create_dir_all(real.join("gh")).unwrap();
        std::fs::create_dir_all(dir.join("gh")).unwrap();
        link_shared(&real, &dir, "gh");
        assert!(std::fs::read_link(dir.join("gh")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
