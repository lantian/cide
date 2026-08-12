//! Multi-root file indexing and watching: a gitignore-aware parallel walk, a flattened
//! windowed row model, and a debounced watcher filtered by the same matchers. (M8)
//!
//! # The three things this crate refuses to do
//!
//! * **Ship a tree.** [`Index::rows`] answers with a window. A project with 100k files has
//!   100k rows only if the user expands everything, and even then the frontend asks for the
//!   thirty it can draw. See [`index`] for how the row addressing works.
//! * **Block the picker.** [`Index::build`] streams every batch of walked entries to a sink
//!   while the walk runs, so Ctrl+P has candidates within milliseconds of a project opening.
//! * **Storm the UI.** The watcher filters events through [`filter::Filter`] — the same
//!   ignore rules the walk used — and then coalesces what survives into one notification per
//!   burst. A `cargo build` is one repaint, not fifty thousand.
//!
//! Nothing here links a webview, and nothing here knows what a pane is. The app wires an
//! [`Index`], a [`filter::Filter`] and a [`watch::Watcher`] together per project.

pub mod copy;
pub mod error;
pub mod filter;
pub mod index;
pub mod ops;
#[doc(hidden)]
pub mod testing;
pub mod trash;
pub mod watch;

pub use error::{FsError, Result};
pub use filter::Filter;
pub use index::{BuildOptions, Index, Root, WalkItem};
pub use watch::{Coalescer, WatchConfig, WatchEvent, Watcher};
