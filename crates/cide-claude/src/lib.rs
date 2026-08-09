//! Spawning and supervising `claude` children: environment hygiene, hooks, statusline,
//! resume/fork, and the headless one-shot lane.
//!
//! # What lives here and why it is not in `cide-app`
//!
//! None of this needs a webview. The hook vocabulary, the `--settings` payload and the
//! session state machine are decisions about how cide talks to the CLI, and keeping them in
//! a crate that does not link Tauri means they can be tested without one — and that
//! `cide-headless` can drive a real session with the same rules the GUI uses.
//!
//! # The one rule that outranks the rest
//!
//! Never read `~/.claude/.credentials.json`, and never inject `ANTHROPIC_API_KEY`. It
//! outranks subscription OAuth in the CLI's credential precedence, so setting one would
//! silently bill a Console organisation for a user on a Claude Max plan. Children inherit
//! their authentication by inheriting the environment, which is all they need.

pub mod hook;
pub mod settings;
pub mod state;

pub use hook::{HookEvent, HookFrame};
pub use settings::{StatusLine, inline_settings};
pub use state::{is_permission_request, next_state};
