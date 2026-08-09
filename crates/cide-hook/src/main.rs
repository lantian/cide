//! `cide-hook <event>` — bridges a Claude Code hook invocation to the running IDE.
//!
//! Reads the hook JSON payload on stdin and writes one framed line to the unix socket
//! named by `CIDE_HOOK_SOCK`. Exits 0 even when the socket is gone: a hook that fails is
//! a hook that blocks or annoys the user's session, and a missing IDE is a normal state
//! (the app quit while a `claude` child was still winding down).
//!
//! `cide-hook statusline` is special — it additionally chains the user's own configured
//! statusLine command and prints that command's stdout verbatim, so adopting cide never
//! costs someone the status line they already had.
//!
//! M0 stub: argument shape and the exit-code contract only. The socket write lands in M7.

use std::io::Read;

fn main() {
    let event = std::env::args().nth(1).unwrap_or_default();

    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);

    // Never fail loudly. A hook is on the critical path of the user's Claude session.
    if event.is_empty() {
        std::process::exit(0);
    }

    // M7: connect to $CIDE_HOOK_SOCK and forward {event, payload}; for `statusline`,
    // exec the user's chained command first and pass its stdout through.
    std::process::exit(0);
}
