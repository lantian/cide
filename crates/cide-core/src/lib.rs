//! The cide domain.
//!
//! The Rust process owns everything durable; the webview owns only transient gesture state
//! and the live DOM instances (an xterm `Terminal`, a CodeMirror `EditorView`). This is
//! forced rather than preferred: a detached pane is a separate JavaScript realm, sessions
//! must outlive every webview, and PTY children are Rust resources that must not drift
//! from the pane tree.
//!
//! # Shape
//!
//! The *data* lives in `cide-ipc` — it is simultaneously the in-memory domain, the on-disk
//! format and the wire format, and duplicating it three ways would only create
//! opportunities for the three to disagree. This crate owns the *behaviour*, as free
//! functions over those types rather than inherent impls (which Rust would not allow
//! across crates anyway, and which reads better for operations that are genuinely pure).
//!
//! Nothing here depends on `tauri`. `cide-headless` links this crate and must never be
//! able to link a webview — that is the standing check that the rule is being kept.

pub mod child_env;
pub mod commands;
pub mod diagnostics;
pub mod document;
pub mod error;
pub mod handshake;
pub mod keymap;
pub mod layout;
pub mod persist;
pub mod proxy;
pub mod workspace;

pub use error::{CoreError, Result};
