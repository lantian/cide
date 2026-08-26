//! Routes `.cide/` filesystem changes to the Rust state that mirrors those files.
//!
//! `cide_fs::filter` has always watched `<root>/.cide`, and until this module existed the events
//! went to the webview (`cide://fs-changed`) and stopped there: nothing in Rust consumed them, so
//! a role file written by an agent's ordinary `Write` tool — the *documented* way to create a
//! role, since the MCP vocabulary deliberately has no `cide_agent_create` — was invisible to the
//! Agents panel until a restart, and a `git pull` that moved `.cide/tasks.json` sat unmerged
//! until this process happened to have a local write pending. Both module headers claimed the
//! watcher closed this loop (`cmd::agents`' "the panel simply asks again"); this is the module
//! that makes the claim true.
//!
//! Two routes, both deliberately thin:
//!
//! * **`tasks.json`** → [`cide_tasks::TaskStore::refresh_from_disk`], which is the watcher's
//!   primitive and not `reload` — `reload` is the panel's Retry and merges unconditionally,
//!   which on this path would merge on every echo of our own debounced flush. The stamp compare
//!   inside `refresh_from_disk` is the echo suppression. The 250 ms flusher in
//!   [`crate::tasks_state`] stays as the belt to this suspender: it still catches a change made
//!   while the watcher is degraded or an event that coalesced away.
//! * **`agents/`, `config.json`** → [`crate::agents::AgentRegistry::mark_changed`]. That is the
//!   entire fix: the registry's coalescer (120 ms, 1 s ceiling) rebuilds the roster from disk on
//!   flush, so routing through it keeps `cide://agents-changed`'s one-funnel emit and the
//!   argument in `emit.rs` for why that event carries no `rev`.
//!
//! # The worktrees filter is the load-bearing line
//!
//! Agent worktrees live at `.cide/worktrees/<agent>` — *under* the watched `.cide` prefix
//! (`Filter::is_watched_path` is a `starts_with`) — and a worktree is a checkout of the whole
//! repository, so it contains its own `.cide/tasks.json`. Without the filter, a subagent running
//! `cargo build` floods this router with thousands of paths per second, and the inner tracker of
//! its checkout would be mistaken for the project's. Only the **first** `.cide` component
//! decides, and `worktrees` after it ends the question for the whole path.
//!
//! This runs on the watcher thread (see [`crate::files::on_watch_event`]'s note about stalling
//! later bursts), so classification is component scans with no allocation, no locks and no I/O;
//! the disk work happens on a blocking worker.
//!
//! The **global** agents directory (`cide_agents::defs::global_dir()`) is outside every project
//! watcher and deliberately not covered: every read path re-reads it (`load_project` is
//! uncached), and `agents_save` — the in-app editor for those files — already emits. The one
//! unmet case is a hand-edited global role file while the app is completely idle; if that ever
//! matters, the answer is a `notify` watcher on the global dir calling `mark_changed` for every
//! open project, not a special case here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cide_ipc::ProjectId;
use tauri::{AppHandle, Manager as _};

use crate::agents::AgentRegistry;
use crate::tasks_state::TasksStores;

/// What one path under `.cide/` means to this process.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// The project's task tracker moved.
    Tasks,
    /// The roster's inputs moved — a role file or the config that gates them.
    Agents,
    /// Nothing this module mirrors: another project file, or anything inside a worktree.
    Nothing,
}

/// Decide from the **first** `.cide` component only — see the module header for why a second
/// one (inside a worktree's checkout) must never be consulted.
fn classify(path: &Path) -> Route {
    let mut components = path.components().map(|c| c.as_os_str());
    while let Some(part) = components.next() {
        if part != ".cide" {
            continue;
        }
        return match components.next().map(|next| (next, components.next())) {
            // The storm filter: an agent's build churning its checkout.
            Some((next, _)) if next == "worktrees" => Route::Nothing,
            // Only the file itself, not e.g. an editor's `tasks.json.swp` beside it.
            Some((next, None)) if next == "tasks.json" => Route::Tasks,
            Some((next, None)) if next == "config.json" => Route::Agents,
            Some((next, _)) if next == "agents" => Route::Agents,
            _ => Route::Nothing,
        };
    }
    Route::Nothing
}

/// The one entry point, called from [`crate::files::FsEvents`]' `AppHandle` implementation for
/// every batch of changed paths.
pub fn files_changed(app: &AppHandle, project: ProjectId, paths: &[PathBuf]) {
    let mut tasks = false;
    let mut agents = false;
    for path in paths {
        match classify(path) {
            Route::Tasks => tasks = true,
            Route::Agents => agents = true,
            Route::Nothing => {}
        }
        if tasks && agents {
            break;
        }
    }

    if tasks {
        // `get`, never `ensure`: a project whose tracker was never opened reads fresh on its
        // first `tasks_board` anyway, and opening stores from fs events would do disk work for
        // a panel nobody has.
        let store = app
            .try_state::<Arc<TasksStores>>()
            .and_then(|stores| stores.get(project));
        if let Some(store) = store {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if store.refresh_from_disk().is_some() {
                    tracing::info!(%project, "the task tracker changed on disk; broadcasting the merged board");
                    crate::tasks_state::broadcast(&app, project, &store);
                }
            });
        }
    }

    if agents && let Some(registry) = app.try_state::<Arc<AgentRegistry>>() {
        registry.mark_changed(app, project);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(path: &str) -> Route {
        classify(Path::new(path))
    }

    #[test]
    fn the_router_reads_only_the_first_dot_cide_component() {
        assert_eq!(route("/repo/.cide/tasks.json"), Route::Tasks);
        assert_eq!(route("/repo/.cide/config.json"), Route::Agents);
        assert_eq!(route("/repo/.cide/agents/developer.md"), Route::Agents);
        // A deleted role arrives as the directory itself.
        assert_eq!(route("/repo/.cide/agents"), Route::Agents);

        // The storm filter, and the checkout-inside-a-worktree trap it also closes: the inner
        // `.cide/tasks.json` belongs to the agent's checkout, not to the project.
        assert_eq!(
            route("/repo/.cide/worktrees/dev/src/main.rs"),
            Route::Nothing
        );
        assert_eq!(
            route("/repo/.cide/worktrees/dev/.cide/tasks.json"),
            Route::Nothing
        );

        // Neighbours that must not fire: an editor's swap beside the tracker, a stray file at
        // the top of `.cide/`, and a project file that merely contains the substring.
        assert_eq!(route("/repo/.cide/tasks.json.swp"), Route::Nothing);
        assert_eq!(route("/repo/.cide/notes.md"), Route::Nothing);
        assert_eq!(route("/repo/src/dot.cide/tasks.json"), Route::Nothing);
        assert_eq!(route("/repo/src/main.rs"), Route::Nothing);
    }
}
