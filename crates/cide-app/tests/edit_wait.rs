//! `cide --wait <file>` against a socket the test owns.
//!
//! # Why this is an integration test and not a unit one
//!
//! The unit tests beside `edit_wait.rs` hold the half that is a decision — which project a file
//! lands in, and what "still open" means. They cannot reach the half that is *a process*: that
//! `cide --wait` parses that argv at all, that it resolves the path against **its own** cwd, that
//! it does not exit while the answer is outstanding, and that the exit code says what happened.
//!
//! Every one of those is load-bearing in a way nothing else can catch. The CLI's contract is
//! `spawnSync` plus a read of the file afterwards, so a client that exited early would have Claude
//! Code read back a file the human had not finished editing and report success — silently, and
//! only for people who use Ctrl+G. Driving the real binary is the only way to assert on it.
//!
//! The socket here is the test's, not a running cide's: what is being pinned is the client's end
//! of the protocol, and a test that needed a live application would need a display.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// A directory that removes itself. `cide-tasks`'s tests carry the same twelve lines rather than
/// the workspace taking a dependency for them.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("cide-edit-wait-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        // Canonical, because the client reports `current_dir()` and resolves the file it was
        // given, and both come back with symlinks resolved. `/tmp` is one on more systems than
        // it is not.
        Self(std::fs::canonicalize(&path).expect("canonical temp dir"))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Spawn `cide --wait` with a controlled environment, from `cwd`.
fn spawn_wait(socket: Option<&Path>, session: Option<&str>, cwd: &Path, file: &str) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cide"));
    command
        .arg("--wait")
        .arg(file)
        .current_dir(cwd)
        .stderr(Stdio::piped())
        .env_remove("CIDE_EDIT_SOCK")
        .env_remove("CIDE_SESSION");
    if let Some(socket) = socket {
        command.env("CIDE_EDIT_SOCK", socket);
    }
    if let Some(session) = session {
        command.env("CIDE_SESSION", session);
    }
    command.spawn().expect("the cide binary is built")
}

/// The whole contract, in one exchange: it asks, it waits, and it exits 0 on the answer.
#[test]
fn the_client_asks_and_then_blocks_until_the_answer_arrives() {
    let dir = TempDir::new("blocks");
    let socket = dir.join("s.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    std::fs::write(dir.join("plan.md"), "# plan\n").expect("write");

    let session = "11111111-2222-3333-4444-555555555555";
    // **A relative path**, deliberately: the app's cwd is somewhere else entirely, so resolving
    // it is the client's job and getting it wrong is a file nobody can open.
    let mut child = spawn_wait(Some(&socket), Some(session), &dir.0, "plan.md");

    let (stream, _) = listener.accept().expect("accept");
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .expect("request");
    let request: serde_json::Value = serde_json::from_str(line.trim()).expect("json");

    assert_eq!(request["v"], 1);
    assert_eq!(
        request["open"],
        serde_json::Value::from(dir.join("plan.md").to_str().unwrap()),
        "the path must arrive absolute, resolved against the client's own cwd"
    );
    assert_eq!(
        request["session"],
        serde_json::Value::from(session),
        "identity comes from the client's own environment, never from an argument"
    );
    assert_eq!(
        request["cwd"],
        serde_json::Value::from(dir.0.to_str().unwrap()),
        "a shell pane has no session, so its cwd is the only thing that can place the file"
    );

    // The assertion the whole feature rests on. An editor that has already exited is one Claude
    // Code would read the file back from, mid-edit, and call a success.
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the client exited while the file was still open"
    );

    (&stream)
        .write_all(b"{\"v\":1,\"done\":true}\n")
        .expect("reply");

    let status = child.wait().expect("wait");
    assert_eq!(
        status.code(),
        Some(0),
        "exit 0 is the only status on which the CLI reads the file back"
    );
}

/// An error from the app is a non-zero exit and a sentence, never a silent success.
#[test]
fn an_error_from_cide_is_reported_rather_than_swallowed() {
    let dir = TempDir::new("error");
    let socket = dir.join("s.sock");
    let listener = UnixListener::bind(&socket).expect("bind");

    let child = spawn_wait(Some(&socket), None, &dir.0, "plan.md");
    let (stream, _) = listener.accept().expect("accept");
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .expect("request");
    (&stream)
        .write_all(b"{\"v\":1,\"error\":\"cide: no project is open to show this file in\"}\n")
        .expect("reply");

    let out = child.wait_with_output().expect("wait");
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no project is open"),
        "the app's own sentence must reach the user; got {stderr:?}"
    );
}

/// Run outside a cide session, it says so instead of exiting 0 on a file it never opened.
///
/// Reachable only by hand — `child_env::editor_env` sets `EDITOR` and the socket together — so
/// what this pins is that the hand case is legible rather than a mystery success.
#[test]
fn with_no_socket_it_fails_and_names_cide() {
    let dir = TempDir::new("nosock");
    let out = spawn_wait(None, None, &dir.0, "plan.md")
        .wait_with_output()
        .expect("wait");
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("cide:"), "got {stderr:?}");
    assert!(stderr.contains("CIDE_EDIT_SOCK"), "got {stderr:?}");
}

/// cide quitting mid-edit ends the wait as a failure, not as a finished edit.
///
/// The connection dying is the only thing that ends a wait early, and the CLI must not read a
/// half-edited file back off the strength of it.
#[test]
fn a_connection_that_dies_mid_wait_is_not_a_finished_edit() {
    let dir = TempDir::new("hangup");
    let socket = dir.join("s.sock");
    let listener = UnixListener::bind(&socket).expect("bind");

    let child = spawn_wait(Some(&socket), None, &dir.0, "plan.md");
    let (stream, _) = listener.accept().expect("accept");
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .expect("request");
    drop(stream);

    let out = child.wait_with_output().expect("wait");
    assert_ne!(out.status.code(), Some(0));
}
