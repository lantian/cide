//! IPC command handlers, one module per namespace.
//!
//! Rust function names are snake_case (`session_attach`); `ui/src/ipc/client.ts` presents
//! them to the rest of the frontend as `session.attach`. Handlers stay thin — unwrap
//! arguments, call a domain crate, wrap the result — so that logic stays testable in
//! crates that do not link a webview.

pub mod app;
pub mod diag;
pub mod file;
pub mod git;
pub mod lifecycle;
pub mod pane;
pub mod project;
pub mod session;
pub mod settings;
pub mod window;

// --- M8 ---
pub mod fs;
pub mod picker;

// --- M11 ---
// Content search. Separate from `picker` on purpose: that one ranks paths, this one greps
// file contents, and the two share no state and no scoring.
pub mod search;

// --- M12: language support ---
// Two modules, because they answer to different producers: `symbols` is tree-sitter's structure
// (parsed in-process, on demand), `diagnostics` is the merged view over the language servers,
// tree-sitter and a Claude one-shot. A file has both, and that is not a reason to share a module.
pub mod diagnostics;
pub mod symbols;
