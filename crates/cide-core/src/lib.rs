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

pub mod check;
pub mod child_env;
pub mod claude_cli;
pub mod commands;
pub mod diagnostics;
pub mod document;
pub mod error;
pub mod format;
pub mod handshake;
/// Images: what a file is, from its own bytes. The sibling of [`document`], never its pixels.
pub mod image;
pub mod jsonlog;
pub mod keymap;
pub mod layout;
pub mod login_path;
/// Node package managers' global-binary directories: where `npm -g` puts a thing on a machine
/// whose shell rc, not its desktop launcher, put that directory on `PATH`.
pub mod node_dirs;
pub mod notes;
pub mod persist;
/// Process ancestry: whose child a pid is. The join key when a `claude` is not the process
/// cide forked — a wrapper, a shell, a re-exec through a proxy.
pub mod proc;
pub mod profile;
/// What a path *is* — OS stat, and what one pass over a text file's bytes says. (M70)
pub mod properties;
pub mod proxy;
pub mod remote;
/// Editor colour schemes: importing a VS Code theme, and the imported ones on disk.
pub mod scheme;
pub mod scratch;
pub mod shell;
pub mod toolchain;
pub mod toolwindow;
pub mod workspace;

pub use error::{CoreError, Result};
