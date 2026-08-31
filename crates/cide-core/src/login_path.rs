//! The `PATH` the user's login shell believes in, asked once and appended to every child's. (M35)
//!
//! # The bug, and why the static list cannot close it
//!
//! Reported from macOS: *"claude doesn't see my local `~/bin` that is added to PATH"*, and
//! alongside it that nvm and npm are missing from what cide's children can reach.
//!
//! [`crate::toolchain`]'s header records the first half of this failure and the M17 fix for it:
//! a `.app` launched from Finder, the Dock or Spotlight inherits **launchd's** environment —
//! `PATH=/usr/bin:/bin:/usr/sbin:/sbin` — and `path_helper(8)` is a shell mechanism that never
//! runs for it. `toolchain::extra_dirs` answers that with a list of directories cide can name
//! in advance: `~/.cargo/bin`, `~/go/bin`, `/etc/paths`, the Homebrew and MacPorts floor.
//!
//! What no list can name in advance is a directory that exists only inside a sentence in the
//! user's `~/.zshrc`. `export PATH="$HOME/bin:$PATH"` and `source ~/.nvm/nvm.sh` are exactly
//! that: cide cannot guess `~/bin` is wanted, and an nvm `PATH` names one version directory out
//! of however many are installed. `docs/platforms.md` has named this as the principled next
//! step since M17, along with the two conditions it must arrive under, which are the shape of
//! everything below:
//!
//! > It is strictly more accurate — it is the only way to learn a `PATH` that exists solely
//! > inside a `~/.zshrc` — and it loses on two counts […] `$SHELL -ilc 'echo $PATH'` runs the
//! > user's interactive rc, which can block on a prompt, an ssh-agent unlock or a slow network
//! > mount. VS Code carries a timeout, a cancel path and a user-facing *resolving shell
//! > environment failed* dialog because that hangs in the field. […] it should arrive with a
//! > timeout and a log line rather than silently.
//!
//! So: **one** probe per process, on its own thread, behind a hard deadline, with the answer
//! logged. Never on the main thread, never per spawn, and never at all unless [`warm`] is
//! called — which `cide-app` does at startup and nothing else does, so `cide-headless`, every
//! test and every `cargo` invocation in this workspace behave exactly as they did before.
//!
//! # Why the output is delimited rather than trusted
//!
//! An interactive rc file prints things. Version-manager banners, `fortune`, a `motd`, a
//! warning about a deprecated option — all of it lands on the same stdout as the answer. So the
//! probe does not print `$PATH`; it prints [`BEGIN`], then `$PATH`, then [`END`], and this
//! module reads what is between them. Without that, a shell with a chatty rc contributes its
//! banner to `PATH` as a directory name, which fails silently for ever after.
//!
//! # What it is allowed to change
//!
//! Only *additions*, and only appended — [`crate::toolchain::child_path_from`]'s rule, for its
//! reason: nothing cide discovers may shadow a directory the user arranged themselves. A
//! directory already on the process `PATH` is dropped, so on a machine launched from a terminal
//! (which is most Linux launches, including `./run.sh`) this whole module contributes nothing
//! and costs one shell invocation.

use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Set in the probe child's environment so a user's rc file can tell it apart from a terminal.
///
/// Nothing in cide reads it. It exists because the alternative — a user whose `~/.zshrc` starts
/// a tmux session, prints a prompt or asks for a passphrase having no way to notice cide is the
/// caller — is a support conversation with no evidence in it.
pub const PROBE_MARKER: &str = "CIDE_SHELL_PROBE";

/// Setting this to anything disables the probe entirely, in any process.
///
/// The escape hatch this must ship with: a shell that hangs for four seconds on every launch is
/// four seconds of startup, and somebody who knows their rc is the reason should be able to say
/// so without rebuilding.
pub const DISABLE_ENV: &str = "CIDE_NO_SHELL_PATH";

/// How long the shell gets. Past this the child is killed and the launch proceeds without it.
///
/// Four seconds is a compromise between the two failures. Too short and a cold `nvm.sh` on a
/// spinning disk loses the answer it was about to give; too long and a `~/.zshrc` that blocks
/// on an unreachable network mount stalls the first pane spawn by that much. The wait in
/// [`dirs`] is shorter still, so even a probe that runs to the deadline cannot hold a spawn for
/// the whole of it.
pub const DEADLINE: Duration = Duration::from_secs(4);

/// How long a caller waits for a probe that is still running. See [`dirs`].
const WAIT: Duration = Duration::from_secs(3);

/// The markers the probe prints around the answer. See the module header.
const BEGIN: &str = "__cide_path_begin__";
const END: &str = "__cide_path_end__";

/// The published answer, and whether a probe was ever started.
///
/// A `Mutex`/`Condvar` pair rather than a bare `OnceLock` because [`dirs`] has to be able to
/// *wait*: a session spawned in the first second of a launch would otherwise silently get the
/// unaugmented list, which is the same bug this module exists to fix, arriving intermittently.
static ANSWER: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);
static SETTLED: Condvar = Condvar::new();
/// Whether [`warm`] has been called. Read by [`dirs`] to decide between waiting and answering
/// immediately — an unwarmed process has nothing to wait *for*, and must not pause.
static STARTED: OnceLock<()> = OnceLock::new();

/// Start the probe, once per process. Returns immediately.
///
/// Called from `cide-app`'s setup and from nowhere else. A second call is a no-op, so it is
/// safe on a path that might run twice.
///
/// The thread is detached deliberately: [`crate::child_env::arm`]'s contract is that the
/// forking thread outlives the child, and `run_filter_bare` satisfies it by construction (it
/// either waits for the child or kills it before returning), so this thread's only remaining
/// job is to publish and exit.
pub fn warm() {
    if STARTED.set(()).is_err() {
        return;
    }
    if std::env::var_os(DISABLE_ENV).is_some() {
        tracing::debug!("{DISABLE_ENV} is set; not asking the login shell for its PATH");
        publish(Vec::new());
        return;
    }
    if cfg!(not(unix)) {
        publish(Vec::new());
        return;
    }
    std::thread::Builder::new()
        .name("cide-login-path".to_string())
        .spawn(|| publish(probe()))
        // A thread that could not be spawned is a machine in trouble, but not a reason to fail
        // a launch: publish nothing and every child gets exactly the `PATH` it got before M35.
        .map(drop)
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "could not start the login-shell PATH probe");
            publish(Vec::new());
        });
}

/// The directories the login shell had that this process does not, or an empty slice.
///
/// **Empty, immediately, when [`warm`] was never called** — which is every test, every
/// `cide-headless` invocation and every other binary in the workspace. That is what keeps this
/// module opt-in rather than a hidden `fork` under any code path that composes a `PATH`.
///
/// Otherwise: the answer if it has arrived, or a wait of up to [`WAIT`] for it. A caller here is
/// on its way to spawning a child, so the wait is on a command worker or a spawn thread and
/// never on the GTK main loop — and it happens at most for the first few children of a launch.
/// A timed-out wait answers empty rather than blocking again, so a wedged probe costs one
/// three-second pause and not one per spawn.
pub fn dirs() -> Vec<PathBuf> {
    if STARTED.get().is_none() {
        return Vec::new();
    }
    let guard = ANSWER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dirs) = guard.as_ref() {
        return dirs.clone();
    }
    let (guard, _) = SETTLED
        .wait_timeout_while(guard, WAIT, |answer| answer.is_none())
        .unwrap_or_else(|e| e.into_inner());
    guard.clone().unwrap_or_default()
}

fn publish(dirs: Vec<PathBuf>) {
    let mut guard = ANSWER.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(dirs);
    }
    SETTLED.notify_all();
}

/// Run the shell and turn its answer into directories. The impure half.
fn probe() -> Vec<PathBuf> {
    let started = std::time::Instant::now();
    let shell = crate::shell::shell();

    let mut command = std::process::Command::new(&shell);
    // `-l -i -c`, all three. `-l` is what reads `~/.zprofile` and `/etc/profile`, `-i` is what
    // reads `~/.zshrc` and `~/.bashrc` — and `-i` is the one that matters, because that is where
    // `nvm.sh` and every `export PATH=` a person actually writes ends up. `-c` is the command.
    //
    // `printf` rather than `echo` because `echo` is a builtin with three incompatible dialects
    // and one of them eats backslashes.
    command
        .arg("-l")
        .arg("-i")
        .arg("-c")
        .arg(format!(r#"printf '{BEGIN}%s{END}' "$PATH""#))
        .env(PROBE_MARKER, "1");

    let outcome = crate::child_env::run_filter_bare(command, None, DEADLINE);
    let elapsed = started.elapsed();
    let filtered = match outcome {
        Ok(filtered) => filtered,
        Err(error) => {
            tracing::warn!(
                shell = %shell.display(),
                ?elapsed,
                ?error,
                "could not ask the login shell for its PATH"
            );
            return Vec::new();
        }
    };

    let text = String::from_utf8_lossy(&filtered.stdout);
    let Some(reported) = between(&text) else {
        tracing::warn!(
            shell = %shell.display(),
            ?elapsed,
            ok = filtered.ok,
            "the login shell printed no PATH between the markers"
        );
        return Vec::new();
    };

    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let dirs = additions(reported, inherited.to_string_lossy().as_ref());
    // Info, not debug, and it names the directories. This is a mechanism whose failures are
    // invisible from the outside — an empty answer and a perfect one look identical from the
    // pane — so the log line is the only way anybody ever diagnoses it. One line per launch.
    tracing::info!(
        shell = %shell.display(),
        ?elapsed,
        added = ?dirs,
        "asked the login shell for its PATH"
    );
    dirs
}

/// What the shell printed between the markers, if it printed both.
///
/// Tolerates anything before, after or *between* runs of output — an rc file's banner, a
/// warning, a `motd`. Takes the **last** `BEGIN`, so an rc that echoes the command line itself
/// (`set -x`, or a shell tracing what it runs) cannot win over the real answer.
fn between(text: &str) -> Option<&str> {
    let start = text.rfind(BEGIN)? + BEGIN.len();
    let rest = &text[start..];
    let end = rest.find(END)?;
    Some(&rest[..end])
}

/// The entries of `reported` that are absolute and not already in `inherited`, in order.
///
/// Absolute only: a relative entry in a `PATH` names a directory relative to the *child's* cwd,
/// which for a cide child is a project root and not wherever the shell was. Adding one would
/// silently make a project's own `./node_modules/.bin` executable by everything cide spawns,
/// which is not a decision this module gets to make.
///
/// An empty entry is dropped for the same reason [`crate::toolchain::push_unique`] drops one:
/// in a `PATH` it means the current directory.
fn additions(reported: &str, inherited: &str) -> Vec<PathBuf> {
    let had: Vec<&str> = inherited.split(':').collect();
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in reported.split(':') {
        if entry.is_empty() || !entry.starts_with('/') || had.contains(&entry) {
            continue;
        }
        let path = PathBuf::from(entry);
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marker rule, which is the whole reason the probe does not simply read stdout. Every
    /// string here is something a real rc file prints.
    #[test]
    fn a_chatty_rc_file_cannot_contribute_a_directory() {
        let noisy = format!(
            "Now using node v22.21.0 (npm v10.9.4)\n\
             warning: `~/.zshrc:12: command not found: thefuck`\n\
             {BEGIN}/usr/bin:/bin{END}\n"
        );
        assert_eq!(between(&noisy), Some("/usr/bin:/bin"));
    }

    #[test]
    fn no_markers_is_no_answer() {
        assert_eq!(between("/usr/bin:/bin\n"), None);
        assert_eq!(between(&format!("{BEGIN}/usr/bin")), None);
    }

    /// `set -x` in an rc file echoes the command being run, markers and all, before running it.
    /// The real answer is the later one.
    #[test]
    fn the_last_begin_wins_over_an_echoed_command_line() {
        let traced =
            format!("+ printf '{BEGIN}%s{END}' /usr/bin\n{BEGIN}/opt/homebrew/bin:/usr/bin{END}");
        assert_eq!(between(&traced), Some("/opt/homebrew/bin:/usr/bin"));
    }

    /// The reported case: `~/bin` and an nvm version directory exist only in the rc file, and
    /// everything launchd supplied is already there and must not be repeated.
    #[test]
    fn only_the_directories_the_process_does_not_have_are_added() {
        let reported = "/Users/x/.nvm/versions/node/v22.21.0/bin:/Users/x/bin:/usr/bin:/bin";
        let added = additions(reported, "/usr/bin:/bin:/usr/sbin:/sbin");
        assert_eq!(
            added,
            vec![
                PathBuf::from("/Users/x/.nvm/versions/node/v22.21.0/bin"),
                PathBuf::from("/Users/x/bin"),
            ]
        );
    }

    /// A launch from a terminal, where the shell has nothing cide does not already have. The
    /// whole module is then a no-op, which is the common case on Linux.
    #[test]
    fn a_terminal_launch_adds_nothing() {
        assert!(additions("/usr/bin:/bin", "/usr/bin:/bin").is_empty());
    }

    #[test]
    fn relative_and_empty_entries_are_refused() {
        assert!(additions("node_modules/.bin::.", "/usr/bin").is_empty());
    }

    #[test]
    fn a_directory_reported_twice_is_added_once() {
        assert_eq!(
            additions("/opt/bin:/opt/bin", "/usr/bin"),
            vec![PathBuf::from("/opt/bin")]
        );
    }

    /// The guarantee every other binary in the workspace depends on: without [`warm`], this
    /// module never blocks and never forks. Nothing in this test file calls `warm`, and it must
    /// stay that way — `STARTED` is process-global and a test that warmed it would make this
    /// assertion pass or fail depending on test order.
    #[test]
    fn an_unwarmed_process_answers_immediately_and_empty() {
        let started = std::time::Instant::now();
        assert!(dirs().is_empty());
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
