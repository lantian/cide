//! A Language Server Protocol **client**: spawns `rust-analyzer` and `gopls`, keeps them fed, and
//! turns `textDocument/publishDiagnostics` into `cide_ipc::Diagnostic`.
//!
//! ```text
//! discover  → is it installed, and is there a project for it?   (a sentence, not a code)
//! server    → spawn, three threads, the shutdown ladder, backoff
//! codec     → Content-Length framing, generic over Read/Write
//! session   → a pure state machine: messages in, Effects out
//! convert   → LSP shapes into cide's
//! ```
//!
//! # Explicit threads, not tokio — and the reason is not ergonomics
//!
//! Both precedents exist in this workspace: `cide-pty` uses threads because portable-pty's handles
//! are blocking, `cide-ide-mcp` uses tokio because tokio-tungstenite is the only sane WebSocket
//! transport. This crate takes `cide-pty`'s, for one reason that outranks every ergonomic
//! argument:
//!
//! > `cide_core::child_env::arm` installs `PR_SET_PDEATHSIG` from `pre_exec`, and its contract is
//! > **the calling thread must outlive the child**.
//!
//! A tokio task does not satisfy that. A task can be moved between workers, so the thread that
//! forked can retire while the server is perfectly healthy — at which point the kernel delivers
//! `SIGTERM` to a working rust-analyzer, intermittently, on a work-stealing schedule nobody can
//! reproduce. A dedicated supervisor thread per server satisfies it by construction, the same way
//! `cide_claude::headless::run` does.
//!
//! Secondary, and each true on its own: `Content-Length` framing has nothing to multiplex;
//! `crossbeam-channel` is already a workspace dependency, so the outbound queue and the
//! `recv_timeout` the shutdown ladder needs cost nothing new; and staying tokio-free keeps
//! `cide-headless` able to link this crate.
//!
//! # `lsp-types` is used and never re-exported
//!
//! Everything that crosses this crate's boundary is cide's own [`cide_ipc::Diagnostic`]. That is
//! what contains the 0.x hazard the workspace manifest's comment describes: a future bump is a
//! diff inside these five files rather than a fork of the dependency graph.
//!
//! # Both spawn rules apply here
//!
//! A language server is the second long-lived, memory-hungry child in this application, and it
//! needs both of them:
//!
//! * **ADR 0007** — `child_env::scrub_command`, or an AppImage lends the server the bundle's
//!   `LD_LIBRARY_PATH` and it dies on a symbol lookup three processes below anything cide logs.
//! * **ADR 0008** — `child_env::arm`, or a `SIGKILL` of cide leaves a 1–4 GB indexer running with
//!   nothing left to talk to.

pub mod codec;
pub mod convert;
pub mod discover;
pub mod server;
pub mod session;

pub use discover::{Found, Server};
pub use server::{LspError, LspEvent, LspHandle, RequestError, Requester};
pub use session::{Effect, Session};
