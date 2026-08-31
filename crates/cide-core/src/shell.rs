//! Which shell a terminal pane runs, and what it takes to make it a *login* shell. (M35)
//!
//! # The bug
//!
//! Reported from macOS: *"bash and claude problem — it doesn't see all PATH, for example it
//! doesn't see nvm/npm"*. The shell half of that has one cause and it is in this repository:
//! `ui/src/panes/TerminalPane.tsx` named `/bin/bash` as the program for every shell pane,
//! with a comment saying `$SHELL` is not readable from a webview and that Settings would take
//! it over "in M11". It did not, and the default is wrong in a way that is invisible on the
//! machine cide is developed on.
//!
//! Since Catalina the login shell on macOS is `/bin/zsh`, and `/bin/bash` is 3.2.57 — the 2007
//! GPLv2 release Apple still ships. `bash -l` reads `/etc/profile` and `~/.bash_profile`; the
//! user's entire `~/.zshrc` never runs, and with it the `eval "$(/opt/homebrew/bin/brew
//! shellenv)"` and the `nvm.sh` source line that are *the* documented ways those two tools put
//! themselves on `PATH`. So the pane opened a shell the user had never configured and could
//! not explain, in a terminal that on every other machine they own runs zsh.
//!
//! On Linux the same default happens to be right often enough that nothing was noticed.
//!
//! # Why the answer is computed here and not chosen in the webview
//!
//! A webview cannot read `$SHELL`, and it should not: the pane's program is decided at the
//! fork, and the fork is in Rust. `cmd::session::session_spawn` treats an empty `program` as
//! "the user's login shell" and calls [`login_shell`]; `cide-headless` calls the same function,
//! so there is one answer in the workspace rather than the three near-misses there were before
//! (`/bin/bash` in the pane, `$SHELL` or `/bin/sh` in headless).
//!
//! Deliberately **not** a setting. The correct value is a fact about the account, the OS knows
//! it, and a text field that starts out holding the right answer is a text field whose only
//! reachable states are "the right answer" and "a typo".

use std::path::{Path, PathBuf};

/// The argument that makes a shell a *login* shell.
///
/// One flag rather than a per-shell table: `-l` is a login shell for sh, bash, dash, zsh, ksh,
/// fish, tcsh and nushell alike. It is what makes the difference this module exists for — a
/// non-login zsh reads `~/.zshrc` but not `~/.zprofile`, and Homebrew's own instructions put
/// `brew shellenv` in the latter.
pub const LOGIN_ARG: &str = "-l";

/// The shell to run, and the arguments that make it a login shell.
///
/// See [`choose`] for the ladder. The arguments are separate from the program because
/// `SpawnSpec` wants them separately and because a caller that wants a *non*-login shell (a
/// probe running one command, say — see [`crate::login_path`]) needs the program without them.
pub fn login_shell() -> (PathBuf, Vec<String>) {
    (shell(), vec![LOGIN_ARG.to_string()])
}

/// Just the program, with the process's own environment and password entry as the inputs.
pub fn shell() -> PathBuf {
    choose(
        std::env::var_os("SHELL").map(PathBuf::from).as_deref(),
        passwd_shell().as_deref(),
        &crate::toolchain::is_executable,
    )
}

/// The ladder, over inputs handed in rather than read from the process.
///
/// Pure so that every rung is testable on a machine that has none of the shells in question —
/// which is the whole macOS half of this, since the rung that matters there
/// (`/bin/zsh` as the last resort) names a file no Linux box has.
///
/// `$SHELL` first: it is what the user's own terminal emulator honours, so agreeing with it is
/// the only way a cide pane and a Konsole tab can be the same shell. It is checked for being
/// *executable* rather than merely non-empty, because a stale `SHELL` naming a shell that has
/// since been uninstalled is a spawn failure with a confusing message, and the password entry
/// underneath it is usually right.
///
/// The password entry second, because a Finder-launched `.app` on macOS is not guaranteed to
/// have `SHELL` in its environment at all — launchd's is not a login session's.
///
/// The platform default last, and it is a *different* default per platform for the reason in
/// the module header: `/bin/bash` on macOS is a shell nobody there configures. `/bin/sh` is the
/// floor under both, because a machine without it cannot run anything at all.
pub fn choose(
    env_shell: Option<&Path>,
    passwd_shell: Option<&Path>,
    usable: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    let default = if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    };
    for path in [env_shell, passwd_shell].into_iter().flatten() {
        // An empty `SHELL=` is a variable that is set and says nothing. `is_executable`
        // would answer `false` for it anyway; this is here so the reason is stated.
        if !path.as_os_str().is_empty() && usable(path) {
            return path.to_path_buf();
        }
    }
    let default = PathBuf::from(default);
    if usable(&default) {
        return default;
    }
    PathBuf::from("/bin/sh")
}

/// The shell in this account's password entry, or `None`.
///
/// `getpwuid_r` rather than `getpwuid`: the non-reentrant form returns a pointer into a static
/// buffer that the next call anywhere in the process overwrites, and cide has threads. The
/// buffer is sized from `_SC_GETPW_R_SIZE_MAX` with a floor, and `ERANGE` is treated as "no
/// answer" rather than retried in a loop — an account whose passwd entry does not fit in 16 KiB
/// is not a case worth carrying code for, and the ladder above has two more rungs.
#[cfg(unix)]
fn passwd_shell() -> Option<PathBuf> {
    use std::ffi::{CStr, OsString};
    use std::os::unix::ffi::OsStringExt;

    // SAFETY: `sysconf` and `getuid` take no pointers and cannot fail in a way that matters —
    // a negative `sysconf` answer means "no limit published", which the clamp below covers.
    let size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let size = if size <= 0 { 4096 } else { size as usize };
    let size = size.min(16 * 1024);

    let mut buf = vec![0u8; size];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();

    // SAFETY: `pwd` and `found` are live locals, `buf` is a live allocation of `size` bytes,
    // and `getpwuid_r` writes only into those three. A non-zero return or a null `found` means
    // it wrote nothing usable, which is checked before anything is read back.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr().cast(),
            size,
            &mut found,
        )
    };
    if rc != 0 || found.is_null() || pwd.pw_shell.is_null() {
        return None;
    }
    // SAFETY: `found` is non-null, so `pwd` is populated and `pw_shell` points into `buf`,
    // which outlives this borrow.
    let shell = unsafe { CStr::from_ptr(pwd.pw_shell) }.to_bytes().to_vec();
    if shell.is_empty() {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(shell)))
}

#[cfg(not(unix))]
fn passwd_shell() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing_is_usable(_: &Path) -> bool {
        false
    }

    #[test]
    fn the_environment_wins_when_it_names_something_runnable() {
        let chosen = choose(
            Some(Path::new("/usr/bin/fish")),
            Some(Path::new("/bin/zsh")),
            &|p| p == Path::new("/usr/bin/fish") || p == Path::new("/bin/zsh"),
        );
        assert_eq!(chosen, PathBuf::from("/usr/bin/fish"));
    }

    /// The rung that exists for a `SHELL` left behind by an uninstall. Without the
    /// executability check this returns a path that `execve` answers `ENOENT` for, in a pane
    /// whose only output is that error.
    #[test]
    fn a_shell_that_is_not_there_falls_through_to_the_password_entry() {
        let chosen = choose(
            Some(Path::new("/usr/bin/gone")),
            Some(Path::new("/bin/zsh")),
            &|p| p == Path::new("/bin/zsh"),
        );
        assert_eq!(chosen, PathBuf::from("/bin/zsh"));
    }

    /// The macOS case, which is the reported one: a Finder launch with no `SHELL` in the
    /// environment still has to reach zsh rather than the 2007 bash.
    #[test]
    fn the_password_entry_answers_when_the_environment_is_silent() {
        let chosen = choose(None, Some(Path::new("/bin/zsh")), &|p| {
            p == Path::new("/bin/zsh")
        });
        assert_eq!(chosen, PathBuf::from("/bin/zsh"));
    }

    #[test]
    fn an_empty_shell_variable_is_not_an_answer() {
        let chosen = choose(Some(Path::new("")), Some(Path::new("/bin/zsh")), &|p| {
            p == Path::new("/bin/zsh") || p.as_os_str().is_empty()
        });
        assert_eq!(chosen, PathBuf::from("/bin/zsh"));
    }

    /// Both rungs silent. The default is platform-dependent and this asserts the one this
    /// machine builds for; the macOS arm is a `cfg!` in `choose` and is covered by the Darwin
    /// type-check in `docs/platforms.md`, not by a value here.
    #[test]
    fn the_platform_default_is_the_third_rung() {
        let default: &Path = if cfg!(target_os = "macos") {
            Path::new("/bin/zsh")
        } else {
            Path::new("/bin/bash")
        };
        assert_eq!(choose(None, None, &|p| p == default), default.to_path_buf());
    }

    /// A machine with nothing runnable at any of the three. `/bin/sh` is returned unchecked
    /// because there is nothing left to check against, and a spawn failure naming `/bin/sh` is
    /// at least a true statement about the machine.
    #[test]
    fn the_floor_is_bin_sh() {
        assert_eq!(
            choose(None, None, &nothing_is_usable),
            PathBuf::from("/bin/sh")
        );
    }

    /// Not an assertion about a value — the CI machine's shell is not ours to predict — but
    /// about the two things every caller relies on: the ladder terminates in an absolute path,
    /// and the login flag is the first argument.
    #[test]
    fn the_process_answer_is_an_absolute_path_and_a_login_flag() {
        let (program, args) = login_shell();
        assert!(program.is_absolute(), "{}", program.display());
        assert_eq!(args, vec![LOGIN_ARG.to_string()]);
    }
}
