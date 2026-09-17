//! A container's terminal and its logs, as ordinary `cide-pty` sessions. (M42 — ADR 0013)
//!
//! # Why these are sessions and not a second terminal stack
//!
//! Because everything a pane needs from a stream of terminal bytes is already written, once, in
//! `cide-pty`: coalescing to ≥8 KiB or 8 ms — which is a *correctness* requirement and not tuning,
//! see that crate's header on `webview.eval` — the vt100 mirror that makes a reattach gapless, the
//! sink list that makes detach-into-a-window free, and `CreditPolicy`'s per-sink backpressure with
//! its watchdog. An exec pane that reimplemented that would be a second answer to every one of
//! those questions, and the second answer is the one nobody tests.
//!
//! So M42 put a [`cide_pty::Transport`] seam in that crate, and this module is its second
//! implementation. An exec and a log follow reach `PtySession::connect` with a reader, a writer
//! and a way to be waited on, and get the rest.
//!
//! # What a container's TTY decides
//!
//! Everything, for logs. A container started **without** a TTY has its output multiplexed by the
//! daemon into 8-byte-framed stdout and stderr; one started **with** one is raw. Reading one as the
//! other paints a screenful of control characters. bollard demultiplexes either way — the stream
//! yields `LogOutput::StdOut`/`StdErr`/`Console` — so the framing is never cide's problem, and the
//! variant is deliberately **not branched on**: every one is written through unchanged. That is the
//! point. cide never has to know, and so cannot get it wrong.
//!
//! For **exec** the TTY is ours to choose, and it is always on: the pane is an xterm, and an exec
//! without one gives a shell with no prompt, no line editing and no job control.

use std::sync::Arc;

use cide_pty::{Exit, Geometry, PtySession, SpawnSpec, Transport};
use futures_util::StreamExt as _;

use crate::stream::{ChannelWriter, DiscardWriter, pipe, send_chunk};
use crate::{Docker, DockerError};

/// How a container's shell is chosen when the caller does not name one.
///
/// # Why a ladder, and why `sh` is last rather than first
///
/// The first one that exists is the best one the user could have got, and the difference is line
/// editing and a prompt worth reading. `docker exec -it x bash` failing on an Alpine image is the
/// most common complaint about every Docker UI, and falling straight to `sh` on an image that has
/// `bash` is the same mistake pointing the other way.
pub const SHELL_LADDER: &[&str] = &["/bin/bash", "/bin/zsh", "/bin/ash", "/bin/sh"];

/// How long a non-interactive exec — the shell probe, and M44's directory listings — may take.
const CAPTURE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

/// The shell probe, as one `sh -c` line.
///
/// One exec rather than one per candidate: an exec is a create *and* a start, and four round trips
/// to answer a question the container can answer itself is four times the latency before the pane
/// paints anything. `command -v` was the alternative and `[ -x ]` was chosen because a shell that
/// is present but not executable should not be offered; both are POSIX builtins, so this works in
/// an image with no `which`.
fn shell_probe() -> Vec<String> {
    let tests = SHELL_LADDER
        .iter()
        .map(|shell| format!("[ -x {shell} ] && echo {shell}"))
        .collect::<Vec<_>>()
        .join(" || ");
    vec!["/bin/sh".to_string(), "-c".to_string(), tests]
}

/// Pick the shell out of what the probe printed.
///
/// Pure, and separated from the exec that produces the text for `cide_spec::cli`'s reason: the
/// shape of a real answer can then be a unit test on a machine with no daemon.
///
/// The **last** matching line, not the first, and not the whole output: a shell's own startup
/// noise reaches the same capture, and `||` in the probe means at most one candidate is echoed —
/// so anything else on the stream is noise that must not be mistaken for the answer.
fn shell_from_probe(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .rfind(|line| SHELL_LADDER.contains(line))
        .map(str::to_string)
}

/// The sentence for an image with no shell at all.
fn no_shell() -> DockerError {
    // Named as what it is, with the reason, because "exec failed" on a distroless image sends
    // people looking for a bug in cide.
    DockerError::Refused(format!(
        "the image this container runs has no shell — cide looked for {}. A distroless or \
         `scratch` image has no interactive terminal to open.",
        SHELL_LADDER.join(", ")
    ))
}

impl Docker {
    /// Run a command in a container and collect what it printed.
    ///
    /// Non-interactive and deadline-bounded: the shell probe below, and M44's directory listings.
    /// stdout and stderr are **merged**, deliberately — a caller here is parsing output it asked
    /// for, and a command that failed usually explains itself on stderr.
    pub fn exec_capture(&self, container: &str, command: &[String]) -> Result<String, DockerError> {
        let (client, runtime) = self.parts();
        runtime.block_on(async {
            let created = client
                .create_exec(
                    container,
                    bollard::models::ExecConfig {
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        // No TTY: this output is parsed, and a TTY would put the container's own
                        // line discipline between the command and the answer — `\r\n` on every
                        // line, and column-wrapping on a long one.
                        tty: Some(false),
                        cmd: Some(command.to_vec()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;

            let started = client
                .start_exec(&created.id, None)
                .await
                .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;

            let bollard::exec::StartExecResults::Attached { mut output, .. } = started else {
                return Err(DockerError::Refused(
                    "the daemon detached an exec cide asked to attach to".to_string(),
                ));
            };

            let collect = async {
                let mut text = String::new();
                while let Some(chunk) = output.next().await {
                    match chunk {
                        Ok(out) => text.push_str(&String::from_utf8_lossy(out.as_ref())),
                        Err(e) => return Err(DockerError::Refused(crate::api::daemon_words(&e))),
                    }
                }
                Ok(text)
            };

            match tokio::time::timeout(CAPTURE_DEADLINE, collect).await {
                Ok(text) => text,
                // A deadline rather than a hang: this runs on a command worker, and a container
                // whose `ls` never returns would otherwise hold it for ever.
                Err(_) => Err(DockerError::Unreachable(format!(
                    "`{}` in this container did not finish in {} seconds and was given up on",
                    command.join(" "),
                    CAPTURE_DEADLINE.as_secs()
                ))),
            }
        })
    }

    /// Which shell to run, asking the container rather than guessing.
    pub fn best_shell(&self, container: &str) -> Result<String, DockerError> {
        let printed = self.exec_capture(container, &shell_probe())?;
        shell_from_probe(&printed).ok_or_else(no_shell)
    }

    /// Open an interactive exec and hand back a live session.
    ///
    /// `command` empty means "the best shell this container has" — see [`SHELL_LADDER`].
    pub fn exec_session(
        self: &Arc<Self>,
        container: &str,
        command: &[String],
        geometry: Geometry,
    ) -> Result<Arc<PtySession>, DockerError> {
        let command = if command.is_empty() {
            vec![self.best_shell(container)?]
        } else {
            command.to_vec()
        };

        let (client, runtime) = self.parts();
        let (exec_id, output, input) = runtime.block_on(async {
            let created = client
                .create_exec(
                    container,
                    bollard::models::ExecConfig {
                        attach_stdin: Some(true),
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        // Always. The pane is an xterm; an exec without a TTY gives a shell with
                        // no prompt, no line editing and no job control.
                        tty: Some(true),
                        // The daemon sizes the exec's terminal at creation. Without this the
                        // shell starts at 80x24 and only corrects on the first resize — which for
                        // a pane that is never resized is never.
                        console_size: Some(vec![geometry.rows as usize, geometry.cols as usize]),
                        cmd: Some(command.clone()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;

            let started = client
                .start_exec(
                    &created.id,
                    Some(bollard::exec::StartExecOptions {
                        detach: false,
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;

            match started {
                bollard::exec::StartExecResults::Attached { output, input } => {
                    Ok((created.id, output, input))
                }
                bollard::exec::StartExecResults::Detached => Err(DockerError::Refused(
                    "the daemon detached an exec cide asked to attach to".to_string(),
                )),
            }
        })?;

        // Output: the daemon's stream into the channel `cide-pty` reads.
        let (out_tx, reader) = pipe();
        runtime.spawn(async move {
            let mut output = output;
            while let Some(chunk) = output.next().await {
                let Ok(chunk) = chunk else { break };
                if !send_chunk(&out_tx, chunk.as_ref().to_vec()) {
                    break;
                }
            }
            // Dropping `out_tx` here is what gives the reader EOF, which is what ends the
            // session. Explicit rather than incidental, because it is the whole exit path.
            drop(out_tx);
        });

        // Input: keystrokes out of the writer thread and into the hijacked connection.
        let (in_tx, in_rx) = crossbeam_channel::unbounded::<Vec<u8>>();
        runtime.spawn(async move {
            use tokio::io::AsyncWriteExt as _;
            let mut input = input;
            // A blocking `recv` on the async runtime would park a worker thread. `spawn_blocking`
            // for the wait, and the write back on this task, is the shape that does not.
            loop {
                let rx = in_rx.clone();
                let Ok(Ok(bytes)) = tokio::task::spawn_blocking(move || rx.recv()).await else {
                    break;
                };
                if input.write_all(&bytes).await.is_err() || input.flush().await.is_err() {
                    break;
                }
            }
        });

        let transport = Arc::new(ExecTransport {
            docker: Arc::clone(self),
            exec_id: exec_id.clone(),
            input: parking_lot::Mutex::new(Some(in_tx.clone())),
        });

        let docker = Arc::clone(self);
        let wait_id = exec_id;
        Ok(PtySession::connect(
            SpawnSpec::new("docker-exec", std::path::PathBuf::from("/")).geometry(geometry),
            Box::new(reader),
            Box::new(ChannelWriter::new(in_tx)),
            transport,
            // No pid. The process is inside a container and its pid there means nothing to this
            // machine — the IDE-MCP join key, the jobs watcher and `arm`'s death signal are all
            // about processes cide forked, and this is not one.
            None,
            Box::new(move || docker.wait_exec(&wait_id)),
        ))
    }

    /// Follow a container's logs as a read-only session.
    pub fn logs_session(
        self: &Arc<Self>,
        container: &str,
        tail: u32,
        geometry: Geometry,
    ) -> Result<Arc<PtySession>, DockerError> {
        let (client, runtime) = self.parts();
        let (tx, reader) = pipe();
        // How the reaper learns the follow has ended. See the `wait` closure below.
        let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);

        let mut stream = client.logs(
            container,
            Some(
                bollard::query_parameters::LogsOptionsBuilder::default()
                    .follow(true)
                    .stdout(true)
                    .stderr(true)
                    // Timestamps off: `cide_pty::LineRender` and the JSON-log detector both read
                    // the line the container wrote, and a daemon-prefixed RFC3339 stamp in front
                    // of a JSON document stops it being one.
                    .timestamps(false)
                    .tail(&tail.to_string())
                    .build(),
            ),
        );

        runtime.spawn(async move {
            while let Some(chunk) = stream.next().await {
                let Ok(chunk) = chunk else { break };
                // Line endings are the container's own and are passed through untouched. A log
                // stream from a non-TTY container carries bare `\n`, which an xterm renders as a
                // line feed with no carriage return — so the pane is opened in a mode that
                // converts, rather than the bytes being rewritten here. Rewriting them would
                // corrupt a raw-mode TUI's output the moment somebody logged one.
                if !send_chunk(&tx, chunk.as_ref().to_vec()) {
                    break;
                }
            }
            drop(tx);
            // The follow is over — the container stopped, or the connection dropped. Tell the
            // reaper, which is blocked on the other end.
            let _ = done_tx.send(());
        });

        Ok(PtySession::connect(
            SpawnSpec::new("docker-logs", std::path::PathBuf::from("/")).geometry(geometry),
            Box::new(reader),
            // No input direction. See `stream::DiscardWriter` for why this discards rather than
            // errors.
            Box::new(DiscardWriter),
            Arc::new(LogsTransport),
            None,
            // A follow ends when the stream does, which is when the container stops or the
            // connection drops.
            //
            // # This must BLOCK, and the first version did not
            //
            // `PtySession::connect` runs this on the reaper thread and sets `exited` the moment
            // it returns. A closure that answered immediately therefore marked the session dead
            // on the frame it opened: `sessionSink` wrote `— exited —` into a pane whose bytes
            // were still arriving, which reads exactly like a log that was fetched once instead
            // of followed. That was reported as "logs open without follow" and the `follow=1` in
            // the request had been right all along.
            Box::new(move || {
                // `recv` fails only when the pump task is gone without signalling, which is the
                // same end state — so either way this returns once, when the stream is over.
                let _ = done_rx.recv();
                Exit {
                    code: 0,
                    signal: None,
                }
            }),
        ))
    }

    /// Tell the daemon an exec's terminal changed size.
    fn resize_exec(&self, exec_id: &str, geometry: Geometry) -> Result<(), DockerError> {
        let (client, runtime) = self.parts();
        runtime.block_on(async {
            client
                .resize_exec(
                    exec_id,
                    bollard::query_parameters::ResizeExecOptionsBuilder::default()
                        .h(geometry.rows as i32)
                        .w(geometry.cols as i32)
                        .build(),
                )
                .await
                .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))
        })
    }

    /// Block until an exec finishes, and report how.
    ///
    /// # Why this polls
    ///
    /// The Engine API has no "wait for an exec". `GET /exec/{id}/json` reports `Running` and an
    /// `ExitCode`, and that is the whole of what is available — so this asks, on a slow tick, on
    /// a thread that exists to do exactly this and nothing else. The interval is a compromise
    /// nobody notices: the pane's `— exited —` marker is driven by the *stream's* EOF, which is
    /// immediate; this only decides what the exit code says.
    fn wait_exec(&self, exec_id: &str) -> Exit {
        const TICK: std::time::Duration = std::time::Duration::from_millis(400);
        let (client, runtime) = self.parts();
        loop {
            std::thread::sleep(TICK);
            let inspected = runtime.block_on(client.inspect_exec(exec_id));
            match inspected {
                Ok(state) => {
                    if state.running == Some(true) {
                        continue;
                    }
                    return Exit {
                        code: state.exit_code.unwrap_or(0) as i32,
                        signal: None,
                    };
                }
                // The daemon went away, or the exec was garbage-collected. Reporting an unknown
                // status is honest; looping for ever would leak this thread for the life of the
                // app, one per exec pane anybody ever opened.
                Err(_) => {
                    return Exit {
                        code: cide_pty::UNKNOWN_EXIT_CODE,
                        signal: None,
                    };
                }
            }
        }
    }
}

/// The transport for one interactive exec.
struct ExecTransport {
    docker: Arc<Docker>,
    exec_id: String,
    /// Dropped by [`Transport::kill`], which is what ends the exec — see its note.
    input: parking_lot::Mutex<Option<crossbeam_channel::Sender<Vec<u8>>>>,
}

impl Transport for ExecTransport {
    fn resize(&self, geometry: Geometry) -> Result<(), cide_pty::PtyError> {
        self.docker
            .resize_exec(&self.exec_id, geometry)
            .map_err(|e| cide_pty::PtyError::Resize(Box::new(e)))
    }

    /// # There is no "kill an exec" in the Engine API, and this is what that means
    ///
    /// Docker exposes create, start, inspect and resize for an exec, and nothing that stops one.
    /// The process inside the container ends the way any process on the far end of a terminal
    /// does: the connection goes away. So this drops the input sender, the pump task's `recv`
    /// fails, the hijacked connection is dropped, and the daemon delivers EOF on stdin — after
    /// which a shell exits.
    ///
    /// The honest limitation, stated because it differs from every other pane in cide: a program
    /// that ignores stdin does **not** exit. Closing a pane running `sleep 600` in a container
    /// leaves it running until it finishes; a pty pane would have sent SIGHUP. The alternative is
    /// a *second* exec running `kill`, which needs a shell and a pid visible inside the
    /// container, and fails on exactly the distroless images where it would matter most.
    fn kill(&self) {
        drop(self.input.lock().take());
    }
}

/// The transport for a log follow, which can do none of the three things a transport does.
struct LogsTransport;

impl Transport for LogsTransport {
    /// A log stream has no terminal on the far end, so there is no size to report. `Ok` rather
    /// than an error: `PtySession::resize` still resizes the vt100 mirror, which is what makes
    /// the pane reflow, and a transport error there would surface as a failure to the user for a
    /// window resize that worked.
    fn resize(&self, _geometry: Geometry) -> Result<(), cide_pty::PtyError> {
        Ok(())
    }

    /// Nothing to kill: closing the pane drops the sink, and the stream is ended by the session
    /// being dropped. A follow holds no resource in the container.
    fn kill(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_asks_about_every_shell_in_the_ladder_in_order() {
        let probe = shell_probe();
        assert_eq!(probe[0], "/bin/sh");
        assert_eq!(probe[1], "-c");
        let script = &probe[2];
        let mut at = 0;
        for shell in SHELL_LADDER {
            let found = script[at..]
                .find(shell)
                .unwrap_or_else(|| panic!("{shell} is asked about: {script}"));
            at += found;
        }
        assert!(
            script.contains("-x"),
            "executability, not mere presence: {script}"
        );
    }

    #[test]
    fn the_answer_is_the_last_matching_line_and_never_the_whole_output() {
        // A shell's own startup noise reaches the same capture, and `||` means at most one
        // candidate is echoed — so anything else on the stream is noise.
        assert_eq!(
            shell_from_probe("/bin/bash\n").as_deref(),
            Some("/bin/bash")
        );
        assert_eq!(
            shell_from_probe("some warning on stderr\n/bin/ash\n").as_deref(),
            Some("/bin/ash"),
            "noise before the answer is skipped"
        );
        assert_eq!(
            shell_from_probe("/bin/ash\nmesg: ttyname failed\n").as_deref(),
            Some("/bin/ash"),
            "and noise after it"
        );
    }

    #[test]
    fn an_image_with_no_shell_is_a_sentence_and_not_an_empty_answer() {
        assert_eq!(shell_from_probe(""), None);
        assert_eq!(shell_from_probe("sh: not found"), None);
        // Never a path the ladder does not name — a container echoing something arbitrary must
        // not become a command cide runs.
        assert_eq!(shell_from_probe("/usr/local/bin/evil"), None);

        let sentence = no_shell().to_string();
        assert!(sentence.contains("distroless"), "{sentence}");
        assert!(
            sentence.contains("/bin/bash"),
            "the ladder is named: {sentence}"
        );
    }
}
