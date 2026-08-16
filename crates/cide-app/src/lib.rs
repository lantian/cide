//! The Tauri shell.
//!
//! This crate is glue. It owns window creation, the command surface and process
//! lifecycle; everything else lives in a domain crate that does not link a webview.

pub mod closed_tabs;
pub mod cmd;
pub mod emit;
// M8: one file index, picker and watcher per project.
pub mod files;
pub mod graphics;
pub mod groups;
pub mod hooks;
pub mod ide;
pub mod libraries;
pub mod lifecycle;
pub mod lsp;
pub mod positions_state;
pub mod scratches;
pub mod state;
pub mod symbols;
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

    let mut builder = tauri::Builder::default()
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
    // M12. Empty until a project opens, and managed from the start for the same reason as the two
    // above: `diagnostics.get` must resolve its state and answer `unavailable` with a sentence
    // rather than failing to resolve at all — a panel that cannot reach its state looks exactly
    // like a workspace with no problems.
    builder = builder.manage(lsp::DiagnosticsRegistry::default());
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
            cmd::file::claude_selection_changed,
            cmd::file::claude_send_lines,
            cmd::file::file_read,
            cmd::file::file_write,
            cmd::file::file_note_position,
            cmd::file::file_position,
            cmd::file::tab_open_file,
            cmd::file::terminal_open_path,
            cmd::file::tab_open_diff,
            cmd::file::tab_retarget_diff,
            cmd::file::tab_set_dirty,
            cmd::lifecycle::app_restore_plan,
            cmd::pane::pane_split,
            cmd::pane::pane_add_row,
            cmd::pane::pane_close,
            cmd::pane::pane_focus,
            cmd::pane::pane_maximize,
            cmd::pane::pane_set_ratio,
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
            cmd::fs::fs_writable_roots,
            cmd::fs::fs_rename,
            cmd::fs::fs_delete,
            cmd::fs::fs_paste,
            cmd::fs::fs_paste_plan,
            cmd::picker::picker_query,
            cmd::picker::picker_rank,
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
            cmd::diagnostics::diagnostics_restart,
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

            restore_windows(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the cide application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // This event covers a window close and a quit from the UI. A SIGTERM never
                // reaches it, which is why `lifecycle` also owns a signal thread; both call
                // the same function, and it runs once whichever gets there first.
                lifecycle::shutdown(app);
            }
        });
}
