//! A shell pane's command history survives its child. Screen replay cannot restore Readline's
//! history, and a new Bash otherwise reads the shared `~/.bash_history` on every app launch.
//!
//! The initializer runs through the first `PROMPT_COMMAND` in an interactive login shell whose
//! automatic profile loading is disabled. It sources the normal login profiles, then reloads
//! the pane history. Apple's Bash bypasses the POSIX `ENV` startup entry, so that entry cannot
//! be used to install the hook. No user startup file is edited.

use std::fs;
use std::path::{Path, PathBuf};

use cide_ipc::PaneId;

use crate::{Result, persist};

pub const BASH_ARGS: [&str; 3] = ["--login", "--noprofile", "-i"];
const INIT: &str = include_str!("bash_history.sh");

pub struct HistoryFiles {
    pub history: PathBuf,
    pub init: PathBuf,
}

/// Called on the blocking spawn pool: resolving symlinks and preparing state both touch disk.
pub fn is_bash(program: &str) -> bool {
    is_shell(program, "bash")
}

/// macOS's login shell since Catalina; without this branch its panels shared `~/.zsh_history`.
pub fn is_zsh(program: &str) -> bool {
    is_shell(program, "zsh")
}

fn is_shell(program: &str, shell: &str) -> bool {
    let path = Path::new(program);
    path.file_name().is_some_and(|name| name == shell)
        || fs::canonicalize(path)
            .ok()
            .is_some_and(|path| path.file_name().is_some_and(|name| name == shell))
}

pub const ZSH_ARGS: [&str; 2] = ["-l", "-i"];
const ZSHENV: &str = include_str!("zsh_history/zshenv");
const ZPROFILE: &str = include_str!("zsh_history/zprofile");
const ZSHRC: &str = include_str!("zsh_history/zshrc");

pub struct ZshFiles {
    pub history: PathBuf,
    pub zdotdir: PathBuf,
}

pub fn prepare_zsh(pane: PaneId) -> Result<ZshFiles> {
    prepare_zsh_in(&persist::state_dir().join("shell-history"), pane)
}

/// Zsh reads its startup files from `$ZDOTDIR`, so a shim directory that forwards to the
/// user's real files is the one hook that needs no edit to them. Content-addressed for the
/// same reason as the Bash initializer.
pub fn prepare_zsh_in(dir: &Path, pane: PaneId) -> Result<ZshFiles> {
    let history = prepare_in(dir, pane)?.history;
    let hash = blake3::hash([ZSHENV, ZPROFILE, ZSHRC].join("\0").as_bytes()).to_hex();
    let zdotdir = dir.join(format!("zdotdir-{hash}"));
    if !zdotdir.join(".zshrc").is_file() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&zdotdir)?;
        persist::write_atomic(&zdotdir.join(".zshenv"), ZSHENV.as_bytes())?;
        persist::write_atomic(&zdotdir.join(".zprofile"), ZPROFILE.as_bytes())?;
        // Last: its presence marks the directory complete.
        persist::write_atomic(&zdotdir.join(".zshrc"), ZSHRC.as_bytes())?;
    }
    Ok(ZshFiles { history, zdotdir })
}

impl ZshFiles {
    pub fn env(&self, original_zdotdir: Option<&str>) -> Vec<(String, String)> {
        let path = |p: &Path| p.to_string_lossy().into_owned();
        vec![
            ("ZDOTDIR".into(), path(&self.zdotdir)),
            ("_CIDE_ZSH_SHIM".into(), path(&self.zdotdir)),
            ("HISTFILE".into(), path(&self.history)),
            ("_CIDE_ZSH_HISTORY_FILE".into(), path(&self.history)),
            (
                "_CIDE_ZSH_ZDOTDIR_WAS_SET".into(),
                if original_zdotdir.is_some() { "1" } else { "0" }.into(),
            ),
            (
                "_CIDE_ZSH_ORIG_ZDOTDIR".into(),
                original_zdotdir.unwrap_or_default().into(),
            ),
        ]
    }
}

pub fn prepare(pane: PaneId) -> Result<HistoryFiles> {
    prepare_in(&persist::state_dir().join("shell-history"), pane)
}

/// An explicit directory keeps tests and other profile owners off the process-global state.
pub fn prepare_in(dir: &Path, pane: PaneId) -> Result<HistoryFiles> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;

    let history = dir.join(format!("{pane}.history"));
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(persist::PRIVATE_MODE);
    }
    options.open(&history)?;

    // Content-addressing lets two concurrent spawns publish the same initializer safely and
    // keeps a new version from changing a shell whose startup has not read its script yet.
    let init = dir.join(format!(
        "bash-init-{}.sh",
        blake3::hash(INIT.as_bytes()).to_hex()
    ));
    if !init.is_file() {
        persist::write_atomic(&init, INIT.as_bytes())?;
    }
    Ok(HistoryFiles { history, init })
}

impl HistoryFiles {
    /// Bootstrap before the first prompt; user profiles install the subsequent prompt hooks.
    pub fn env(
        &self,
        original_env: Option<&str>,
        original_prompt_command: Option<&str>,
    ) -> Vec<(String, String)> {
        vec![
            // Expand a quoted variable once so special characters in the path stay literal.
            (
                "PROMPT_COMMAND".into(),
                "builtin source \"$_CIDE_BASH_INIT_FILE\"".into(),
            ),
            // Bash loads history before the first prompt, before our initializer runs.
            (
                "HISTFILE".into(),
                self.history.to_string_lossy().into_owned(),
            ),
            (
                "_CIDE_BASH_INIT_FILE".into(),
                self.init.to_string_lossy().into_owned(),
            ),
            (
                "_CIDE_BASH_HISTORY_FILE".into(),
                self.history.to_string_lossy().into_owned(),
            ),
            (
                "_CIDE_BASH_ENV_WAS_SET".into(),
                if original_env.is_some() { "1" } else { "0" }.into(),
            ),
            (
                "_CIDE_BASH_ORIGINAL_ENV".into(),
                original_env.unwrap_or_default().into(),
            ),
            (
                "_CIDE_BASH_PROMPT_COMMAND_WAS_SET".into(),
                if original_prompt_command.is_some() {
                    "1"
                } else {
                    "0"
                }
                .into(),
            ),
            (
                "_CIDE_BASH_ORIGINAL_PROMPT_COMMAND".into(),
                original_prompt_command.unwrap_or_default().into(),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparing_again_keeps_commands_and_isolates_panes() {
        let dir = std::env::temp_dir().join(format!("cide-history-{}", PaneId::new()));
        let pane = PaneId::new();
        let first = prepare_in(&dir, pane).unwrap();
        fs::write(&first.history, "echo retained\n").unwrap();
        let again = prepare_in(&dir, pane).unwrap();
        assert_eq!(first.history, again.history);
        assert_eq!(
            fs::read_to_string(&again.history).unwrap(),
            "echo retained\n"
        );
        let other = prepare_in(&dir, PaneId::new()).unwrap();
        assert_ne!(first.history, other.history);
        assert_eq!(fs::read_to_string(other.history).unwrap(), "");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for path in [&first.history, &first.init] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zsh_shim_is_shared_and_history_is_per_pane() {
        let dir = std::env::temp_dir().join(format!("cide-zsh-history-{}", PaneId::new()));
        let a = prepare_zsh_in(&dir, PaneId::new()).unwrap();
        let b = prepare_zsh_in(&dir, PaneId::new()).unwrap();
        assert_eq!(a.zdotdir, b.zdotdir);
        assert_ne!(a.history, b.history);
        for name in [".zshenv", ".zprofile", ".zshrc"] {
            assert!(a.zdotdir.join(name).is_file());
        }
        assert!(is_zsh("/bin/zsh") && !is_zsh("/bin/bash") && is_bash("/bin/bash"));
        fs::remove_dir_all(dir).unwrap();
    }
}
