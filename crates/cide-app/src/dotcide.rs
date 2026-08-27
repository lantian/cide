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

/// What one path under `.cide/` or `openspec/` means to this process.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// The project's task tracker moved.
    Tasks,
    /// The roster's inputs moved — a role file or the config that gates them.
    Agents,
    /// A spec or a change moved — in the project's own tree, or in one agent's checkout. (M28)
    Spec,
    /// Nothing this module mirrors: another project file, or anything inside a worktree.
    Nothing,
}

/// Decide from the **first** `.cide` component only — see the module header for why a second
/// one (inside a worktree's checkout) must never be consulted.
///
/// # `openspec/` is matched against the project root, and `.cide/` is not (M28)
///
/// The `.cide` rule is a component scan because that name cannot plausibly occur anywhere else
/// in a repository. `openspec` can: it is an ordinary lowercase word with no dot, and
/// `src/openspec/notes.md` in a project that documents its own tooling is a perfectly normal
/// file. So the spec arm is anchored — the directory has to be the project's own — and that
/// needs the root, which is why this takes one.
fn classify(path: &Path, root: &Path) -> Route {
    // Anchored first. A path outside the project cannot be about the project, and this also
    // tightens the `.cide` rule for free: `vendor/x/.cide/tasks.json` used to fire.
    let Ok(relative) = path.strip_prefix(root) else {
        return Route::Nothing;
    };
    let mut components = relative.components().map(|c| c.as_os_str());
    let Some(first) = components.next() else {
        return Route::Nothing;
    };

    // The project's own `openspec/`, at the root and nowhere else.
    if first == cide_spec::SPEC_RELATIVE {
        return Route::Spec;
    }

    // The project's Claude Code subagents. (M30)
    //
    // `.claude/` holds three kinds of thing and only one of them is a roster: `agents/` is read by
    // `cide_agents::defs`, while `projects/` and `settings.json` are the CLI's own business and
    // move constantly under a live session. So the arm is `agents` and nothing else, and every
    // other path under `.claude/` is `Nothing` **by name** rather than by falling through — a
    // fall-through would be one refactor away from routing a transcript write into a roster
    // rebuild, which is a rebuild per assistant token.
    //
    // The **user's** `~/.claude/agents` is not here and is not watched anywhere, exactly as
    // `defs::global_dir()` is not: every read path re-reads it and `agents_save` emits, so the
    // only cost is that another program editing it is not noticed until something else asks.
    if first == ".claude" {
        return match components.next() {
            Some(next) if next == "agents" => Route::Agents,
            _ => Route::Nothing,
        };
    }

    if first != ".cide" {
        return Route::Nothing;
    }

    match components.next().map(|next| (next, components.next())) {
        /*
         * The storm filter, and the one hole deliberately cut in it. (M28)
         *
         * `.cide/worktrees/<agent>/` is a whole checkout: an agent's `cargo build` puts thousands
         * of paths a second through here, and that checkout's own `.cide/tasks.json` is not this
         * project's. So everything under it is `Nothing` — except one directory.
         *
         * While a run is live its checklist is the one in **its** worktree, and the boxes it
         * ticks are what the task card's progress bar draws. Without this arm the bar sits at
         * zero for the whole of a run and jumps only when the branch is integrated, which reads
         * as an agent that did nothing for an hour.
         *
         * The hole is exactly one component wide: the component after the agent's name must be
         * `openspec`, so `target/`, `src/` and the checkout's inner `.cide/` all still die here.
         */
        Some((next, agent)) if next == "worktrees" => match (agent, components.next()) {
            (Some(_), Some(inner)) if inner == cide_spec::SPEC_RELATIVE => Route::Spec,
            _ => Route::Nothing,
        },
        // Only the file itself, not e.g. an editor's `tasks.json.swp` beside it.
        Some((next, None)) if next == "tasks.json" => Route::Tasks,
        Some((next, None)) if next == "config.json" => Route::Agents,
        Some((next, _)) if next == "agents" => Route::Agents,
        _ => Route::Nothing,
    }
}

/// The one entry point, called from [`crate::files::FsEvents`]' `AppHandle` implementation for
/// every batch of changed paths.
pub fn files_changed(app: &AppHandle, project: ProjectId, paths: &[PathBuf]) {
    // Resolved once per batch and not per path — `tasks_state::project_root`'s own shape, and
    // this runs on the watcher thread where a batch can be thousands of paths long.
    let Some(root) = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .and_then(|workspace| crate::tasks_state::project_root(&workspace, project).ok())
    else {
        return;
    };

    let mut tasks = false;
    let mut agents = false;
    let mut spec = false;
    for path in paths {
        match classify(path, &root) {
            Route::Tasks => tasks = true,
            Route::Agents => agents = true,
            Route::Spec => spec = true,
            Route::Nothing => {}
        }
        if tasks && agents && spec {
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

    if spec {
        // Marked, not read. The board is a subprocess away, so a burst of a hundred paths must
        // become one `openspec list` and not a hundred — `SpecBoards`' coalescer is the same one
        // the roster uses, and its `flushing` flag is also what keeps two reads from racing and
        // landing out of order.
        if let Some(boards) = app.try_state::<Arc<crate::spec_state::SpecBoards>>() {
            boards.mark_changed(app, project);
        }
        // And the checklist may have just been finished. Same events, a different question, and
        // deliberately a different module — see `crate::spec_triggers`.
        crate::spec_triggers::consider(app, project, paths, &root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/repo";

    fn route(path: &str) -> Route {
        classify(Path::new(path), Path::new(ROOT))
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

        // Anchored to the project root since M28, which tightened this rule for free: a vendored
        // dependency that ships its own `.cide/` used to fire on every one of its files.
        assert_eq!(route("/repo/vendor/x/.cide/tasks.json"), Route::Nothing);
        assert_eq!(route("/elsewhere/.cide/tasks.json"), Route::Nothing);
    }

    /// The project's own `openspec/`, and only that one. (M28)
    #[test]
    fn a_spec_path_has_to_be_the_projects_own() {
        assert_eq!(route("/repo/openspec/specs/dark-mode/spec.md"), Route::Spec);
        assert_eq!(
            route("/repo/openspec/changes/add-dark-mode/tasks.md"),
            Route::Spec
        );
        assert_eq!(route("/repo/openspec"), Route::Spec);

        // `openspec` is an ordinary lowercase word with no dot, so unlike `.cide` it really can
        // occur elsewhere: a project documenting its own tooling has `src/openspec/notes.md`, and
        // a component scan would fire on it. This is why `classify` takes a root at all.
        assert_eq!(route("/repo/src/openspec/notes.md"), Route::Nothing);
        assert_eq!(route("/repo/docs/openspec.md"), Route::Nothing);
        assert_eq!(route("/elsewhere/openspec/specs/x/spec.md"), Route::Nothing);
    }

    /// The one hole cut in the storm filter, and how narrow it is. (M28)
    #[test]
    fn a_live_agents_checklist_is_watched_and_the_rest_of_its_checkout_is_not() {
        // Why the hole exists: while a run is live the boxes it ticks are in *its* worktree, and
        // those events are what the task card's progress bar draws. Without this the bar sits at
        // zero for the whole of a run and jumps only at integration.
        assert_eq!(
            route("/repo/.cide/worktrees/dev/openspec/changes/c/tasks.md"),
            Route::Spec
        );
        assert_eq!(
            route("/repo/.cide/worktrees/dev-t-17/openspec/specs/a/spec.md"),
            Route::Spec
        );

        // And how narrow: the component after the agent's name has to be `openspec`, so the
        // build output that made the storm filter necessary still dies here.
        assert_eq!(
            route("/repo/.cide/worktrees/dev/target/debug/x"),
            Route::Nothing
        );
        assert_eq!(
            route("/repo/.cide/worktrees/dev/src/openspec/notes.md"),
            Route::Nothing
        );
        assert_eq!(route("/repo/.cide/worktrees/dev"), Route::Nothing);
        assert_eq!(route("/repo/.cide/worktrees"), Route::Nothing);
    }
    /// Claude Code's directory routes on `agents/` and on nothing else. (M30)
    ///
    /// The negative half is the half that matters. `.claude/projects/` is where the CLI writes a
    /// transcript, continuously, while a session is running: routing it here would rebuild the
    /// roster once per assistant token in every project with a live pane.
    #[test]
    fn dot_claude_routes_only_its_agents_directory() {
        let root = Path::new("/repo");
        assert_eq!(
            classify(&root.join(".claude/agents/reviewer.md"), root),
            Route::Agents
        );
        assert_eq!(
            classify(&root.join(".claude/agents/team/reviewer.md"), root),
            Route::Agents,
            "Claude Code allows a nested layout, and a change in one still changes the roster"
        );
        for quiet in [
            ".claude/projects/-repo/abc.jsonl",
            ".claude/settings.json",
            ".claude/sessions/1234.json",
            ".claude",
        ] {
            assert_eq!(classify(&root.join(quiet), root), Route::Nothing, "{quiet}");
        }
    }

    /// A subagent directory inside an agent's own checkout is still silence. (M30)
    ///
    /// A worktree is a whole copy of the project, `.claude/agents/` included when it is
    /// committed, and a run's `git checkout` moves every one of those files at once. The `.cide`
    /// arm's storm filter already eats this because `.cide` is the first component — pinned here
    /// because the `.claude` arm sits *above* that filter in the function and the ordering is
    /// what makes it safe.
    #[test]
    fn a_runs_own_checkout_never_rebuilds_the_projects_roster() {
        let root = Path::new("/repo");
        assert_eq!(
            classify(
                &root.join(".cide/worktrees/reviewer-t-1/.claude/agents/reviewer.md"),
                root
            ),
            Route::Nothing
        );
    }
}
