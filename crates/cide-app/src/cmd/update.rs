//! Self-update commands. The logic is `crate::updater`; these are its doors.
//!
//! *Skip this version* has no command of its own: it is `settings_set` with the `update` group,
//! so the skip is stored, bumped and broadcast exactly like any other setting, and the Settings
//! screen's *clear* undoes it through the same road.

use cide_ipc::{UpdateCheck, UpdateInfo};
use tauri::{AppHandle, State};

use crate::updater::{self, UpdaterState};
use crate::workspace_state::WorkspaceState;

/// The update the start-up check found, for a window that mounted after the event went out.
/// `None` once it is installed: that window is owed nothing but the restart, and a notice
/// offering to download what is already on disk would be a second download. `None` too when it
/// is the skipped version — the same rule the start-up event obeys, or a window opened later
/// would announce what the user already declined.
#[tauri::command]
pub fn update_status(
    state: State<'_, UpdaterState>,
    workspace: State<'_, WorkspaceState>,
) -> Option<UpdateInfo> {
    let skipped = workspace.with(|ws| ws.settings.update.skipped_version.clone());
    state
        .status()
        .filter(|info| updater::announce(&info.version, skipped.as_deref()))
}

/// *Check for updates*, asked by the user. Ignores the skipped version — asking is the user
/// overriding their own skip — and folds a failed request into [`UpdateCheck::Unavailable`] so
/// the window has one sentence to show whatever went wrong.
#[tauri::command]
pub async fn update_check(app: AppHandle) -> UpdateCheck {
    match updater::check(&app).await {
        Ok(check) => check,
        Err(error) => UpdateCheck::Unavailable {
            reason: format!("Could not reach the release server: {error}"),
        },
    }
}

/// Download and install the release the last check found. Progress arrives as
/// `cide://update-progress`, completion as `cide://update-ready` as well as in the result, so
/// every window learns it and not only the one whose button was pressed.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<String, String> {
    updater::install(&app).await
}

/// Restart into the installed version. The frontend asks first when anything is unsaved or
/// mid-turn (`app_quit_requested`, the quit dialog's own question).
///
/// `AppHandle::restart` ignores `prevent_exit`, which is why `lifecycle::exit_requested` runs the
/// teardown inline for `RESTART_EXIT_CODE`: the workspace is flushed and every child stopped
/// before the new process starts, and the runs it interrupted are resumed by it (M118).
#[tauri::command]
pub fn update_restart(app: AppHandle) {
    app.restart();
}
