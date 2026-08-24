//! The Tauri shell.
//!
//! This crate is glue. It owns window creation, the command surface and process
//! lifecycle; everything else lives in a domain crate that does not link a webview.

// M18: the `$CIDE_AGENT_SOCK` server — the task tools a dispatched agent and the project's own
// orchestrator reach over a unix socket, scoped from the connection's header line.
pub mod agent_rpc;
// M18: the run registry — the queue, the slots and every dispatched subagent run. The spawn
// itself is the ordinary session path (`SpawnSpec` -> `PtySession` -> `SessionRegistry`), which
// is what makes the shutdown ladder and the orphan sweep cover runs with no second
// implementation of either.
pub mod agents;
pub mod closed_tabs;
pub mod cmd;
pub mod edit_wait;
pub mod emit;
/// The `cide-ext://` scheme: an installed extension's own files, path-jailed and read-only.
pub mod ext_assets;
/// The extension registry: marketplaces, installs, and the contribution set they resolve to.
///
/// One store and not one per project — an extension is a tool the user chose, not a fact about a
/// repository. `ext_state.rs`'s header argues it, and `cide_ext::config`'s argues the file layout.
pub mod ext_state;
// M8: one file index, picker and watcher per project.
pub mod files;
pub mod graphics;
pub mod groups;
pub mod hooks;
pub mod ide;
pub mod libraries;
pub mod lifecycle;
pub mod lsp;
pub mod notes;
pub mod positions_state;
pub mod scratches;
// Comment stripping for this crate's structural source assertions. Test-only: a source
// assertion is a gate, not a product feature, and shipping the stripper in the binary would
// invite it to be used for something.
#[cfg(test)]
pub mod srcgrep;
pub mod state;
pub mod symbols;
// M18: one `.cide/tasks.json` store per open project, and the thread that ticks them.
pub mod tasks_state;
pub mod windows;
pub mod workspace_state;

use state::SessionRegistry;
use tauri::Manager;
use workspace_state::WorkspaceState;

/// Recreate the windows a saved workspace describes.
///
/// Two failures this replaces, both reported by review and both silent:
///
/// * A workspace saved in `PerProject` mode could not relaunch at all. Startup always made
///   one fresh `shell:<uuid>` and `adopt_shell_window` is deliberately a no-op outside
///   `Stacked`, so the live window carried a label the workspace had never heard of — every
///   lookup missed and the window showed no project while holding several.
/// * Detached-pane windows were never recreated. Detaching a pane and quitting left it in
///   `project.detached` with no window and no gesture that could reach it: the pane existed,
///   its conversation was resumable, and nothing rendered either.
///
/// A first launch has no windows recorded, so one shell is created and the domain is told
/// its real label.
fn restore_windows(app: &tauri::AppHandle) -> tauri::Result<()> {
    let state = app.state::<WorkspaceState>();
    let saved = state.snapshot();

    let recorded: Vec<_> = saved
        .windows
        .iter()
        .map(|(l, r)| (l.clone(), r.clone()))
        .collect();

    // A hard ceiling on how many windows a saved file may cause to open.
    //
    // This is not defensive programming for its own sake — it is here because the absence of
    // it made a development machine nearly unusable. A workspace accumulated 242 projects,
    // was left in `PerProject` mode, and this function faithfully opened a window for every
    // one of them on the next launch. Restoring state is exactly the path where a corrupt or
    // pathological file turns into an action the user cannot interrupt, so the restore
    // refuses rather than obeys, and says why.
    //
    // The excess windows are not lost: their roles stay in the workspace, so raising the cap
    // or fixing the file recovers them.
    const MAX_RESTORED_WINDOWS: usize = 8;
    if recorded.len() > MAX_RESTORED_WINDOWS {
        tracing::error!(
            recorded = recorded.len(),
            cap = MAX_RESTORED_WINDOWS,
            "refusing to restore this many windows; opening one shell instead"
        );
        eprintln!(
            "[cide] workspace records {} windows, which is past the {} the restore will open. \n\
             [cide] Opening a single window. The saved layout is untouched at {}.",
            recorded.len(),
            MAX_RESTORED_WINDOWS,
            state_path_hint()
        );
        let window = windows::create_shell(app)?;
        let label = cide_ipc::WindowLabel(window.label().to_string());
        let _ = state.update(|ws| {
            cide_core::workspace::adopt_shell_window(ws, label);
            Ok(())
        });
        return Ok(());
    }

    if recorded.is_empty() {
        let window = windows::create_shell(app)?;
        let label = cide_ipc::WindowLabel(window.label().to_string());
        let _ = state.update(|ws| {
            cide_core::workspace::adopt_shell_window(ws, label);
            Ok(())
        });
        return Ok(());
    }

    for (label, role) in recorded {
        let title = match &role {
            cide_ipc::WindowRole::Shell { .. } => "cide".to_string(),
            // A detached window is named for what it shows, since it has no tab strip to
            // say so and its own titlebar is all the user gets.
            cide_ipc::WindowRole::DetachedPane { project, pane, .. } => saved
                .projects
                .get(project)
                .and_then(|p| p.detached.get(pane))
                .map(|p| p.title.clone())
                .unwrap_or_else(|| "cide".to_string()),
            cide_ipc::WindowRole::DetachedTab { .. } => "cide".to_string(),
        };
        windows::create(app, &label, &title, None)?;
    }

    // Every window above was built with a placeholder name — "cide" for a shell, whatever the
    // pane was last called for a detached one — because this runs before anything has read the
    // workspace for a *title*. `retitle` derives all of them from the tree that was just
    // loaded, so a `PerProject` shell comes back named after its project instead of after the
    // application. The awaiting count is zero at this point and adds nothing.
    crate::cmd::window::retitle(app, &saved);
    Ok(())
}

/// Where the workspace file lives, for a message a user can act on.
fn state_path_hint() -> String {
    cide_core::persist::workspace_path().display().to_string()
}

/// The log destination, configured so that it can still hold anything worth reading.
///
/// **`Builder::default()` was making the log eat itself.** Its defaults are level `Trace`,
/// `max_file_size` 40 000 bytes and `RotationStrategy::KeepOne`, and the `notify` inotify
/// backend emits a TRACE line per watch descriptor — roughly 27 KB during start-up alone on a
/// project of any size. Everything written before that point was rotated out within a second
/// of launch, so by the time anyone opened the file the only thing in it was inotify
/// bookkeeping. That is why `app_open_log_dir` exists and why it has never been useful, and it
/// is what has to be fixed before any diagnostic added here is worth adding.
///
/// The two noisy targets are pinned to `Info` rather than raising the global floor: a
/// `tracing::debug!` from this workspace is exactly the thing a user is asked to send back.
fn log_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_log::Builder::default()
        .level_for("notify", log::LevelFilter::Info)
        .level_for("notify_debouncer_full", log::LevelFilter::Info)
        // Two megabytes, three files. A session's worth of diagnostics is a few hundred
        // kilobytes; the old 40 KB could not hold one project's start-up. `KeepSome` rather
        // than `KeepAll` because a log directory that grows for ever is the other way to
        // make a log useless.
        .max_file_size(2 * 1024 * 1024)
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(3))
        .build()
}

/// `deny(unused_variables)` is scoped to this function on purpose.
///
/// `run` is where every subsystem is built and handed to the app, so an unused binding here
/// means precisely one thing: something was constructed and then never wired up. That is not a
/// style problem, it is the shape of the bug this function keeps producing — `IdeServers` built
/// at launch and never offered the restored workspace, so every project came up with no
/// lockfile and no `CLAUDE_CODE_SSE_PORT` and nothing failed. Deleting the `install` call
/// leaves `ide` unused, and a warning in a build that emits none is still a thing a person has
/// to notice. This makes it stop the build instead.
#[deny(unused_variables)]
pub fn run() {
    // The audit compares against a mock stated at 1440x900, and this plugin restores
    // whatever size the window was last left at — quietly measuring some other viewport and
    // still reporting PASS. A diagnostic run starts from the stated geometry instead.
    let restore_geometry = std::env::var_os("CIDE_AUDIT").is_none();

    // Everything Tauri stores per application, moved onto the profile.
    //
    // `cide-core::persist` already moved every path this workspace computes for itself, but
    // four locations belong to Tauri and are resolved from the bundle identifier rather than
    // from us: the WebKit data directory (Tauri *forces* it to `LocalData/<identifier>` on
    // Linux, so localstorage, the cache and the cookie jar all live under it),
    // `tauri-plugin-log`'s output, `tauri-plugin-store`'s file, and
    // `tauri-plugin-window-state`'s geometry. Two instances sharing them means two processes
    // appending to one log and fighting over one remembered window size.
    //
    // Every one of those reads `config.identifier` at **runtime**, from this context — so
    // suffixing it here is the whole fix, and it is why the plugins registered below are not
    // a problem despite being registered first: each resolves its directory inside a `setup`
    // hook, which needs an `AppHandle` and therefore does not run until `build` below.
    //
    // `tauri.conf.json` is untouched, so packaging still asserts the shipped identifier.
    let mut context = tauri::generate_context!();
    if cide_core::profile::active().is_some() {
        let identifier = cide_core::profile::identifier(&context.config().identifier);
        tracing::info!(%identifier, "running under a profile");
        context.config_mut().identifier = identifier;
    }

    let mut builder = tauri::Builder::default()
        // `cide-ext://<marketplace>.<extension>/<path>`. Registered here rather than in `setup`
        // because a scheme handler has to exist before the first webview is created — a window
        // that opened first would have a renderer for which the scheme does not resolve, and the
        // failure shows as a worker that never loads rather than as anything about registration.
        .register_uri_scheme_protocol(cide_ext::assets::SCHEME, ext_assets::respond)
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(log_plugin());

    if restore_geometry {
        builder = builder.plugin(
            tauri_plugin_window_state::Builder::default()
                // Geometry only. DECORATIONS is deliberately excluded — we draw the
                // titlebar, so letting the plugin restore a decoration state would fight
                // `decorations(false)` on the next launch.
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                // Windows are labelled `shell:<uuid>` / `pane:<uuid>`, so without this
                // every launch would add a fresh geometry entry and the file would grow
                // without bound.
                .map_label(cide_ipc::state_key_of)
                .build(),
        );
    }

    // A failure here costs IDE integration, not the app: without it panes still spawn, the
    // terminal still works, and Claude prints its diffs as text in the pane the way it does
    // in any other terminal. Refusing to launch over it would trade a degraded feature for
    // no editor at all.
    let ide = match ide::PendingIdeServers::new() {
        Ok(servers) => Some(servers),
        Err(error) => {
            tracing::error!(%error, "could not start the IDE subsystem; panes will run without it");
            None
        }
    };

    builder = builder.manage(SessionRegistry::default());
    // Empty until a project asks to be indexed; managed from the start so that a `fs.*`
    // command arriving before any `fs.index` answers `NoIndex` rather than failing to
    // resolve its state.
    builder = builder.manage(files::FsRegistry::default());
    // Likewise: empty until the search panel asks for something, and managed from the start
    // so that `search.query` resolves its state and answers `NoIndex` rather than failing to
    // resolve at all.
    builder = builder.manage(cmd::search::SearchRegistry::default());
    // M20. Empty until a tool tab asks for a page of log, and managed from the start for the
    // reason the one above is: `git_log` claims its job before it dispatches the walk, so a
    // command that could not resolve this state would fail outright — and `git_log_cancel` would
    // fail on the very path whose whole job is to stop a walk that is already running.
    builder = builder.manage(cmd::log::LogRegistry::default());
    // M12. Empty until a project opens, and managed from the start for the same reason as the two
    // above: `diagnostics.get` must resolve its state and answer `unavailable` with a sentence
    // rather than failing to resolve at all — a panel that cannot reach its state looks exactly
    // like a workspace with no problems.
    builder = builder.manage(lsp::DiagnosticsRegistry::default());
    // M18. Empty until a project opens, and managed from the start for the reason the three
    // above are: `tasks_board` must resolve its state and answer with a `TaskBoard` — which has a
    // word for every one of "no tracker", "an empty tracker" and "a tracker that will not parse"
    // — rather than failing to resolve at all, which the panel could only draw as a blank list.
    //
    // Behind an `Arc` because the flusher thread outlives the call that starts it; Tauri's
    // `State` hands out a reference, which a `thread::spawn` cannot keep.
    builder = builder.manage(std::sync::Arc::new(tasks_state::TasksStores::default()));
    // M18. Empty until something is dispatched, and managed from the start for the same reason:
    // `agents_roster` reads it on every call, and a command that cannot resolve its state fails
    // rather than answering "nothing is running", which is what an empty registry already says.
    //
    // Behind an `Arc` because the coalescer's flusher thread and every `on_exit` callback outlive
    // the call that registered them; Tauri's `State` hands out a reference, which a
    // `thread::spawn` cannot keep.
    builder = builder.manage(std::sync::Arc::new(agents::AgentRegistry::default()));
    // `ide` is deliberately *not* managed here. It is a `PendingIdeServers`, whose one method
    // ensures every restored project's server and registers the state together — so it has to
    // wait for `setup`, where the restored workspace exists to be offered. Registering it here
    // is what used to let the two steps drift apart. See `ide::PendingIdeServers`.

    builder
        // Loaded before the first window exists, so `app.get_bootstrap` can answer from a
        // real tree on the very first call rather than serving defaults and correcting
        // itself a frame later.
        .manage(WorkspaceState::load())
        // Per-file view memory. Loaded here rather than in `setup` for the same reason the
        // workspace is: `file_position` can be asked the instant a restored editor mounts, and
        // a store that resolved to nothing for the first few hundred milliseconds would put
        // every restored tab at line 1 — precisely the bug it exists to fix.
        //
        // Behind an `Arc` because the flusher thread outlives the call that starts it; Tauri's
        // `State` hands out a reference, which a `thread::spawn` cannot keep.
        .manage(std::sync::Arc::new(positions_state::PositionsState::load()))
        // The Ctrl+Shift+T stack. Empty at launch by design — see `closed_tabs`'s note on why
        // it does not survive a restart — and managed here rather than in `setup` for the
        // plainer reason the two above are: `tab_close` pushes to it, and a command that cannot
        // resolve its state fails rather than merely losing a record.
        .manage(closed_tabs::ClosedTabs::default())
        // M22. Loaded here rather than in `setup` for the strongest version of the reason the
        // workspace is: `app_get_bootstrap` reads the resolved language table out of it, and that
        // table decides how the first editor folds. A registry that resolved to nothing for the
        // first few hundred milliseconds would mean every restored tab mounting with no fold spec
        // — precisely the failure `FoldSpecDto`'s note describes.
        //
        // Constructing it reads `extensions.json`, every marketplace clone and every installed
        // manifest, and publishes the result into `cide-lsp`'s server registry. That is disk work
        // on the main thread before the first window, and it is deliberate: it is bounded by the
        // number of installed extensions, and doing it lazily would mean the first window's
        // bootstrap raced it.
        .manage(ext_state::ExtState::new())
        .invoke_handler(tauri::generate_handler![
            cmd::app::app_quit_requested,
            cmd::app::app_ready,
            cmd::app::app_get_bootstrap,
            cmd::app::window_set_viewport,
            cmd::diag::diag_echo_bytes,
            cmd::diag::diag_push_bytes,
            cmd::diag::diag_report_ipc,
            cmd::diag::diag_bench_report,
            cmd::diag::diag_log,
            cmd::git::git_status,
            cmd::git::git_repos,
            cmd::git::git_locate,
            cmd::git::git_blame,
            cmd::git::git_blame_parent,
            cmd::git::git_log,
            cmd::log::git_log_cancel,
            cmd::git::git_commit_detail,
            cmd::git::git_commit_line_counts,
            cmd::git::git_diff_revision,
            cmd::git::git_diff_revision_files,
            cmd::git::git_file_at_revision,
            cmd::git::git_resolve_rev,
            cmd::git::git_tree_status,
            cmd::git::git_branch_info,
            cmd::git::git_branch_list,
            cmd::git::git_branch_create,
            cmd::git::git_branch_checkout,
            cmd::git::git_branch_blockers,
            cmd::git::git_branch_rename,
            cmd::git::git_branch_delete,
            cmd::git::git_fetch,
            cmd::git::git_pull,
            cmd::git::git_merge,
            cmd::file::tab_open_merge,
            cmd::git::git_conflicts,
            cmd::git::git_conflict_read,
            cmd::git::git_conflict_resolve,
            cmd::git::git_conflict_take,
            cmd::git::git_conflict_unresolve,
            cmd::git::git_merge_continue,
            cmd::git::git_merge_abort,
            cmd::git::git_diff_file,
            cmd::git::git_resolve_selection,
            cmd::git::git_stage,
            cmd::git::git_unstage,
            cmd::git::git_rollback,
            cmd::git::git_commit,
            cmd::git::git_push,
            cmd::git::git_adopt_index,
            cmd::git::git_set_use_staging_area,
            cmd::git::git_changelist_create,
            cmd::git::git_changelist_rename,
            cmd::git::git_changelist_delete,
            cmd::git::git_changelist_move_paths,
            cmd::git::git_changelist_set_active,
            cmd::git::git_shelve,
            cmd::git::git_unshelve,
            cmd::git::git_shelf_list,
            cmd::git::git_shelf_patch,
            cmd::git::git_shelf_drop,
            cmd::git::git_stash_save,
            cmd::git::git_stash_list,
            cmd::git::git_stash_pop,
            cmd::git::git_stash_apply,
            cmd::git::git_stash_drop,
            // The commit actions — the log's right-click menu. `git_reset_preview` is the
            // read that has to answer before the confirmation dialog can be honest; the other
            // five mutate and each broadcasts `cide://git-status`. Amend and *branch from
            // here* are absent on purpose: they are `git_commit` with `amendOf` and
            // `git_branch_create` with a `startPoint`. See `cmd/git.rs`'s block header.
            cmd::git::git_revert,
            cmd::git::git_cherry_pick,
            cmd::git::git_reset_preview,
            cmd::git::git_reset,
            cmd::git::git_tag_create,
            cmd::git::git_checkout_detached,
            cmd::file::claude_selection_changed,
            cmd::file::claude_send_lines,
            cmd::file::claude_session_names,
            cmd::file::file_read,
            cmd::file::image_read,
            cmd::file::file_write,
            cmd::file::file_note_position,
            cmd::file::file_position,
            cmd::file::tab_open_file,
            cmd::file::tab_reopen_file,
            cmd::file::terminal_open_path,
            cmd::file::tab_open_diff,
            cmd::file::tab_retarget_diff,
            cmd::file::tab_open_revision_diff,
            cmd::file::tab_retarget_revision_diff,
            cmd::file::tab_open_revision,
            cmd::file::tab_set_dirty,
            cmd::lifecycle::app_restore_plan,
            cmd::pane::pane_split,
            cmd::pane::pane_add_row,
            cmd::pane::pane_close,
            cmd::pane::pane_focus,
            cmd::pane::pane_maximize,
            cmd::pane::pane_set_ratio,
            cmd::pane::pane_distribute,
            cmd::pane::pane_navigate,
            cmd::pane::pane_swap,
            cmd::pane::pane_bind_session,
            cmd::project::project_open,
            cmd::project::project_activate,
            cmd::project::project_close,
            cmd::project::project_reorder,
            cmd::project::project_pick,
            cmd::project::project_recent,
            cmd::project::project_open_recent,
            cmd::project::project_forget_recent,
            cmd::project::project_reveal,
            cmd::project::tab_new_claude,
            cmd::project::tab_activate,
            cmd::project::tab_close,
            cmd::project::tab_reorder,
            cmd::project::tab_reopen_closed,
            cmd::session::session_spawn,
            cmd::session::session_attach,
            cmd::session::claude_diff_content,
            cmd::session::claude_diff_result,
            cmd::session::session_ack,
            cmd::session::session_detach,
            cmd::session::session_scrollback,
            cmd::session::session_cwd,
            cmd::session::session_in_alternate_screen,
            cmd::session::session_write,
            cmd::session::session_resize,
            cmd::session::session_exit,
            cmd::session::session_has_exited,
            cmd::session::session_list,
            cmd::session::session_kill,
            cmd::session::session_resumable,
            cmd::settings::settings_get,
            cmd::settings::settings_set,
            cmd::settings::tab_open_settings,
            cmd::settings::keymap_report,
            cmd::settings::keymap_edit,
            cmd::settings::graphics_status,
            cmd::settings::claude_headless,
            cmd::settings::claude_commit_message,
            cmd::settings::claude_explain_selection,
            cmd::settings::claude_cli_support,
            cmd::settings::app_open_log_dir,
            cmd::settings::scheme_import,
            cmd::settings::scheme_remove,
            cmd::window::window_detach_pane,
            cmd::window::window_redock_pane,
            cmd::window::window_reveal_pane,
            cmd::window::window_set_mode,
            cmd::window::window_close,
            cmd::window::window_list,
            cmd::window::window_set_awaiting,
            cmd::window::window_awaiting_sessions,
            cmd::fs::fs_index,
            cmd::fs::fs_close,
            cmd::fs::fs_status,
            cmd::fs::fs_tree_count,
            cmd::fs::fs_tree_rows,
            cmd::fs::fs_tree_match,
            cmd::fs::tree_match_labels,
            cmd::fs::fs_expand,
            cmd::fs::fs_collapse,
            cmd::fs::fs_reveal,
            cmd::fs::fs_show_in_manager,
            cmd::fs::fs_paths_exist,
            cmd::fs::fs_stat_paths,
            cmd::fs::fs_read_file,
            cmd::fs::fs_write_file,
            cmd::fs::fs_create,
            cmd::fs::fs_create_in,
            cmd::fs::fs_scratch_new,
            cmd::fs::fs_notes_ensure,
            cmd::fs::fs_writable_roots,
            cmd::fs::fs_reveal_roots,
            cmd::fs::fs_rename,
            cmd::fs::fs_delete,
            cmd::fs::fs_paste,
            cmd::fs::fs_paste_plan,
            cmd::picker::picker_query,
            cmd::picker::picker_rank,
            cmd::picker::picker_index_libraries,
            cmd::search::search_query,
            cmd::search::search_cancel,
            cmd::symbols::symbols_outline,
            cmd::symbols::symbols_index,
            cmd::symbols::symbol_query,
            cmd::diagnostics::diagnostics_get,
            cmd::diagnostics::diagnostics_did_open,
            cmd::diagnostics::diagnostics_did_change,
            cmd::diagnostics::diagnostics_did_save,
            cmd::diagnostics::diagnostics_did_close,
            cmd::diagnostics::diagnostics_definition,
            cmd::diagnostics::diagnostics_probe,
            cmd::diagnostics::diagnostics_usages,
            cmd::diagnostics::diagnostics_usages_cancel,
            cmd::diagnostics::diagnostics_implementations,
            cmd::diagnostics::diagnostics_refresh,
            cmd::diagnostics::diagnostics_restart,
            // --- M18: the git tool window ---
            cmd::toolwindow::tool_window_set_layout,
            cmd::toolwindow::tool_window_activate,
            cmd::toolwindow::tool_window_open_history,
            cmd::toolwindow::tool_window_close_history,
            // --- M18: the task tracker ---
            cmd::tasks::tasks_board,
            cmd::tasks::task_new,
            cmd::tasks::task_edit,
            cmd::tasks::task_delete,
            // --- M18: subagent orchestration ---
            cmd::agents::agents_roster,
            cmd::agents::agents_config_get,
            cmd::agents::agents_config_set,
            cmd::agents::agents_dispatch,
            cmd::agents::agents_stop,
            cmd::agents::agents_pause,
            cmd::agents::agents_resume,
            cmd::agents::agents_retry_turn,
            cmd::agents::agents_ack_stale_turn,
            cmd::agents::agents_integrate,
            cmd::agents::agents_draft,
            cmd::agents::agents_save,
            cmd::agents::agents_delete,
            // --- M22: extensions and their marketplaces ---
            cmd::ext::ext_snapshot,
            cmd::ext::ext_reload,
            cmd::ext::ext_connect,
            cmd::ext::ext_disconnect,
            cmd::ext::ext_refresh,
            cmd::ext::ext_install,
            cmd::ext::ext_uninstall,
            cmd::ext::ext_set_enabled,
            cmd::ext::ext_set_setting,
            cmd::ext::ext_publish_diagnostics,
            cmd::ext::ext_page,
            cmd::ext::tab_open_extension,
            cmd::ext::ext_open_link,
        ])
        .on_window_event(|window, event| {
            match event {
                // A close from the window manager — Alt+F4, the compositor's own button —
                // never reaches a command, and for a detached pane the difference is the pane
                // itself: it has to go back in its tab rather than disappear with the window
                // that was showing it, taking a live session out of reach.
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    if cmd::window::intercept_close(
                        window.app_handle(),
                        &cide_ipc::WindowLabel(window.label().to_string()),
                    ) {
                        api.prevent_close();
                    }
                }

                /*
                 * Where the user is, which is half of whether a window should be asking for
                 * them.
                 *
                 * **Both directions, and the `false` one is the fix for "i saw it once only".**
                 * The old arm was `Focused(true)` alone, lowering the urgency hint because
                 * Tauri documents that a window manager "might not" clear it on input. That
                 * half is still here and still required. What was missing is that an urgency
                 * hint on the *active* window is meaningless — KWin's `demandAttention` starts
                 * `if (isActive()) set = false;` — so the request made the instant a turn
                 * finished was **dropped** whenever the user happened to be in the window, and
                 * the old comment here ("the hint goes back up when the count next changes")
                 * was wrong about the one case that matters: for a session that is already
                 * waiting, the count never changes again. The user's loop after the first turn
                 * is come back, read it, type the next thing, stay in the window while it runs
                 * — so every turn after the first raised a hint into a focused window, had it
                 * dropped, and nothing re-asked when they walked away.
                 *
                 * So the hint is a level now, not an edge: `focus_changed` records where the
                 * user is and re-runs `retitle`, which recomputes both surfaces of every
                 * window from `windows::announce`. Coming to a window lowers its hint; leaving
                 * a window that still holds an unread finished turn raises it.
                 *
                 * Deliberately **still only the window's own surfaces**, and not the awaiting
                 * set. Coming to a window is not "I have read every session in it": a shell
                 * holds every tab of every project it shows, and clearing those markers on a
                 * window focus would wipe the pane and project badges for conversations the
                 * user has not opened. Those clear per pane, on a click or a keystroke into
                 * that pane, which is the act that actually means someone read it. The hint is
                 * about the *window*; the markers are about the sessions.
                 */
                tauri::WindowEvent::Focused(has_focus) => {
                    cmd::window::focus_changed(
                        window.app_handle(),
                        &cide_ipc::WindowLabel(window.label().to_string()),
                        *has_focus,
                    );
                }

                // A window that has gone takes its focus record with it, so a reused label
                // can never inherit "the user is already in there" and fall silent for ever.
                tauri::WindowEvent::Destroyed => {
                    crate::windows::forget_focused(&cide_ipc::WindowLabel(
                        window.label().to_string(),
                    ));
                }

                _ => {}
            }
        })
        .setup(move |app| {
            // Before any window exists, so the first mutation can already broadcast.
            app.state::<WorkspaceState>()
                .attach_app(app.handle().clone());
            // Before the first window, so a signal arriving during startup still finds a
            // shutdown path rather than the default disposition.
            lifecycle::install_signal_handlers(app.handle());

            // The view-position store's own flusher. Its own thread and its own timer because
            // there is no app tick to hang it on — `WorkspaceState::flush_if_due` documents one
            // and is called by nobody, which is why `workspace.json` is in fact written once
            // per run. See `positions_state.rs`.
            app.state::<std::sync::Arc<positions_state::PositionsState>>()
                .start_flusher();

            // And the task trackers'. Same shape, same reason, and one more of its own: this is
            // the only store in the process whose file has writers cide does not control, so its
            // tick is also what notices a `git pull` moving `.cide/tasks.json` under an open
            // panel and broadcasts the merged board. See `tasks_state`.
            app.state::<std::sync::Arc<tasks_state::TasksStores>>()
                .start_flusher(app.handle().clone());

            // Before the listener binds, so a `SIGKILL`ed previous run's socket is gone
            // rather than accumulating one file per hard kill for the life of the account.
            // Keyed on pid liveness and refuses anything it cannot establish as dead — see
            // `cide_claude::orphans::sweep_hook_sockets`.
            let swept = cide_claude::orphans::sweep_hook_sockets();
            if swept > 0 {
                tracing::info!(swept, "removed hook sockets left by a previous run");
            }

            // Before any pane spawns, because a child that starts without `CIDE_HOOK_SOCK`
            // in its environment never reports at all — there is no later moment at which it
            // can be told where to send hooks.
            match hooks::HookServer::start(app.handle().clone()) {
                Ok(server) => {
                    app.manage(server);
                }
                // Costs live token figures, the fast buffer reload, and a close confirm that
                // can tell busy from idle. Claude Code runs fine with no hooks configured, so
                // this degrades rather than refusing to launch.
                Err(error) => {
                    tracing::error!(%error, "no hook socket; sessions will report no state")
                }
            }

            // And the agent-RPC socket, beside it and for the identical ordering reason: a child
            // that starts without `CIDE_AGENT_SOCK` in its environment can never be told where
            // the task tools are, because `--mcp-config` is read at exec and the bridge reads the
            // variable once, on connect.
            //
            // A *separate* socket from the one above rather than a second vocabulary on it — the
            // hook socket is write-and-forget and strictly serialised, this one is a
            // request/response conversation that does file I/O. See `agent_rpc`'s header.
            match agent_rpc::AgentRpcServer::start(app.handle().clone()) {
                Ok(server) => {
                    app.manage(server);
                }
                // Costs every session its `cide_task_*` tools; costs nothing else. The bridge is
                // written to degrade into a valid MCP server with an empty tool list when it
                // cannot connect, precisely so that this failure is not a broken `claude`.
                Err(error) => {
                    tracing::error!(%error, "no agent rpc socket; sessions get no task tools")
                }
            }

            // And the editor socket, third and last of the three, for the same ordering reason
            // again: `CIDE_EDIT_SOCK` is read out of a child's environment at exec, so a pane
            // spawned before this binds has no way to be told about it later.
            //
            // A *separate* socket from both above rather than a fourth verb on either — the hook
            // socket is write-and-forget and strictly serialised, the agent socket's header line
            // is an authorisation decision about MCP tools, and this one blocks a thread for as
            // long as a human is editing a file. See `edit_wait`'s header.
            match edit_wait::EditWaitServer::start(app.handle().clone()) {
                Ok(server) => {
                    app.manage(server);
                }
                // Costs Ctrl+G in a Claude pane and nothing else: `child_env::editor_env` sets
                // `EDITOR` only when there is a socket to name, so a child gets exactly the
                // environment it had before this feature existed and the CLI falls back to its
                // own guess.
                Err(error) => {
                    tracing::error!(%error, "no edit socket; $EDITOR is left to the user's own")
                }
            }

            // One IDE server per restored project, for the same ordering reason as the hook
            // socket above and with the same consequence for getting it wrong: a pane spawns
            // with `CLAUDE_CODE_SSE_PORT` read out of the environment at exec, and a `claude`
            // that starts without it never finds this project's server at all.
            //
            // `ensure` is otherwise called only from `project_open`, which a restoring launch
            // never reaches — the projects arrive from `workspace.json` — so without this the
            // headline feature was missing on every launch but the first.
            if let Some(pending) = ide {
                // The snapshot is taken and dropped before any server starts: `ensure` blocks
                // on a runtime, and holding the workspace lock across that would stall every
                // command behind a port bind.
                let ws = app.state::<WorkspaceState>().snapshot();
                // Ensuring and managing are one call because they were two, and the bug was
                // that they could disagree. See `ide::PendingIdeServers`.
                pending.install(app.handle(), &ws);
            }

            // And one set of language servers per restored project — the exact same omission as
            // the block above, one feature over, with the same cause and the same symptom.
            //
            // `DiagnosticsRegistry::ensure` is otherwise called only from `project_open`, which a
            // restoring launch never reaches, so every launch but the one where the user opened
            // the project by hand came up with no analyser at all: no rust-analyzer process, an
            // `unavailable` panel, `✗ — ⚠ —` in the status bar, and `getDiagnostics` answering
            // `[]` to Claude. Each of those is individually indistinguishable from "this
            // workspace is fine", which is what made it survive a full green gate.
            //
            // After `pending.install`, deliberately: `ensure` here links each project's store
            // into the IDE server that the line above just started, so `getDiagnostics` answers
            // from live data on a restored launch too. `ide::link_diagnostics` documents why the
            // link is made from both ends rather than by fixing an order.
            {
                let ws = app.state::<WorkspaceState>().snapshot();
                if let Some(registry) = app.try_state::<lsp::DiagnosticsRegistry>() {
                    for (project, roots) in ide::servable_projects(&ws) {
                        registry.ensure(app.handle(), project, roots);
                        ide::link_diagnostics(app.handle(), project);
                    }
                }
            }

            // And one task tracker per restored project — the *third* registry to need this line,
            // written here because the two blocks above are each a record of what happens without
            // it: a registry reachable only from `project_open` is a registry every restoring
            // launch comes up without, and in all three cases the absence has no symptom of its
            // own. A project with no store here would have a Tasks panel that works (the commands
            // `ensure` for themselves) and a flusher that watches nothing — so an agent's comment
            // or a teammate's `git pull` would sit unwritten and unseen until the next mutation,
            // which is precisely the class of failure that has no error message.
            //
            // `servable_projects` rather than a walk of its own, for the reason its doc gives
            // about the language servers: every one of these features needs "every project the
            // restored workspace holds", and two walks of one tree are two chances for a project
            // to get one half and not the other. Its 32-project cap costs nothing here — the
            // commands `ensure` on their own — beyond the tick, which the next `project_open`
            // restores.
            {
                let ws = app.state::<WorkspaceState>().snapshot();
                let stores = app.state::<std::sync::Arc<tasks_state::TasksStores>>();
                for (project, roots) in ide::servable_projects(&ws) {
                    // `roots[0]`: one project, one tracker. A project with no roots fails
                    // validation, so this only ever skips a file we could not have named.
                    if let Some(root) = roots.first() {
                        stores.ensure(project, root);
                    }
                }
            }

            restore_windows(app.handle())?;
            Ok(())
        })
        .build(context)
        .expect("failed to build the cide application")
        .run(|app, event| {
            match event {
                // `ExitRequested` covers a window close and a quit from the UI. A SIGTERM
                // never reaches it, which is why `lifecycle` also owns a signal thread.
                //
                // It is answered by `lifecycle::exit_requested` rather than by `shutdown`
                // directly, because this arm runs **on the GTK main thread** and the teardown
                // it used to run inline takes seconds: two file writes, the IDE servers, every
                // language server's own kill ladder and then the children's. Nothing repainted
                // for the whole of it and the compositor greyed the window out — the "closing
                // freezes the window" report. `exit_requested` holds the exit with
                // `api.prevent_exit()`, runs the teardown on a worker, and asks for the exit
                // again when it is done; `api` is what makes that possible, so this arm has to
                // bind it rather than match `{ .. }`.
                //
                // `Exit` is the *other* way out, and on macOS it is the only one the common
                // gesture takes. ⌘Q goes `NSApp terminate:` → `applicationWillTerminate` →
                // `AppState::exit()` → `Event::LoopDestroyed`, which tauri-runtime-wry turns
                // into `RunEvent::Exit` — `ExitRequested` is never raised and no signal is
                // sent, so with only the arm above a ⌘Q would skip `shutdown` entirely: the
                // 500 ms workspace debounce would go unflushed and the SIGHUP→SIGTERM→SIGKILL
                // ladder would never run, orphaning every `claude` and every language server.
                // That is the whole of the quit path on a Mac, because cide binds no quit
                // command of its own and ⌘Q comes from tauri's default macOS menu.
                //
                // Costs Linux nothing and is a small correctness win there too: `Exit` also
                // follows `app.exit(code)` and a runtime-initiated teardown, both of which
                // reach here without an `ExitRequested`. `shutdown` sets `SHUTTING_DOWN`
                // first, so the ordinary Linux sequence — `ExitRequested`, the teardown on its
                // worker, the worker's own `app.exit`, a second `ExitRequested`, then `Exit` —
                // still runs the ladder exactly once. What the later calls do is *wait* for
                // the one in flight, which by then has already finished.
                tauri::RunEvent::ExitRequested { code, api, .. } => {
                    lifecycle::exit_requested(app, code, &api);
                }
                tauri::RunEvent::Exit => {
                    lifecycle::shutdown(app);
                }
                _ => {}
            }
        });
}
