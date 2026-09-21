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
pub mod remote;
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

// --- M26: Reformat code ---
// Its own module and not a row in `diagnostics`, because only one of its two roads is a language
// server: the other spawns a user-configured filter and never speaks LSP at all. Filing it under
// the module whose header is about document sync would put a process spawn somewhere nobody
// looking for one would think to check.
pub mod format;

// --- M18: the git tool window ---
// The bottom panel's own state — open, height, which tabs. Separate from `git` because nothing
// here touches a repository: `git` answers about commits and working trees, this answers about a
// strip of chrome, and the only thing they share is the word "git" in the feature's name.
pub mod toolwindow;

// --- M18: the task tracker ---
// `.cide/tasks.json`, the committed file the product-owner session and its subagents exchange
// state through. Its own module for the reason `toolwindow` is separate from `git`: nothing here
// touches a repository, and the only thing it shares with `project` is that both name a project.
pub mod tasks;

// --- M18: subagent orchestration ---
// `.cide/config.json` and `.cide/agents/*.md`: the roles a project defines and the per-project
// switch that says whether it may run them. Its own module rather than rows in `project` for the
// reason `tasks` is separate too — nothing here touches the workspace tree, and the only thing it
// shares with `project` is that both name a project. `cide-agents` holds every rule; these three
// handlers resolve a root, call it, and hand back what it said.
pub mod agents;

// --- M20: cancelling a log walk ---
// The registry of in-flight log walks, keyed by tool tab, and the one command that stops one.
// Deliberately *not* rows in `git`: everything there is stateless by that module's own first
// rule — "There is no `GitState` and nothing is managed" — and this is the one piece of git
// machinery that has to be managed, because a flag nobody holds is a flag nobody can set.
// `git_log` itself stays beside its siblings in `git` and borrows a flag from here.
pub mod log;

// --- M22: extensions and their marketplaces ---
// The one global registry in this crate. Its own module rather than rows in `settings` for the
// reason `tasks` is separate from `project`: nothing here touches the workspace tree or a
// project's settings, and the only thing it shares with either is that both are things a user
// configures. It is also the only command module whose handlers can take seconds — a clone over
// a network — which is why every one of them is `async` and hands its work to `spawn_blocking`
// rather than being a synchronous handler Tauri would poll on the GTK loop.
pub mod ext;

// M28: OpenSpec. Its own module rather than rows in `cmd::tasks`, because everything here spawns
// a subprocess and answers with somebody else's file format — the two facts that keep `cide-spec`
// a separate crate apply one layer up as well.
pub mod docker;
pub mod spec;
