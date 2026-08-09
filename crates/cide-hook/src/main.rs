//! `cide-hook <event>` — bridges a Claude Code hook invocation to the running IDE.
//!
//! Reads the hook JSON payload on stdin and writes one newline-delimited frame to the unix
//! socket named by `CIDE_HOOK_SOCK`.
//!
//! # This program must never make a session worse
//!
//! It runs on the critical path of the user's Claude Code turn — before every tool call,
//! after every one, and around the statusline on a timer. Every failure mode here is
//! therefore an exit 0: a missing socket, a full socket, a refused connection, an unset
//! variable. A hook that errors is a hook that puts noise in someone's session or, worse,
//! blocks it. The IDE being gone is a *normal* state, not an error — the app can quit while
//! a `claude` child is still winding down.
//!
//! The one thing it must do faithfully is the statusline: `cide-hook statusline` runs the
//! user's own configured command and prints its stdout verbatim, so adopting cide never
//! costs someone the status line they already had. If that command fails, we print nothing
//! rather than printing an error into their status bar.
//!
//! It is a separate binary rather than a subcommand of the app so that spawning a hook costs
//! a ~1 MB process rather than a webview.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long to wait on the socket before giving up.
///
/// Short on purpose. The IDE is a local process on the other end of a unix socket; if it has
/// not accepted within this, it is wedged or gone, and continuing to wait would stall the
/// user's turn for a status update nobody is watching.
const SOCKET_TIMEOUT: Duration = Duration::from_millis(250);

fn main() {
    let event = std::env::args().nth(1).unwrap_or_default();
    if event.is_empty() {
        std::process::exit(0);
    }

    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);

    if event == "statusline" {
        // The user's own command, if they had one, is everything after the subcommand.
        let chained: Vec<String> = std::env::args().skip(2).collect();
        if !chained.is_empty() {
            passthrough(&chained, &payload);
        }
    }

    forward(&event, &payload);
    std::process::exit(0);
}

/// Run the user's statusline command and print its stdout verbatim.
///
/// The hook payload is piped to it on stdin, because that is how the CLI would have invoked
/// it — a chained command must not be able to tell it is being chained.
fn passthrough(argv: &[String], payload: &str) {
    // Through a shell, because the user configured a command line and may well have written
    // a pipeline or a `~` in it. This is the user's own string from their own settings, so it
    // is exactly as trusted as it was before cide was involved.
    let joined = argv.join(" ");
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(&joined)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    let Ok(mut child) = spawned else { return };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
        // Dropped explicitly: a statusline command that reads to EOF would otherwise hang,
        // and it would hang inside the user's session.
        drop(stdin);
    }

    let Ok(out) = child.wait_with_output() else {
        return;
    };
    // Verbatim, including the absence of a trailing newline. Reformatting someone's status
    // line is not ours to do.
    let _ = std::io::stdout().write_all(&out.stdout);
}

/// Write one frame to the IDE, if it is listening.
fn forward(event: &str, payload: &str) {
    let Some(path) = std::env::var_os("CIDE_HOOK_SOCK") else {
        return;
    };

    let Ok(mut stream) = UnixStream::connect(&path) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(SOCKET_TIMEOUT));

    // The payload is embedded as a JSON value when it parses, and as a string when it does
    // not. Refusing to forward an unparseable payload would lose the event entirely; the
    // reader can tell the difference and is better placed to decide what to do about it.
    let parsed: serde_json::Value =
        serde_json::from_str(payload).unwrap_or_else(|_| serde_json::Value::String(payload.into()));

    let frame = serde_json::json!({ "event": event, "payload": parsed });
    let Ok(mut line) = serde_json::to_vec(&frame) else {
        return;
    };
    line.push(b'\n');

    let _ = stream.write_all(&line);
    let _ = stream.flush();
}
