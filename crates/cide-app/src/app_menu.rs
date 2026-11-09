//! The macOS menu bar: tauri's default menu, plus a File menu and *Settings…* of cide's own.
//! (M127, M198)
//!
//! cide never built a menu bar of its own: it calls no `.menu()`, so tauri installs
//! `Menu::default` on macOS (`docs/platforms.md`, *The menu bar*). That default is kept whole
//! here — the app menu, Edit, View, Window and the File submenu's *Close Window* — and items are
//! **inserted** into it rather than the menu being replaced. Replacing it was the option that
//! lost: on macOS the Edit submenu's items are a large part of how ⌘C/⌘V reach a text view at all,
//! ⌘Q *is* the Quit item (cide has no `app.quit` command), and `keymap::MACOS_MENU_CHORDS` pins
//! exactly the default's accelerators. Keeping the default keeps all three true.
//!
//! # What is added
//!
//! * The application menu: *Check for Updates…* under *About* (M127), and *Settings…* in its own
//!   group below — where every Mac app keeps it.
//! * The File menu, which the default leaves holding only *Close Window* (M198): New Project…,
//!   New Scratch File…, Open Project…, *Open Recent ▸*, Save, Save All, Close Tab, Reopen Closed
//!   Tab, Close Project and Settings… again — [`FILE_MENU`] is the list. They go **above** the
//!   default's *Close Window*, which stays where macOS users look for it, with its ⌘W.
//!
//! # No accelerators, on any of them
//!
//! AppKit resolves a menu item's key equivalent in `performKeyEquivalent:` before WKWebView is
//! asked, so an accelerator here would not *label* cide's binding, it would **take the chord away
//! from the keymap**: a user who rebound ⌘, in `keymap.json` would find the menu still answering
//! it, and every one would be a thirteenth entry in `MACOS_MENU_CHORDS`. So the menu is the
//! mouse's road to these commands and the keymap stays the keyboard's only authority. *Close Tab*
//! is the item that most needed to exist for that reason: ⌘W is *Close Window* on a Mac, so
//! `tab.close` had no working key there and no other gesture in the menu bar.
//!
//! # A click runs the registry command, in one window
//!
//! Rust decides nothing about what an item does. Every command row is a `cide_core::commands` id,
//! and a click asks one shell window to run it (`cide://command-requested`, with no project:
//! "whatever this window is showing") — the same dispatcher the palette, the keyboard and the
//! console's `cide_command_run` reach, so a menu pick behaves exactly like a palette pick,
//! refusals included. One window, not every window: in `PerProject` mode each shell window runs
//! its own dispatcher, and a broadcast would open the New Project wizard once per window.
//! [`target`] picks it.
//!
//! *Open Recent* rows are the exception, because a recent project is not a command: a click asks
//! the window to reopen that path (`cide://recent-project-requested`), which runs the header
//! menu's own `projectMenu.reopen`. Routed through the window rather than opened from here so a
//! folder deleted since the list was drawn fails the way it fails from the header — through
//! `Failures`, as a toast naming the reason — instead of as a line in a log nobody reads.
//!
//! # Open Recent keeps itself current — at open and forget, not at menu-open
//!
//! muda offers no "menu will open" hook, so the submenu cannot be built on demand the way the
//! dock menu is (`dock.rs`). It is rebuilt instead by [`refresh_recent`], from every place the
//! recents file changes: an open (`cmd::project::open_project_here`) and a forget or clear
//! (`cmd::project::project_forget_recent`, which *Clear Menu* calls too). The cost is that a row's
//! enabled/disabled mark is as of the last rebuild, so a folder deleted since then draws live and
//! fails on the click, loudly, for the reason above.
//!
//! # What has been verified, and what has not
//!
//! Everything here except the one call in [`install`] compiles on every host: the model
//! ([`FILE_MENU`], [`action_for`], [`recent_rows`], [`target`]) is unit-tested below, and the
//! tauri glue uses only `tauri::menu`, which is the same API on Linux. What has **never been run**
//! is all of it on a Mac: that the default's first submenu is the app menu with *About* first,
//! that its File submenu is titled `File`, that the inserted rows draw above *Close Window*, and
//! that a click reaches the focused window.

use std::path::PathBuf;
use std::sync::OnceLock;

use cide_ipc::{RecentEntry, WindowLabel, WindowRole};
use parking_lot::Mutex;
use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Wry};

use crate::workspace_state::WorkspaceState;

/// *Check for Updates…*.
const CHECK_FOR_UPDATES: &str = "cide.checkForUpdates";
/// A row that runs a registry command: the prefix, then the command id.
const COMMAND_PREFIX: &str = "cide.cmd:";
/// A row that reopens a recent project: the prefix, then the project's path.
///
/// The path travels **on the id** rather than as an index into a list kept beside the menu —
/// `dock.rs`'s argument: an index is a second piece of state to keep in step with a list that can
/// change between the rebuild and the click, and a stale one reopens the wrong project rather than
/// none. Every path cide records is UTF-8 on macOS (APFS requires it), so the lossy conversion in
/// [`recent_rows`] is lossless where this runs.
const RECENT_PREFIX: &str = "cide.recent:";
/// *Clear Menu*. A dot, not a colon, so it can never be read as a recent path.
const CLEAR_RECENT: &str = "cide.recent.clear";
/// The disabled *No Recent Projects* row. Never clicked; it needs an id all the same.
const NO_RECENTS: &str = "cide.recent.none";
/// The *Open Recent* submenu itself.
const OPEN_RECENT: &str = "cide.openRecent";

/// How many recent projects the submenu lists before *More…*.
///
/// The recents file keeps up to `persist::MAX_RECENT` (a hundred); a menu of a hundred rows is a
/// list to scroll, not to pick from. Fifteen is a little over the ten macOS's own *Open Recent*
/// shows, and *More…* opens the project picker, which searches the whole list.
pub const RECENT_IN_MENU: usize = 15;

/// One row of a menu cide adds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Runs `command` — a `cide_core::commands` id — in the answering window.
    Command {
        command: &'static str,
        label: &'static str,
    },
    Separator,
    /// The *Open Recent* submenu, filled by [`refresh_recent`].
    OpenRecent,
}

const fn command(command: &'static str, label: &'static str) -> Row {
    Row::Command { command, label }
}

/// The File menu, top to bottom, above the default's *Close Window*.
///
/// Labels are macOS title case rather than the palette's sentence case (*Close project*), because
/// that is how every other item in the same bar reads. Every id is checked against the registry by
/// a test below: ids are API and never renamed, but a row naming one that does not exist would
/// click into the dispatcher's fallback and do nothing.
///
/// No enablement. A row whose precondition fails — Close Tab on the pinned console, Save with no
/// editor focused — is the dispatcher's refusal, written to the diagnostic log exactly as it is
/// from the palette. Greying rows live would need the frontend's context flags on this side.
pub const FILE_MENU: &[Row] = &[
    command("project.new", "New Project…"),
    command("scratch.new", "New Scratch File…"),
    Row::Separator,
    command("project.open", "Open Project…"),
    Row::OpenRecent,
    Row::Separator,
    command("file.save", "Save"),
    command("file.saveAll", "Save All"),
    Row::Separator,
    command("tab.close", "Close Tab"),
    command("tab.reopenClosed", "Reopen Closed Tab"),
    command("project.close", "Close Project"),
    Row::Separator,
    command("settings.open", "Settings…"),
    // Ends on a separator: the default's *Close Window* follows, in a group of its own.
    Row::Separator,
];

/// *Settings…* in the application menu. Also the last command row of [`FILE_MENU`] — asked for in
/// both places: the app menu is where a Mac user looks, File is where the request put it.
pub const APP_MENU_SETTINGS: (&str, &str) = ("settings.open", "Settings…");

/// The id a command row carries.
pub fn command_id(command: &str) -> String {
    format!("{COMMAND_PREFIX}{command}")
}

/// What a click on a menu item asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    CheckForUpdates,
    /// Run this registry command in the answering window.
    Command(String),
    /// Reopen the recent project at this path.
    OpenRecent(PathBuf),
    /// Forget every recent project.
    ClearRecent,
}

/// Decode a clicked item's id. `None` for an id cide did not mint — the default menu's own items
/// are predefined and never reach `on_menu_event` with one of these prefixes.
pub fn action_for(id: &str) -> Option<MenuAction> {
    if id == CHECK_FOR_UPDATES {
        return Some(MenuAction::CheckForUpdates);
    }
    if id == CLEAR_RECENT {
        return Some(MenuAction::ClearRecent);
    }
    if let Some(command) = id.strip_prefix(COMMAND_PREFIX) {
        return (!command.is_empty()).then(|| MenuAction::Command(command.to_owned()));
    }
    if let Some(path) = id.strip_prefix(RECENT_PREFIX) {
        return (!path.is_empty()).then(|| MenuAction::OpenRecent(PathBuf::from(path)));
    }
    None
}

/// One row of the *Open Recent* submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecentRow {
    Item {
        id: String,
        label: String,
        enabled: bool,
    },
    Separator,
}

/// The *Open Recent* submenu for this list, which arrives newest first (`project_recent`'s order,
/// never re-sorted here — one authority for the order, as the header menu's model says).
///
/// The header menu's two rules, restated for this surface:
///
/// * **Labelled by the display path**, not the name: two checkouts of one repository share a
///   basename, and a menu with `cide` in it twice cannot be used.
/// * **A missing folder is listed and disabled**, never dropped: a project that silently vanished
///   from the list reads as cide forgetting it.
///
/// Past [`RECENT_IN_MENU`], *More…* opens the project picker. *Clear Menu* is macOS's own word for
/// the last row of every *Open Recent*, and is disabled when there is nothing to clear.
pub fn recent_rows(entries: &[RecentEntry]) -> Vec<RecentRow> {
    let mut rows = Vec::new();
    if entries.is_empty() {
        rows.push(RecentRow::Item {
            id: NO_RECENTS.into(),
            label: "No Recent Projects".into(),
            enabled: false,
        });
    }
    for entry in entries.iter().take(RECENT_IN_MENU) {
        rows.push(RecentRow::Item {
            id: format!("{RECENT_PREFIX}{}", entry.project.path.to_string_lossy()),
            label: entry.project.display_path.clone(),
            enabled: entry.exists,
        });
    }
    if entries.len() > RECENT_IN_MENU {
        rows.push(RecentRow::Item {
            id: command_id("project.open"),
            label: "More…".into(),
            enabled: true,
        });
    }
    rows.push(RecentRow::Separator);
    rows.push(RecentRow::Item {
        id: CLEAR_RECENT.into(),
        label: "Clear Menu".into(),
        enabled: !entries.is_empty(),
    });
    rows
}

/// Which window should answer a click in the menu bar.
///
/// The focused shell window, which is the one the user is looking at when they reach for the
/// menu bar. None focused is possible — the menu bar stays up while a detached pane's window
/// has the focus, and that window has no palette and no dispatcher to hand a command to — so then
/// the first shell window, in the workspace's own order. `None` only when there is no shell
/// window at all, and then there is nowhere to run anything.
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

/// Put cide's items into the menu bar. Called once, from `setup`. A no-op off macOS, where cide
/// has no menu bar.
pub fn install(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if let Err(error) = install_menu(app) {
        // Logged and dropped: the default menu is still there, and the palette, the keymap and
        // the header still reach every one of these commands. Failing `setup` over a menu would
        // be worse.
        tracing::warn!(%error, "could not add cide's items to the menu bar");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// The *Open Recent* submenu, once [`install_menu`] has put it in the File menu. Empty off macOS,
/// which is what makes [`refresh_recent`] free there.
static OPEN_RECENT_MENU: OnceLock<Submenu<Wry>> = OnceLock::new();

/// Serialises [`refresh_recent`]: the read of the list *and* the rewrite of the submenu.
///
/// Both under one lock, because two refreshes racing — an open and a forget a moment apart —
/// would otherwise each read a list and the slower one would draw the older of the two last. And
/// tauri runs each submenu edit on the main thread and waits for it, so two interleaved rewrites
/// would draw both lists' rows into one menu.
static REFRESH: Mutex<()> = Mutex::new(());

/// Redraw *Open Recent* from the recents file as it stands now.
///
/// **Blocking, and never on the main thread.** It reads the file and stats up to
/// [`RECENT_IN_MENU`]-and-more directories — one of which can be an unmounted share, i.e. an NFS
/// timeout — and it holds [`REFRESH`] while tauri hops each submenu edit to the main thread and
/// waits. A caller *on* the main thread would freeze every window for the stats, and one that
/// held the lock there would wait on itself. Every caller is a `spawn_blocking` closure.
///
/// Immediate and free when the submenu was never installed, which is every host but macOS: the
/// two recents call sites run it unconditionally.
pub fn refresh_recent(app: &AppHandle) {
    let Some(submenu) = OPEN_RECENT_MENU.get() else {
        return;
    };
    let _guard = REFRESH.lock();
    let rows = recent_rows(&crate::cmd::project::load_recent_entries());
    if let Err(error) = fill_recent(app, submenu, &rows) {
        tracing::warn!(%error, "could not redraw Open Recent");
    }
}

/// The application menu and the File menu, inserted into `Menu::default`, then set.
///
/// Only called on macOS; compiled everywhere, so the glue type-checks on the platform this is
/// developed on — `cide-app` cannot be checked for Darwin from Linux at all (`docs/platforms.md`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn install_menu(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    let submenus: Vec<Submenu<Wry>> = menu
        .items()?
        .into_iter()
        .filter_map(|item| item.as_submenu().cloned())
        .collect();

    // The first submenu is the application menu, and *About* is its first item.
    match submenus.first() {
        Some(app_menu) => {
            let updates = MenuItem::with_id(
                app,
                CHECK_FOR_UPDATES,
                "Check for Updates…",
                true,
                None::<&str>,
            )?;
            app_menu.insert(&updates, 1)?;
            // A separator of its own and then *Settings…*, right under *Check for Updates…*: the
            // default's separator that followed *About* now follows Settings, so Services keeps
            // its group. Positions counted from *About*, the one item the default promises first.
            let (command, label) = APP_MENU_SETTINGS;
            app_menu.insert(&PredefinedMenuItem::separator(app)?, 2)?;
            app_menu.insert(&command_item(app, command, label)?, 3)?;
        }
        None => tracing::warn!("the default menu has no application submenu; leaving it alone"),
    }

    // Found by its title rather than by position: the default's order differs by platform
    // (`tauri::menu::Menu::default`), and inserting a dozen rows into Edit would be far worse
    // than inserting none.
    let file = submenus
        .iter()
        .find(|submenu| submenu.text().is_ok_and(|text| text == "File"));
    match file {
        // Logged rather than `?`: a File menu that could not be filled must not cost the user the
        // app menu's items above, which are already in.
        Some(file) => {
            if let Err(error) = fill_file(app, file) {
                tracing::warn!(%error, "could not add cide's items to the File menu");
            }
        }
        None => tracing::warn!("the default menu has no File submenu; leaving it alone"),
    }

    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if let Some(action) = action_for(event.id().as_ref()) {
            act(app, action);
        }
    });
    Ok(())
}

/// [`FILE_MENU`]'s rows, at the top of the default File menu, and the first *Open Recent* fill.
fn fill_file(app: &AppHandle, file: &Submenu<Wry>) -> tauri::Result<()> {
    let open_recent = Submenu::with_id(app, OPEN_RECENT, "Open Recent", true)?;
    for (at, row) in FILE_MENU.iter().enumerate() {
        let item = match *row {
            Row::Command { command, label } => {
                MenuItemKind::MenuItem(command_item(app, command, label)?)
            }
            Row::Separator => MenuItemKind::Predefined(PredefinedMenuItem::separator(app)?),
            Row::OpenRecent => MenuItemKind::Submenu(open_recent.clone()),
        };
        file.insert(&item, at)?;
    }
    // Set only once the submenu is in the menu: a refresh into a submenu nothing shows would be
    // work for nothing, and one that never got set is a refresh that is correctly free.
    let _ = OPEN_RECENT_MENU.set(open_recent);
    // Filled off this thread — `refresh_recent` stats, and this is `setup`, on the main thread.
    // Until it lands, a few milliseconds, the submenu is empty, which AppKit draws disabled.
    let handle = app.clone();
    drop(tauri::async_runtime::spawn_blocking(move || {
        refresh_recent(&handle)
    }));
    Ok(())
}

/// A row that runs `command`. No accelerator — see the module header.
fn command_item(app: &AppHandle, command: &str, label: &str) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, command_id(command), label, true, None::<&str>)
}

/// Replace *Open Recent*'s rows with `rows`. Under [`REFRESH`]; see [`refresh_recent`].
fn fill_recent(app: &AppHandle, submenu: &Submenu<Wry>, rows: &[RecentRow]) -> tauri::Result<()> {
    // Built before anything is removed, so a failure to build leaves the old rows up rather than
    // an empty submenu.
    let items = rows
        .iter()
        .map(|row| {
            Ok(match row {
                RecentRow::Item { id, label, enabled } => MenuItemKind::MenuItem(
                    MenuItem::with_id(app, id.as_str(), label, *enabled, None::<&str>)?,
                ),
                RecentRow::Separator => {
                    MenuItemKind::Predefined(PredefinedMenuItem::separator(app)?)
                }
            })
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    for old in submenu.items()? {
        submenu.remove(&old)?;
    }
    for item in &items {
        submenu.append(item)?;
    }
    Ok(())
}

/// Answer a click. On the main thread, where AppKit sends every menu action.
fn act(app: &AppHandle, action: MenuAction) {
    match action {
        MenuAction::CheckForUpdates => in_window(app, |window| {
            crate::emit::update_check_requested(app, window)
        }),
        MenuAction::Command(command) => in_window(app, |window| {
            crate::emit::command_requested(app, window, None, &command, None)
        }),
        MenuAction::OpenRecent(path) => in_window(app, |window| {
            crate::emit::recent_project_requested(app, window, &path)
        }),
        MenuAction::ClearRecent => clear_recent(app),
    }
}

/// Run `ask` against the window that should answer, or log that there is none.
fn in_window(app: &AppHandle, ask: impl FnOnce(&WindowLabel)) {
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return;
    };
    let window = state.with(|ws| target(&ws.windows, crate::windows::is_focused));
    match window {
        Some(window) => ask(&window),
        None => tracing::debug!("a menu bar click with no shell window to answer it"),
    }
}

/// *Clear Menu*: the header's *Clear recent projects*, which also forgets the remembered layouts
/// and redraws this submenu. Off the main thread, because it rewrites two files and stats.
fn clear_recent(app: &AppHandle) {
    let app = app.clone();
    drop(tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::cmd::project::project_forget_recent(app, None).await {
            tracing::warn!(%error, "Clear Menu could not clear the recent projects");
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::RecentProject;

    fn shell() -> WindowRole {
        WindowRole::Shell {
            projects: Vec::new(),
            active: None,
        }
    }

    fn label(s: &str) -> WindowLabel {
        WindowLabel(s.into())
    }

    fn recent(path: &str, exists: bool) -> RecentEntry {
        RecentEntry {
            project: RecentProject {
                path: PathBuf::from(path),
                name: path.rsplit('/').next().unwrap_or(path).into(),
                display_path: path.replace("/home/u", "~"),
                opened_at: 0,
            },
            exists,
        }
    }

    fn item(row: &RecentRow) -> (&str, &str, bool) {
        match row {
            RecentRow::Item { id, label, enabled } => (id, label, *enabled),
            RecentRow::Separator => ("─", "─", false),
        }
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

    #[test]
    fn every_file_menu_row_names_a_registry_command() {
        // Ids are API and never renamed — but a row naming one that does not exist would click
        // into the dispatcher's fallback and do nothing, with no error anywhere a Mac user sees.
        let (settings, _) = APP_MENU_SETTINGS;
        let commands = FILE_MENU
            .iter()
            .filter_map(|row| match row {
                Row::Command { command, .. } => Some(*command),
                _ => None,
            })
            .chain([settings, "project.open"]);
        for command in commands {
            assert!(
                cide_core::commands::by_id(command).is_some(),
                "the menu bar names `{command}`, which the registry does not have"
            );
        }
    }

    #[test]
    fn the_file_menu_asks_for_what_was_asked_for() {
        let commands: Vec<&str> = FILE_MENU
            .iter()
            .filter_map(|row| match row {
                Row::Command { command, .. } => Some(*command),
                _ => None,
            })
            .collect();
        for wanted in [
            "project.new",
            "project.open",
            "project.close",
            "scratch.new",
            "file.save",
            "file.saveAll",
            "tab.close",
            "tab.reopenClosed",
            "settings.open",
        ] {
            assert!(commands.contains(&wanted), "File lost `{wanted}`");
        }
        assert_eq!(
            FILE_MENU
                .iter()
                .filter(|row| **row == Row::OpenRecent)
                .count(),
            1,
            "one Open Recent, which is the one `refresh_recent` fills"
        );
        assert_eq!(
            FILE_MENU.last(),
            Some(&Row::Separator),
            "the default's Close Window follows, and it gets a group of its own"
        );
    }

    #[test]
    fn no_two_file_menu_rows_share_a_command_or_a_label() {
        let mut seen = std::collections::HashSet::new();
        for row in FILE_MENU {
            if let Row::Command { command, label } = row {
                assert!(seen.insert(*command), "`{command}` is in File twice");
                assert!(seen.insert(*label), "two rows read `{label}`");
            }
        }
    }

    #[test]
    fn a_command_row_decodes_back_to_its_command() {
        for row in FILE_MENU {
            if let Row::Command { command, .. } = row {
                assert_eq!(
                    action_for(&command_id(command)),
                    Some(MenuAction::Command((*command).into()))
                );
            }
        }
        assert_eq!(
            action_for(CHECK_FOR_UPDATES),
            Some(MenuAction::CheckForUpdates)
        );
    }

    #[test]
    fn a_recent_row_decodes_back_to_its_path_whatever_the_path_holds() {
        // A colon and a space: the prefix is stripped once and the rest is the path, whatever
        // it contains — splitting on `:` would have cut this one in half.
        let entries = [recent("/Users/u/work/a: b/c d", true)];
        let rows = recent_rows(&entries);
        let (id, _, _) = item(&rows[0]);
        assert_eq!(
            action_for(id),
            Some(MenuAction::OpenRecent(PathBuf::from(
                "/Users/u/work/a: b/c d"
            )))
        );
    }

    #[test]
    fn the_rows_that_are_not_projects_decode_to_what_they_do() {
        assert_eq!(action_for(CLEAR_RECENT), Some(MenuAction::ClearRecent));
        assert_eq!(
            action_for(NO_RECENTS),
            None,
            "the disabled empty-state row must never act, even if AppKit sent it"
        );
        assert_eq!(action_for(OPEN_RECENT), None, "nor the submenu itself");
        assert_eq!(
            action_for(COMMAND_PREFIX),
            None,
            "a prefix with nothing after it"
        );
        assert_eq!(action_for(RECENT_PREFIX), None);
        assert_eq!(action_for("quit"), None, "a predefined item is not cide's");
    }

    #[test]
    fn recent_rows_keep_the_order_they_are_given_and_grey_a_missing_folder() {
        let entries = [
            recent("/home/u/work/new", true),
            recent("/home/u/tmp/gone", false),
            recent("/home/u/work/old", true),
        ];
        let rows = recent_rows(&entries);
        let drawn: Vec<_> = rows.iter().map(item).collect();
        assert_eq!(
            drawn,
            vec![
                ("cide.recent:/home/u/work/new", "~/work/new", true),
                ("cide.recent:/home/u/tmp/gone", "~/tmp/gone", false),
                ("cide.recent:/home/u/work/old", "~/work/old", true),
                ("─", "─", false),
                (CLEAR_RECENT, "Clear Menu", true),
            ],
            "newest first as given, labelled by display path, and a missing folder is listed and \
             disabled rather than dropped — the header menu's rule"
        );
    }

    #[test]
    fn a_long_list_stops_at_the_cap_and_offers_the_picker() {
        let entries: Vec<_> = (0..RECENT_IN_MENU + 5)
            .map(|n| recent(&format!("/home/u/p{n}"), true))
            .collect();
        let rows = recent_rows(&entries);
        let projects = rows
            .iter()
            .filter(|row| item(row).0.starts_with(RECENT_PREFIX))
            .count();
        assert_eq!(projects, RECENT_IN_MENU);
        let more = rows
            .iter()
            .map(item)
            .find(|(_, label, _)| *label == "More…")
            .expect("a capped list offers More…");
        assert_eq!(
            action_for(more.0),
            Some(MenuAction::Command("project.open".into())),
            "More… is the project picker, which searches the whole list"
        );

        let exactly: Vec<_> = entries.into_iter().take(RECENT_IN_MENU).collect();
        assert!(
            !recent_rows(&exactly)
                .iter()
                .any(|row| item(row).1 == "More…"),
            "a list that fits has nothing more to show"
        );
    }

    #[test]
    fn an_empty_list_says_so_and_has_nothing_to_clear() {
        let drawn: Vec<_> = recent_rows(&[])
            .iter()
            .map(item)
            .map(|(_, l, e)| (l.to_owned(), e))
            .collect();
        assert_eq!(
            drawn,
            vec![
                ("No Recent Projects".to_owned(), false),
                ("─".to_owned(), false),
                ("Clear Menu".to_owned(), false),
            ]
        );
    }
}
