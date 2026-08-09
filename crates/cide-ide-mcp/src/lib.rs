//! The Claude Code IDE-integration server.
//!
//! This is what turns cide from a terminal with Claude in it into an IDE. A `claude` child
//! that connects here stops printing diffs as ASCII in its own pane and starts asking the
//! editor to show them; a selection in the editor becomes context in the prompt; an
//! `@`-mention is a gesture rather than a path typed by hand.
//!
//! # Shape
//!
//! One server per **project**, listening on `127.0.0.1:0`. Every Claude pane in that project
//! is spawned with `CLAUDE_CODE_SSE_PORT=<our port>`, which both satisfies the CLI's
//! lockfile validity check and forces auto-connect — so panes bind to *our* server
//! deterministically instead of racing whatever else wrote a lockfile into `~/.claude/ide/`.
//!
//! Connections are attributed to panes by pid: the CLI sends `ide_connected{pid}` and
//! `cide-pty` already knows each child's pid. One connection, one pid, one pane.
//!
//! # The part that is genuinely hard
//!
//! `openDiff` **blocks the agent's turn**. The CLI will not proceed until a human answers,
//! so every way that human can vanish has to resolve the pending request: the pane closed,
//! the tab closed, the window closed, the project closed, the app quitting, the socket
//! dropping, the same file already under diff from another pane, or the user simply walking
//! away. Miss one and a session hangs for ever with no visible cause. That is what
//! [`diff_broker`] exists for, and why it is a module rather than a `HashMap`.
//!
//! # Caveat carried into every file here
//!
//! The protocol is undocumented and unversioned; see [`protocol`] for provenance. The rule
//! is that a protocol change degrades the diff view and never breaks the terminal.

//! # The surface, in one place
//!
//! Written here rather than left implicit in `server.rs` because more than one thing is
//! built against it — the app wires the events, and the integration test drives the server
//! without the app. A reader of either should not have to infer the API from the
//! implementation, and a second author should not have to guess it.
//!
//! ```ignore
//! let server = IdeServer::start(vec![project_root]).await?;
//! let port    = server.port();          // put in CLAUDE_CODE_SSE_PORT for every child
//! let broker  = server.broker();        // resolve(id, outcome) answers a blocked openDiff
//! let mut rx  = server.events();        // ServerEvent stream
//!
//! server.bind_pane(pid, pane_id);       // the app knows which PtySession has that pid
//! server.selection_changed(pane, payload);
//! server.at_mentioned(pane, payload);
//! server.shutdown().await;
//! ```
//!
//! `ServerEvent` is the whole outbound vocabulary: `Connected { connection, pid }`,
//! `Disconnected { connection }`, `DiffRequested(DiffRequest)`,
//! `DiffWithdrawn { tab_name }`, and `OpenFile { path, start_line, end_line }`.

pub mod diff_broker;
pub mod lockfile;
pub mod protocol;
pub mod server;
pub mod tools;

pub use diff_broker::{CancelReason, DiffBroker, DiffRequest};
pub use lockfile::{Lockfile, sweep_stale};
pub use protocol::{DiffOutcome, OpenDiffParams, SelectionChanged};
pub use server::{IdeServer, ServerEvent};

#[derive(Debug, thiserror::Error)]
pub enum IdeError {
    #[error("could not bind a loopback port: {0}")]
    Bind(String),
    #[error("could not write the lockfile: {0}")]
    Lockfile(String),
    #[error("no diff request with id {0}")]
    NoSuchDiff(String),
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for IdeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, IdeError>;
