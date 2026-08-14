//! What survives a `SIGKILL` of cide, and what must not.
//!
//! A clean quit runs `lifecycle::shutdown`, which kills every child and drops every handle.
//! A `SIGKILL` — `kill -9`, an OOM kill, a compositor tearing the session down — runs
//! nothing. What is left behind, measured rather than guessed:
//!
//! * every PTY child keeps running. `claude` is a session leader in its own process group
//!   (`portable-pty` calls `setsid` before `exec`), so it does not even get a `SIGHUP` when
//!   the master fd closes; it sits on a dead terminal holding a subscription slot until the
//!   user finds it in `ps`.
//! * `$XDG_RUNTIME_DIR/cide-hooks-<pid>.sock` stays on disk. `HookServer`'s `Drop` is the
//!   only thing that removes it and `Drop` does not run.
//!
//! # Where the first half went, and why
//!
//! [`arm`], [`set_parent_death_signal`], [`on_spawn_thread`] and [`DEATH_SIGNAL`] are
//! **re-exports** — the implementations, and the long explanation of `PR_SET_PDEATHSIG`'s three
//! silent failure modes, now live in [`cide_core::child_env`].
//!
//! They moved when a second long-lived, memory-hungry child appeared. A language server is
//! exactly the shape `PDEATHSIG` exists for — rust-analyzer is 1–4 GB of resident memory with
//! nothing left to talk to once cide is gone — and with the rule living here, `cide-lsp` would
//! have had to depend on the *Claude supervisor* to hand the kernel a pid to kill. That is
//! backwards, and `CLAUDE.md` already names `child_env` as the module every spawn site in the
//! workspace passes through. Putting the second rule beside the first means one dependency
//! answers "what does a spawn site owe a child?" rather than two.
//!
//! The sweep below stayed, because it needs [`cide_ide_mcp::lockfile::pid_is_alive`] and
//! `cide-core` must not depend on the MCP server. See `docs/adr/0008`.
//!
//! The re-exports are not deprecated and not a shim: `arm` beside `sweep_hook_sockets` is how
//! this module reads as one answer to "what does a hard kill leave behind", and every call site
//! in `cide-claude` and `cide-pty` already spells it `orphans::arm`.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
pub use cide_core::child_env::DEATH_SIGNAL;
pub use cide_core::child_env::{arm, on_spawn_thread, set_parent_death_signal};

// --- sweeping abandoned hook sockets ------------------------------------------------------

/// The filename prefix `cide-app::hooks` publishes under. The pid follows it.
pub const HOOK_SOCKET_PREFIX: &str = "cide-hooks-";

/// Where hook sockets live: `$XDG_RUNTIME_DIR`, or the temp directory when it is unset.
///
/// Mirrors `cide_app::hooks::socket_path` exactly, and the duplication is deliberate rather
/// than an oversight: this crate must not depend on the Tauri shell, and the alternative —
/// having the shell pass its own directory in — would leave the sweep unable to run before
/// the shell exists, which is the only moment it is useful.
pub fn hook_socket_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// The socket path for a given pid.
pub fn hook_socket_path(pid: u32) -> PathBuf {
    hook_socket_dir().join(format!("{HOOK_SOCKET_PREFIX}{pid}.sock"))
}

/// Remove `cide-hooks-*.sock` files whose owning process is gone. Returns how many went.
///
/// Called at startup, before the new listener binds. Mirrors
/// [`cide_ide_mcp::lockfile::sweep_stale`] in shape and in caution, and reuses its liveness
/// rule rather than restating it — see [`cide_ide_mcp::lockfile::pid_is_alive`].
///
/// The rules, and why each one is not merely tidiness:
///
/// * the name must be exactly `cide-hooks-<digits>.sock`. The fallback directory is the shared
///   temp directory, so "starts with `cide-`" is not specific enough to delete on.
/// * the pid must be *established* dead, ESRCH and nothing else. `EPERM` is another user's
///   live cide in a shared `/tmp`, and unlinking its socket takes hooks away from a running
///   application that has no idea we exist.
/// * our own socket is protected by the same rule for free: this process is alive.
///
/// A missed sweep costs one stale file; a wrong sweep silently deletes a live control channel.
/// The bias is towards leaving files behind.
pub fn sweep_hook_sockets() -> usize {
    sweep_hook_sockets_in(&hook_socket_dir())
}

/// [`sweep_hook_sockets`] against an explicit directory, so tests never point at the real one.
pub fn sweep_hook_sockets_in(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let Some(pid) = socket_pid(name) else {
            continue;
        };
        if pid_is_alive(pid) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            // Another user's file in a shared `/tmp`, or a race with a second sweep. Neither
            // is worth more than a line.
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "could not remove a stale hook socket");
            }
        }
    }
    removed
}

/// The pid in a hook socket's filename, if this is one of ours.
///
/// A named function with tests rather than a `strip_prefix` chain inline, because every way of
/// getting it slightly wrong widens what the sweep will delete.
fn socket_pid(name: &str) -> Option<u32> {
    let digits = name
        .strip_prefix(HOOK_SOCKET_PREFIX)?
        .strip_suffix(".sock")?;
    // `parse::<u32>` accepts a leading `+`, which would make `cide-hooks-+7.sock` a candidate
    // for deleting pid 7's socket under a name that process never wrote.
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The single liveness rule, borrowed rather than reimplemented.
///
/// Re-exported through a private alias only so the two call sites above read cleanly; the
/// implementation, and the ESRCH-versus-EPERM reasoning behind it, lives in
/// [`cide_ide_mcp::lockfile::pid_is_alive`] and must stay there.
fn pid_is_alive(pid: u32) -> bool {
    cide_ide_mcp::lockfile::pid_is_alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pid that certainly names no process: spawn a child, reap it, reuse its pid. Inventing
    /// a large number risks hitting a real process and turning the assertion into a coin toss.
    fn a_dead_pid() -> u32 {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        child.wait().expect("reap");
        pid
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-orphans-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_socket_from_a_dead_process_is_swept_and_a_live_one_is_kept() {
        let dir = temp_dir("sweep");
        let dead = dir.join(format!("cide-hooks-{}.sock", a_dead_pid()));
        let live = dir.join(format!("cide-hooks-{}.sock", std::process::id()));
        fs::write(&dead, "").expect("write");
        fs::write(&live, "").expect("write");

        assert_eq!(sweep_hook_sockets_in(&dir), 1);
        assert!(!dead.exists(), "a socket with no process was left behind");
        assert!(
            live.exists(),
            "the sweep deleted the socket of a running cide — this one"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_else_in_the_directory_is_a_candidate() {
        // The fallback directory is the shared temp directory. Every name below is one a
        // loose prefix match would have deleted.
        let dir = temp_dir("shape");
        let dead = a_dead_pid();
        let keep = [
            format!("cide-hooks-{dead}.sock.bak"),
            format!("cide-hooks-{dead}"),
            format!("cide-hooks-+{dead}.sock"),
            "cide-hooks-.sock".to_string(),
            "cide-hooks-notapid.sock".to_string(),
            format!("cide-ide-{dead}.sock"),
            format!("{dead}.sock"),
        ];
        for name in &keep {
            fs::write(dir.join(name), "").expect("write");
        }

        assert_eq!(sweep_hook_sockets_in(&dir), 0);
        for name in &keep {
            assert!(dir.join(name).exists(), "{name} was treated as ours");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_name_this_crate_parses_is_the_name_the_app_writes() {
        // `cide_app::hooks::socket_path` formats `cide-hooks-{pid}.sock`. If the two ever
        // drift the sweep silently stops matching anything, which looks exactly like a sweep
        // that has nothing to do.
        let path = hook_socket_path(4242);
        let name = path.file_name().expect("named").to_str().expect("utf-8");
        assert_eq!(name, "cide-hooks-4242.sock");
        assert_eq!(socket_pid(name), Some(4242));
    }

    #[test]
    fn sweeping_a_directory_that_does_not_exist_is_not_an_error() {
        // `XDG_RUNTIME_DIR` can name a directory that has not been created yet on a fresh
        // login, and startup must not care.
        assert_eq!(
            sweep_hook_sockets_in(Path::new("/nonexistent/cide-orphans")),
            0
        );
    }

    /// The re-exports are the API every spawn site in this crate and `cide-pty` still uses.
    ///
    /// A compile-time assertion rather than a behavioural one: the implementations and their
    /// own tests moved to `cide_core::child_env`, and what could still silently break here is
    /// the *path*, not the logic. `orphans::arm` disappearing would be a build error at four
    /// call sites — but only after somebody removed the `pub use`, which is the edit this
    /// guards.
    #[test]
    fn the_arming_rule_is_still_reachable_under_its_old_name() {
        let _: fn(&mut std::process::Command) = arm;
        let _: fn(u32, libc::c_int) = set_parent_death_signal;
        assert_eq!(DEATH_SIGNAL, libc::SIGTERM);
        assert_eq!(on_spawn_thread(|| 6 * 7), 42);
    }
}
