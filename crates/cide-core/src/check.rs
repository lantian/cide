//! Run a project's own check command and say what it answered. (M83)
//!
//! A milestone's gate and a project's verify command are both *somebody else's* program: a
//! `tools/ci/milestone.sh slice`, a `cargo test`, a `make check`. cide does not interpret them; it
//! runs one with `sh -c` in a directory, waits for it within a limit, and keeps the verdict and the
//! last lines of what it printed — which is where every test runner prints its verdict too.
//!
//! # The rules, each a failure it prevents
//!
//! * **`prepare_command` and `arm`, like every child** (CLAUDE.md). A gate launched from an
//!   AppImage with its `LD_LIBRARY_PATH` would fail for reasons that have nothing to do with the
//!   project, and a check that outlives a crashed cide keeps a CPU busy for nobody.
//! * **Its own process group**, so a timeout ends the whole tree — a test runner's workers, a
//!   headless game — and not just the `sh` in front of it. `sh -c` does not forward a signal to a
//!   foreground job it is waiting on.
//! * **Both streams, drained concurrently.** A runner that fills its stderr pipe while this reads
//!   stdout would block for ever and be reported as a timeout.
//! * **The calling thread waits**, which is `arm`'s contract. Call this from a thread you own.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cide_ipc::CheckResult;

/// How many lines of output a result keeps.
pub const TAIL_LINES: usize = 60;

/// How long one kept line may be, in characters. A progress bar redrawn with `\r` is one line of
/// kilobytes, and a verdict that is mostly one progress bar is not a verdict.
const LINE_CAP: usize = 400;

/// How long a check has after `SIGTERM` before it is killed.
const GRACE: Duration = Duration::from_secs(5);

/// Run `command` in `dir` with `sh -c`, for at most `timeout`. Never fails: a command that could
/// not be started is a failed check whose tail says why.
pub fn run(dir: &Path, command: &str, timeout: Duration) -> CheckResult {
    run_logged(dir, command, timeout, None)
}

/// [`run`], writing **every** line to `log` as it arrives as well as keeping the tail. (M83)
///
/// The tail is what a verdict needs; the log is what a person drilling into a red gate needs,
/// and a gate that ran for twenty minutes printed far more than sixty lines. Written as it
/// happens rather than at the end, so a viewer polling the file watches a running check — and so
/// a check that is killed, or a cide that dies, still leaves what it had printed. The file is
/// truncated at the start: one log per check key, the latest run.
pub fn run_logged(dir: &Path, command: &str, timeout: Duration, log: Option<&Path>) -> CheckResult {
    let started_unix_ms = now_ms();
    let started = Instant::now();
    let head = head_of(dir);

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::child_env::prepare_command(&mut cmd);
    crate::child_env::arm(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let failed = |tail: String| CheckResult {
        command: command.to_string(),
        passed: false,
        exit_code: None,
        timed_out: false,
        tail,
        started_unix_ms,
        duration_ms: elapsed_ms(started),
        head: head.clone(),
    };

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return failed(format!(
                "cide could not start `sh -c {command}` in {}: {error}",
                dir.display()
            ));
        }
    };

    let tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let sink: Arc<Mutex<Option<std::fs::File>>> = Arc::new(Mutex::new(log.and_then(|path| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = std::fs::File::create(path).ok()?;
        let _ = writeln!(
            file,
            "$ {command}\n# in {} at {} (unix ms)\n",
            dir.display(),
            started_unix_ms
        );
        Some(file)
    })));
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|stream| {
        let tail = Arc::clone(&tail);
        let sink = Arc::clone(&sink);
        std::thread::spawn(move || collect(stream, &tail, &sink))
    })
    .collect();

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            stop(&mut child);
            break child.wait().ok();
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    for reader in readers {
        let _ = reader.join();
    }

    let mut lines: Vec<String> = tail
        .lock()
        .map(|t| t.iter().cloned().collect())
        .unwrap_or_default();
    if timed_out {
        lines.push(format!(
            "[cide] stopped after {}s, the limit for this check",
            timeout.as_secs()
        ));
    }
    let exit_code = status.and_then(|s| s.code());
    if let Ok(mut guard) = sink.lock()
        && let Some(file) = guard.as_mut()
    {
        let _ = writeln!(
            file,
            "\n# {}",
            if timed_out {
                format!(
                    "stopped after {}s, the limit for this check",
                    timeout.as_secs()
                )
            } else {
                match exit_code {
                    Some(code) => format!("exit {code}"),
                    None => "killed by a signal".to_string(),
                }
            }
        );
    }
    CheckResult {
        command: command.to_string(),
        passed: !timed_out && exit_code == Some(0),
        exit_code: if timed_out { None } else { exit_code },
        timed_out,
        tail: lines.join("\n"),
        started_unix_ms,
        duration_ms: elapsed_ms(started),
        head,
    }
}

/// Keep the last [`TAIL_LINES`] lines of `stream`. A `\r` starts a line over, which is what a
/// terminal would have shown.
fn collect(
    stream: Box<dyn Read + Send>,
    tail: &Mutex<VecDeque<String>>,
    sink: &Mutex<Option<std::fs::File>>,
) {
    let mut reader = BufReader::new(stream);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&raw);
        let line = text
            .trim_end_matches(['\n', '\r'])
            .rsplit('\r')
            .next()
            .unwrap_or_default();
        let clean = strip_ansi(line);
        if let Ok(mut guard) = sink.lock()
            && let Some(file) = guard.as_mut()
        {
            // The line **as printed** — colour codes and carriage returns included, uncapped —
            // because the log is read in a viewer that renders them (the GitLab job-log parser);
            // the tail is the plain-text verdict and is cleaned. Stripping here would throw away
            // the one thing that makes a test runner's output scannable.
            let _ = writeln!(file, "{}", text.trim_end_matches(['\n', '\r']));
        }
        let line: String = clean.chars().take(LINE_CAP).collect();
        if let Ok(mut tail) = tail.lock() {
            tail.push_back(line);
            while tail.len() > TAIL_LINES {
                tail.pop_front();
            }
        }
    }
}

/// Drop CSI escape sequences, which a runner that thinks it has a terminal prints for colour.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// End the check's whole process group: `SIGTERM`, a grace, then `SIGKILL`.
fn stop(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        crate::child_env::signal_group(child.id(), libc::SIGTERM);
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        crate::child_env::signal_group(child.id(), libc::SIGKILL);
    }
    let _ = child.kill();
}

/// The checked-out commit, when `dir` is a git checkout. Read with `git` itself so a worktree's
/// `.git` file is followed exactly as git follows it.
pub fn head_of(dir: &Path) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    crate::child_env::prepare_command(&mut cmd);
    crate::child_env::arm(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn a_passing_check_keeps_its_last_lines_from_both_streams() {
        let result = run(
            &dir(),
            // The pause is what makes this deterministic: the two streams are read by two
            // threads, so a stderr line written straight after a burst of stdout can be queued
            // *before* that burst and then pushed out of the tail by it. Interleaving within a
            // moment is not something a tail can promise; keeping both streams is.
            "for i in $(seq 1 100); do echo line $i; done; sleep 0.3; echo oops >&2; printf 'a\\rb\\n'",
            Duration::from_secs(20),
        );
        assert!(result.passed, "{result:?}");
        assert_eq!(result.exit_code, Some(0));
        let lines: Vec<&str> = result.tail.lines().collect();
        assert_eq!(lines.len(), TAIL_LINES);
        assert!(lines.contains(&"oops"), "stderr is kept: {lines:?}");
        assert!(
            lines.contains(&"b"),
            "a carriage return starts the line over: {lines:?}"
        );
        assert!(lines.contains(&"line 100"));
        assert!(!lines.contains(&"line 1"), "only the tail");
    }

    #[test]
    fn a_failing_check_says_its_exit_code_and_colour_is_dropped() {
        let result = run(
            &dir(),
            "printf '\\033[31mFAILED\\033[0m\\n'; exit 3",
            Duration::from_secs(20),
        );
        assert!(!result.passed);
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(result.tail, "FAILED");
    }

    #[test]
    fn a_check_past_its_limit_is_stopped_with_everything_it_started() {
        let started = Instant::now();
        // The background `sleep` is the point: it is in the group, and a kill of `sh` alone
        // would leave it holding the pipe open, so the readers — and this test — would hang.
        let result = run(
            &dir(),
            "sleep 30 & echo started; wait",
            Duration::from_millis(500),
        );
        assert!(result.timed_out && !result.passed, "{result:?}");
        assert_eq!(result.exit_code, None);
        assert!(result.tail.contains("started"), "{}", result.tail);
        assert!(
            result.tail.contains("[cide] stopped after"),
            "{}",
            result.tail
        );
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "it did not wait out the sleep"
        );
    }

    #[test]
    fn the_log_keeps_every_line_and_how_it_ended() {
        let log = std::env::temp_dir().join(format!("cide-check-log-{}.log", std::process::id()));
        let result = run_logged(
            &dir(),
            "for i in $(seq 1 100); do echo line $i; done; exit 4",
            Duration::from_secs(20),
            Some(&log),
        );
        assert_eq!(result.exit_code, Some(4));
        let text = std::fs::read_to_string(&log).expect("written");
        assert!(text.starts_with("$ for i in"), "{text}");
        assert!(
            text.contains("line 1\n") && text.contains("line 100\n"),
            "every line, not the tail"
        );
        assert!(text.trim_end().ends_with("# exit 4"), "{text}");
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn a_command_in_a_missing_directory_is_a_failed_check_not_a_panic() {
        let result = run(
            Path::new("/nonexistent/cide/check"),
            "true",
            Duration::from_secs(5),
        );
        assert!(!result.passed);
        assert!(result.tail.contains("could not start"), "{}", result.tail);
    }
}
