//! *Check for Updates…* in the macOS application menu, under *About cide*.
//!
//! cide never built a menu bar of its own: it calls no `.menu()`, so tauri installs
//! `Menu::default` on macOS (`docs/platforms.md`, *The menu bar*). That default is kept whole
//! here — the app menu, Edit, View, Window — and one item is inserted after *About*. Replacing
//! it with a menu of cide's own was the option that lost: on macOS the Edit submenu's items are
//! a large part of how ⌘C/⌘V reach a text view at all, ⌘Q *is* the Quit item (cide has no
//! `app.quit` command), and `keymap::MACOS_MENU_CHORDS` pins exactly the default's accelerators.
//! Keeping the default keeps all three true. The new item has no accelerator for the same
//! reason: a chord there would be a thirteenth one AppKit resolves ahead of the web view.
//!
//! The click does not check anything in Rust. The check's answer is a toast — *available*,
//! *latest*, *could not check* — drawn by `ui/src/chrome/updates.ts`'s `checkForUpdates`, the
//! same function the command palette and the About card call. So the item only asks one shell
//! window to run it (`cide://update-check-requested`). One window, not every window: in
//! `PerProject` mode each shell window draws the app-wide notices, and a broadcast would ask
//! GitHub once per window and stack the same answer in each.
//!
//! Like `dock`'s AppKit half, the macOS arm type-checks for `aarch64-apple-darwin` and has
//! **never been run**. [`target`] is the part that can be tested here, and is.

use cide_ipc::{WindowLabel, WindowRole};

/// The menu item's id, which `on_menu_event` matches on.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const CHECK_FOR_UPDATES: &str = "cide.checkForUpdates";

/// Which window should answer a click in the menu bar.
///
/// The focused shell window, which is the one the user is looking at when they reach for the
/// menu bar. None focused is possible — the menu bar stays up while a detached pane's window
/// has the focus, and that window draws no update notices — so then the first shell window, in
/// the workspace's own order. `None` only when there is no shell window at all, and then there
/// is nowhere to draw an answer.
pub fn target<'a>(
    windows: impl IntoIterator<Item = (&'a WindowLabel, &'a WindowRole)>,
    focused: impl Fn(&WindowLabel) -> bool,
) -> Option<WindowLabel> {
    let shells: Vec<&WindowLabel> = windows
        .into_iter()
        .filter(|(_, role)| matches!(role, WindowRole::Shell { .. }))
        .map(|(label, _)| label)
        .collect();
    let focused_shell = shells.iter().find(|label| focused(label));
    focused_shell
        .or(shells.first())
        .map(|label| (*label).clone())
}

/// Put *Check for Updates…* in the application menu. Called once, from `setup`. A no-op off
/// macOS, where cide has no menu bar.
pub fn install(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    if let Err(error) = macos::install(app) {
        // Logged and dropped: the default menu is still there, and the About card and the
        // command palette still check. Failing `setup` over a menu item would be worse.
        tracing::warn!(%error, "could not add Check for Updates to the application menu");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(target_os = "macos")]
mod macos {
    use super::CHECK_FOR_UPDATES;
    use crate::workspace_state::WorkspaceState;
    use tauri::menu::{Menu, MenuItem};
    use tauri::{AppHandle, Manager};

    pub(super) fn install(app: &AppHandle) -> tauri::Result<()> {
        let menu = Menu::default(app)?;
        // The first submenu is the application menu, and *About* is its first item.
        let app_menu = menu
            .items()?
            .into_iter()
            .find_map(|item| item.as_submenu().cloned());
        let Some(app_menu) = app_menu else {
            tracing::warn!("the default menu has no application submenu; leaving it alone");
            return Ok(());
        };
        let item = MenuItem::with_id(
            app,
            CHECK_FOR_UPDATES,
            "Check for Updates…",
            true,
            None::<&str>,
        )?;
        app_menu.insert(&item, 1)?;
        app.set_menu(menu)?;
        app.on_menu_event(|app, event| {
            if event.id() == CHECK_FOR_UPDATES {
                requested(app);
            }
        });
        Ok(())
    }

    fn requested(app: &AppHandle) {
        let Some(state) = app.try_state::<WorkspaceState>() else {
            return;
        };
        let window = state.with(|ws| super::target(&ws.windows, crate::windows::is_focused));
        match window {
            Some(window) => crate::emit::update_check_requested(app, &window),
            None => tracing::debug!("Check for Updates with no shell window to answer it"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> WindowRole {
        WindowRole::Shell {
            projects: Vec::new(),
            active: None,
        }
    }

    fn label(s: &str) -> WindowLabel {
        WindowLabel(s.into())
    }

    #[test]
    fn the_focused_shell_window_answers() {
        let windows = [(label("a"), shell()), (label("b"), shell())];
        assert_eq!(
            target(windows.iter().map(|(l, r)| (l, r)), |l| l.0 == "b"),
            Some(label("b"))
        );
    }

    #[test]
    fn with_no_shell_focused_the_first_shell_answers() {
        let windows = [(label("a"), shell()), (label("b"), shell())];
        assert_eq!(
            target(windows.iter().map(|(l, r)| (l, r)), |_| false),
            Some(label("a"))
        );
    }

    #[test]
    fn a_detached_window_never_answers() {
        let detached = WindowRole::DetachedTab {
            project: cide_ipc::ProjectId::new(),
            tab: cide_ipc::TabId::new(),
        };
        let windows = [(label("a"), detached)];
        assert_eq!(target(windows.iter().map(|(l, r)| (l, r)), |_| true), None);
    }
}
