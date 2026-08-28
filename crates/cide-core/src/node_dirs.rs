//! Where Node package managers put global binaries. (M28, extracted in M34)
//!
//! Written for `cide-spec`'s `openspec` search and moved here when `cide-lsp` became its
//! second consumer: extension-contributed language servers (`typescript-language-server`,
//! `vscode-css-language-server`, …) are `npm -g` installs exactly like `openspec`, found in
//! exactly these directories, and `cide-lsp` must not depend on `cide-spec` to know that.
//!
//! # The containment rule
//!
//! These directories are **not** [`crate::toolchain::search_paths`] and must never feed it.
//! `toolchain`'s header states why its list stays short: the directories cide searches to
//! find a binary and the directories it gives that binary's process are one list, so
//! widening it would widen `claude_cli::resolve`'s acceptance. The one-list invariant is
//! honoured *per candidate* here instead — whoever finds a binary in one of these
//! directories must hand that directory to [`crate::child_env::prepare_command_with`],
//! because an npm global is a `#!/usr/bin/env node` script: `execve` succeeds and the
//! shebang dies with `env: node: No such file or directory` unless the child's `PATH`
//! reaches a `node` (`prepare_command_with`'s doc names this exact failure).

use std::path::{Path, PathBuf};

/// Every directory a Node package manager might have put a global binary in, best first.
///
/// Pure over the environment and the home directory it is handed, so the whole list — including
/// the nvm version sort, which is the rung that actually matters — is testable on a machine that
/// has none of them.
///
/// The environment variables come first because a user who has one set has told us where their
/// Node lives; the hardcoded floor is for the desktop launch that inherited nothing at all.
pub fn node_dirs_in(home: Option<&Path>, var: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |dir: PathBuf| {
        if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };

    // Set by the version managers themselves, inside a shell that has sourced their init.
    if let Some(bin) = var("NVM_BIN") {
        push(PathBuf::from(bin));
    }
    if let Some(volta) = var("VOLTA_HOME") {
        push(PathBuf::from(volta).join("bin"));
    }
    if let Some(prefix) = var("N_PREFIX") {
        push(PathBuf::from(prefix).join("bin"));
    }
    // `npm config set prefix`, the manual global-directory move.
    if let Some(prefix) = var("NPM_CONFIG_PREFIX") {
        push(PathBuf::from(prefix).join("bin"));
    }

    let Some(home) = home else {
        return dirs;
    };
    for relative in [
        ".npm-global/bin",
        ".local/share/pnpm",
        "Library/pnpm",
        ".bun/bin",
        ".yarn/bin",
        ".local/bin",
    ] {
        push(home.join(relative));
    }
    dirs
}

/// The nvm version directories under `home`, newest first.
///
/// Separate from [`node_dirs_in`] because it is the one rung that needs the filesystem: nvm's
/// layout is `~/.nvm/versions/node/<version>/bin`, and which `<version>` is current is not
/// something an unset environment can say. Sorted **numerically**, so `v22` beats `v9` — a
/// lexicographic sort puts `v9` last-but-one and would pick a years-old Node to run a package
/// installed under the current one.
pub fn nvm_dirs(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".nvm/versions/node");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut versions: Vec<(Vec<u64>, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let bin = entry.path().join("bin");
            bin.is_dir().then(|| (version_key(&name), bin))
        })
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    versions.into_iter().map(|(_, bin)| bin).collect()
}

/// `v22.21.0` → `[22, 21, 0]`, for a sort that orders versions rather than strings.
///
/// Anything unparseable sorts last rather than panicking: nvm's directory can hold aliases and
/// half-removed installs, and one of those must not be able to hide every real version.
fn version_key(name: &str) -> Vec<u64> {
    name.trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Every Node installation directory on this machine, best first: the environment's own
/// answers, the hardcoded floor, then the nvm versions newest-first.
///
/// The nvm walk is appended after the environment's answers on purpose: a user with `NVM_BIN`
/// set has told us which version they mean, and a newer one on disk must not overrule them.
///
/// Deliberately not cached — `npm install -g <server>` in a terminal pane is exactly what a
/// user does after reading a "not found" refusal, and the panel's Retry has to work without a
/// relaunch. (`toolchain::extra_dirs` caches both directions; the asymmetry is the point.)
pub fn enumerate() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| !h.as_os_str().is_empty());
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());

    let mut dirs = node_dirs_in(home.as_deref(), &var);
    if let Some(home) = home.as_deref() {
        dirs.extend(nvm_dirs(home));
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environments_own_answer_comes_before_the_hardcoded_floor() {
        let home = PathBuf::from("/home/u");
        let var = |name: &str| match name {
            "NVM_BIN" => Some("/home/u/.nvm/versions/node/v20.0.0/bin".to_string()),
            "VOLTA_HOME" => Some("/home/u/.volta".to_string()),
            _ => None,
        };
        let dirs = node_dirs_in(Some(&home), &var);
        assert_eq!(
            dirs[0],
            PathBuf::from("/home/u/.nvm/versions/node/v20.0.0/bin")
        );
        assert_eq!(dirs[1], PathBuf::from("/home/u/.volta/bin"));
        assert!(dirs.contains(&PathBuf::from("/home/u/.npm-global/bin")));
        // An empty variable is not an answer, and a repeat is not a second directory.
        let var = |name: &str| (name == "NVM_BIN").then(|| "/home/u/.npm-global/bin".to_string());
        let dirs = node_dirs_in(Some(&home), &var);
        assert_eq!(
            dirs.iter()
                .filter(|d| d.ends_with(".npm-global/bin"))
                .count(),
            1
        );
    }

    #[test]
    fn nvm_versions_sort_numerically_so_v22_beats_v9() {
        // A lexicographic sort puts `v9` above `v22`, which would run a package installed under
        // the current Node with a years-old one.
        let mut keys = [
            version_key("v9.0.0"),
            version_key("v22.21.0"),
            version_key("v20.1.0"),
        ];
        keys.sort_by(|a, b| b.cmp(a));
        assert_eq!(keys[0], vec![22, 21, 0]);
        assert_eq!(keys[2], vec![9, 0, 0]);
        // And a directory nvm left half-removed cannot hide the real ones.
        assert_eq!(version_key("system"), vec![0]);
    }
}
