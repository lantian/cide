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

//! # Two things that only matter when cide dies badly
//!
//! [`orphans`] holds both: `PR_SET_PDEATHSIG`, so a `SIGKILL` of this process takes its
//! children with it, and the sweep for the hook sockets a previous hard kill left behind.
//! Neither has a clean-shutdown path to live on, which is exactly why they are here.

pub mod headless;
pub mod hook;
pub mod orphans;
pub mod permission;
/// Which option on a plan-approval prompt means *yes, proceed*. (M79)
pub mod plan;
pub mod prompt;
pub mod roster;
pub mod session;
pub mod settings;
pub mod state;
pub mod version;

pub use headless::{Headless, ToolAccess};
pub use hook::{HookEvent, HookFrame};
pub use orphans::{arm, on_spawn_thread, sweep_hook_sockets};
pub use roster::claude_dir;
pub use roster::names as session_names;
pub use session::conversation;
pub use settings::{ClaudeTheme, StatusLine, inline_settings};
pub use state::{is_permission_request, next_state};
pub use version::{Support, check_once};
