//! IPC command handlers, one module per namespace.
//!
//! Rust function names are snake_case (`session_attach`); `ui/src/ipc/client.ts` presents
//! them to the rest of the frontend as `session.attach`. Handlers stay thin — unwrap
//! arguments, call a domain crate, wrap the result — so that logic stays testable in
//! crates that do not link a webview.

pub mod app;
pub mod diag;
pub mod lifecycle;
pub mod pane;
pub mod project;
pub mod session;
pub mod window;
