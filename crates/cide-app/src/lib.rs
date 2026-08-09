//! The Tauri shell.
//!
//! This crate is glue. It owns window creation, the command surface and process
//! lifecycle; everything else lives in a domain crate that does not link a webview.

pub mod cmd;
pub mod emit;
pub mod graphics;
pub mod hooks;
pub mod ide;
pub mod lifecycle;
pub mod state;
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
    Ok(())
}

/// Where the workspace file lives, for a message a user can act on.
fn state_path_hint() -> String {
    cide_core::persist::workspace_path().display().to_string()
}

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
        .plugin(tauri_plugin_log::Builder::default().build());

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
    let ide = match ide::IdeServers::new() {
        Ok(servers) => Some(servers),
        Err(error) => {
            tracing::error!(%error, "could not start the IDE subsystem; panes will run without it");
            None
        }
    };

    builder = builder.manage(SessionRegistry::default());
    if let Some(ide) = ide {
        builder = builder.manage(ide);
    }

    builder
        // Loaded before the first window exists, so `app.get_bootstrap` can answer from a
        // real tree on the very first call rather than serving defaults and correcting
        // itself a frame later.
        .manage(WorkspaceState::load())
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
            cmd::file::file_read,
            cmd::file::file_write,
            cmd::file::tab_open_file,
            cmd::file::tab_set_dirty,
            cmd::lifecycle::app_restore_plan,
            cmd::pane::pane_split,
            cmd::pane::pane_close,
            cmd::pane::pane_focus,
            cmd::pane::pane_maximize,
            cmd::pane::pane_set_ratio,
            cmd::pane::pane_navigate,
            cmd::pane::pane_swap,
            cmd::pane::pane_bind_session,
            cmd::project::project_open,
            cmd::project::project_close,
            cmd::project::project_reorder,
            cmd::project::tab_new_claude,
            cmd::project::tab_activate,
            cmd::project::tab_close,
            cmd::session::session_spawn,
            cmd::session::session_attach,
            cmd::session::claude_diff_content,
            cmd::session::claude_diff_result,
            cmd::session::session_ack,
            cmd::session::session_detach,
            cmd::session::session_scrollback,
            cmd::session::session_in_alternate_screen,
            cmd::session::session_write,
            cmd::session::session_resize,
            cmd::session::session_has_exited,
            cmd::session::session_list,
            cmd::session::session_kill,
            cmd::window::window_detach_pane,
            cmd::window::window_redock_pane,
            cmd::window::window_set_mode,
            cmd::window::window_close,
            cmd::window::window_list,
        ])
        .on_window_event(|window, event| {
            // A close from the window manager — Alt+F4, the compositor's own button — never
            // reaches a command, and for a detached pane the difference is the pane itself:
            // it has to go back in its tab rather than disappear with the window that was
            // showing it, taking a live session out of reach.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && cmd::window::intercept_close(
                    window.app_handle(),
                    &cide_ipc::WindowLabel(window.label().to_string()),
                )
            {
                api.prevent_close();
            }
        })
        .setup(|app| {
            // Before any window exists, so the first mutation can already broadcast.
            app.state::<WorkspaceState>()
                .attach_app(app.handle().clone());
            // Before the first window, so a signal arriving during startup still finds a
            // shutdown path rather than the default disposition.
            lifecycle::install_signal_handlers(app.handle());

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
