//! Reading a run's JSON event file, for a harness that reports that way. (M43)
//!
//! `qwen --json-file <path>` writes one stream-json event per line to a path while its TUI paints
//! in the pty — the CLI's dual output mode, and the only state channel it offers that does not
//! mean writing hooks into somebody's settings file. cide hands it a FIFO, made here before the
//! fork, and reads it on a thread of its own into [`AgentRegistry::observe`] as
//! [`Observation::Line`] — exactly what an opencode child's stdout becomes in the stream hook,
//! one stream over. The harness maps the lines; this module only carries them.
//!
//! # Why a FIFO, and why it is opened the way it is
//!
//! A FIFO rather than a file, because a file would have to be tailed: polled for growth with no
//! end-of-writer signal, and left on disk afterwards. A FIFO delivers each line as it is written
//! and answers *no writer* the moment the child is gone.
//!
//! Made **before** the fork ([`make_fifo`]): the CLI opens whatever path it is given, and a
//! path that does not exist yet becomes a regular file. Opened by the reader **non-blocking,
//! read-only** ([`spawn`]): a blocking `O_RDONLY` open waits for a writer, and a child that
//! died before opening its side — a bad flag, a missing binary, an exit in the first
//! millisecond — would leave the thread waiting for ever with nothing to time it out.
//! Non-blocking, the open succeeds at once; a read then answers *would block* while a writer
//! exists and has nothing to say, and *zero bytes* while there is no writer at all — before the
//! child opens its side and after it closes it. The loop tells the two apart with the one fact
//! it can check: whether the child has exited. (The CLI opens its side read-write, its own error
//! text says so, so it never blocks on the order of opens either.)
//!
//! Every line is also appended to the run's post-mortem log, the same file an opencode run's
//! stream is teed into, so a death is debugged from what the child said.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cide_agents::Observation;
use cide_ipc::{RunId, SessionId};
use cide_pty::PtySession;
use tauri::AppHandle;

use crate::agents::{AgentRegistry, RUN_LOG_CAP, RunLog, after_transition, run_logs_dir};

/// How long the reader sleeps when the file has nothing for it. Latency on a state change, so
/// short; every sleep is one wakeup per run, so not shorter.
const POLL: Duration = Duration::from_millis(100);

/// Where a run's event file goes: beside the agent socket, so it lives in this cide's runtime
/// directory and dies with it, or under the temp dir when there is no socket (a test).
pub fn path_for(agent_sock: Option<&Path>, run: RunId) -> PathBuf {
    let dir = agent_sock
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("cide-run-{run}.events"))
}

/// Make the FIFO the child will write into. A stale one from a crashed cide is replaced.
#[cfg(unix)]
pub fn make_fifo(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let _ = std::fs::remove_file(path);
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    // SAFETY: `mkfifo` reads a NUL-terminated path and a mode, touches nothing else, and
    // returns; the `CString` outlives the call.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn make_fifo(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this platform has no FIFOs; a harness that reports through one runs unobserved",
    ))
}

/// Start the reader for one run. Returns at once; the thread ends when the child has exited
/// and the file is drained, and removes the FIFO on its way out.
pub fn spawn(
    app: AppHandle,
    registry: Arc<AgentRegistry>,
    run: RunId,
    session: SessionId,
    pty: Arc<PtySession>,
    path: PathBuf,
) {
    let spawned = std::thread::Builder::new()
        .name(format!("cide-events-{run}"))
        .spawn(move || {
            tap(&app, &registry, run, session, &pty, &path, |line, log| {
                log.append(line);
                if let Some((moved, _)) =
                    registry.observe(Some(&app), session, Observation::Line(line))
                {
                    after_transition(&app, &registry, moved);
                }
            });
            let _ = std::fs::remove_file(&path);
        });
    if let Err(error) = spawned {
        tracing::warn!(%run, %error, "could not start the event tap; this run is unobserved");
    }
}

/// The read loop, with what each line does handed in — so a test can drive it against a
/// shell writing into the FIFO with no app and no registry.
fn tap(
    _app: &AppHandle,
    _registry: &Arc<AgentRegistry>,
    run: RunId,
    _session: SessionId,
    pty: &Arc<PtySession>,
    path: &Path,
    mut each: impl FnMut(&str, &mut RunLog),
) {
    read_lines(
        path,
        || pty.has_exited(),
        &mut |line, log| each(line, log),
        run,
    );
}

/// [`tap`]'s body over any liveness question, which is what makes it testable without a pty.
pub(crate) fn read_lines(
    path: &Path,
    mut exited: impl FnMut() -> bool,
    each: &mut dyn FnMut(&str, &mut RunLog),
    run: RunId,
) {
    let mut log = RunLog::at(run_logs_dir().join(format!("{run}.log")), RUN_LOG_CAP);
    let mut file = {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_NONBLOCK);
        }
        match options.open(path) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(%run, path = %path.display(), %error, "cannot open the event file; this run is unobserved");
                return;
            }
        }
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match file.read(&mut chunk) {
            // No writer: before the child opened its side, or after it closed it. Only the
            // second is the end, and the child's exit is what says which.
            Ok(0) => {
                if exited() {
                    break;
                }
                std::thread::sleep(POLL);
            }
            Ok(n) => {
                pending.extend_from_slice(&chunk[..n]);
                while let Some(at) = pending.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=at).collect();
                    let text = String::from_utf8_lossy(&line);
                    let text = text.trim_end_matches(['\n', '\r']);
                    if !text.is_empty() {
                        each(text, &mut log);
                    }
                }
            }
            // A writer exists and has nothing to say yet.
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::warn!(%run, %error, "the event file stopped reading");
                break;
            }
        }
    }
    // A last line without its terminator is still a line.
    if !pending.is_empty() {
        let text = String::from_utf8_lossy(&pending);
        let text = text.trim_end_matches(['\n', '\r']);
        if !text.is_empty() {
            each(text, &mut log);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A shell writes three events into the FIFO, slowly, and exits; every line reaches the
    /// callback in order, the loop ends when the writer is gone, and the log holds the lines.
    #[test]
    fn lines_written_into_the_fifo_arrive_in_order_and_the_loop_ends_with_the_writer() {
        let dir = std::env::temp_dir().join(format!("cide-tap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("events.fifo");
        make_fifo(&path).expect("mkfifo");
        let run = RunId::new();

        let script = format!(
            "exec 3<>{p}; printf '%s\\n' one >&3; sleep 0.2; printf '%s\\n' two >&3; sleep 0.2; printf '%s' three >&3; exec 3>&-",
            p = path.display()
        );
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .spawn()
            .expect("sh");
        let done = std::sync::Mutex::new(false);
        let mut seen: Vec<String> = Vec::new();
        read_lines(
            &path,
            || {
                if *done.lock().unwrap() {
                    return true;
                }
                match child.try_wait() {
                    Ok(Some(_)) => {
                        *done.lock().unwrap() = true;
                        true
                    }
                    _ => false,
                }
            },
            &mut |line, _log| seen.push(line.to_string()),
            run,
        );
        assert_eq!(seen, vec!["one", "two", "three"]);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(run_logs_dir().join(format!("{run}.log")));
    }
}
