//! Application-lifecycle commands.

use cide_core::{commands, keymap};
use cide_ipc::{Bootstrap, Capabilities, WindowLabel, WindowRole};
use tauri::{AppHandle, Manager, State, Window};

use crate::workspace_state::WorkspaceState;

/// Called by the frontend once it has painted its first frame.
///
/// Kept even though windows are now created visible: it is the one place that reports the
/// window's real geometry, and a window that loads its frontend but never gets a layout is
/// otherwise indistinguishable from a working one until you notice every PTY stuck at its
/// fallback 80x24.
#[tauri::command]
pub fn app_ready(app: AppHandle, window: Window) {
    crate::windows::reveal(&app, window.label());

    let label = window.label().to_string();
    match (
        window.is_visible(),
        window.outer_size(),
        window.scale_factor(),
    ) {
        (Ok(visible), Ok(size), Ok(scale)) => {
            eprintln!(
                "[cide] window {label}: visible={visible} outer={}x{} scale={scale}",
                size.width, size.height
            );
        }
        _ => eprintln!("[cide] window {label}: geometry unavailable"),
    }
}

/// Everything a window needs to paint, in one round trip.
///
/// One call rather than four: a window that has to ask separately for its role, the
/// workspace, the keymap and the command list will render three intermediate states on the
/// way, and on the slow IPC path that is visible as flicker.
#[tauri::command]
pub fn app_get_bootstrap(window: Window, state: State<'_, WorkspaceState>) -> Bootstrap {
    let workspace = state.snapshot();
    let label = WindowLabel(window.label().to_string());

    // A window with no stored role is one the user just opened, before any project exists.
    // That is the empty frame with a `+` in its header, not an error.
    let role = workspace
        .windows
        .get(&label)
        .cloned()
        .unwrap_or(WindowRole::Shell {
            projects: Vec::new(),
            active: None,
        });

    Bootstrap {
        window: label,
        role,
        workspace,
        keymap: keymap::resolve(&user_keymap()),
        commands: commands::registry().to_vec(),
        // Read from the workspace this window already holds, so the version in the header is
        // the version of the binary this workspace's panes will actually spawn.
        capabilities: capabilities(&state.with(|ws| ws.settings.claude.cli.binary.clone())),
    }
}

/// The user's keybinding overrides, or none.
///
/// Read here rather than cached in managed state so a new window picks up an edited
/// `keymap.json` without restarting the app — the file is a few hundred bytes and this runs
/// once per window, not once per keystroke.
///
/// **This is the whole of what makes rebinding work.** Until it existed, `app_get_bootstrap`
/// resolved against an empty user layer, so `~/.config/cide/keymap.json` reached neither
/// entry point of the key gate: not after a restart, not at all. The layering, the conflict
/// detector and the Settings → Keymap section were all built against a layer that was never
/// populated.
///
/// A malformed file degrades to the defaults rather than failing the bootstrap. A user who
/// has broken their JSON needs a working app to fix it in, and the alternative is an editor
/// that will not start because of a comma.
fn user_keymap() -> Vec<cide_ipc::Binding> {
    let path = cide_core::persist::keymap_path();
    match keymap::load_user(&path) {
        Ok(bindings) => bindings,
        Err(error) => {
            tracing::error!(
                path = %path.display(),
                %error,
                "could not read keymap.json; running with default bindings only"
            );
            Vec::new()
        }
    }
}

/// Resize the window until its webview viewport is exactly `width` x `height`.
///
/// Needed because a window's size and its viewport are not the same number here. GTK draws
/// client-side decorations with an invisible shadow border around the surface — 26px a side
/// on this desktop — so asking for a 1440x900 window yields a 1492x952 surface and a
/// 1388x848 viewport. The inset is a property of the desktop, not something to hard-code,
/// so the caller reports the viewport it actually got and this corrects by the difference.
///
/// Used by the layout audit, whose whole premise is comparing against a mock stated at a
/// specific size. Measuring 1388x848 and reporting it as 1440x900 would be the kind of
/// quiet inaccuracy the audit exists to catch elsewhere.
#[tauri::command(rename_all = "camelCase")]
pub fn window_set_viewport(
    window: Window,
    width: f64,
    height: f64,
    current_width: f64,
    current_height: f64,
) {
    let Ok(size) = window.inner_size() else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let logical = size.to_logical::<f64>(scale);

    // The inset is whatever the surface carries beyond what the webview can paint.
    let inset_x = logical.width - current_width;
    let inset_y = logical.height - current_height;
    let _ = window.set_size(tauri::LogicalSize::new(width + inset_x, height + inset_y));
}

/// What this build can actually do, so the frontend never offers a dead control.
///
/// `binary` is the user's configured `claude`, which is why this takes an argument at all: the
/// header's version string has to be about the binary panes will actually run, or a user who
/// pinned an old CLI reads the version of one they are not using.
fn capabilities(binary: &str) -> Capabilities {
    Capabilities {
        version: env!("CARGO_PKG_VERSION").to_string(),
        claude_version: claude_version(binary),
        // **This flag's meaning changed in M12, and the change matters.**
        //
        // It used to mean "a language server ships", which was a *runtime* claim wearing a
        // build-time flag: the frontend read `false` and knew not to paint counts. Now this build
        // can genuinely run diagnostics sources, so it is unconditionally `true` — and the
        // runtime truth moved to `SourceReport`, which says per analyser whether it is running,
        // scanning, or absent and why.
        //
        // Leaving this `false` would have been the safe-looking choice and the wrong one: the
        // panel would never ask. Flipping it without the `SourceReport`s would have been worse —
        // the frontend would read `true` as "there are counts to show" and paint a confident
        // `✗ 0` on a machine with no rust-analyzer, which is exactly what the old comment here
        // existed to prevent.
        diagnostics: true,
        // Wired in M6.
        ide_protocol: false,
    }
}

/// `claude --version`, or `None` when the binary is not on `PATH`.
///
/// Recorded because the IDE-integration protocol is unversioned and the CLI self-updates:
/// knowing which version a session was spawned against is the only way to tell a protocol
/// change from a bug in this code.
fn claude_version(binary: &str) -> Option<String> {
    // Not spawned at all when the configured binary cannot be run. This is on the bootstrap
    // path — every window pays for it — and forking something already known to be missing spends
    // a `fork`/`exec` to learn what `resolve` answered from a `stat`. The screen has the
    // sentence; this field is only the string.
    cide_core::claude_cli::resolve(binary).ok()?;
    let mut command = std::process::Command::new(binary);
    // The CLI is node, and a bundled launch would otherwise hand it this AppImage's
    // `LD_LIBRARY_PATH`. See `cide_core::child_env`.
    cide_core::child_env::prepare_command(&mut command);
    let output = command.arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// What closing would cost: unsaved buffers, and interrupted Claude turns.
///
/// Answers for the whole app when `project` is `None`, and for one project otherwise. The
/// caller asks before it closes anything and confirms only when the answer is non-empty —
/// see `QuitDecision::is_clear`.
///
/// # The two halves are governed differently, and deliberately so
///
/// **Unsaved file tabs** are always reported. This is destruction: the edits exist in no
/// other place, and a quit takes them with it. `Settings::confirm_close_with_live_session`
/// does not suppress them, because that setting is about *sessions* — its label in the
/// Settings tab reads "Confirm before closing a project with a live session" — and a user
/// who turns it off has said something about agent turns, not about their own typing.
/// Reading it as authority over unsaved buffers would let one toggle, worded about
/// something else, silently disable the only guard against losing work.
///
/// **Live sessions** are reported only when that setting is on. "Live" is
/// `Busy | AwaitingPermission`, never "a process exists" — see `cide_claude::state`. A
/// session sitting at a prompt has nothing to lose by being closed, and warning about it
/// would train people to dismiss the dialog. Even a busy one loses only its turn: the
/// conversation is keyed by `SessionId` and resumes with `claude --resume`. That is
/// recoverable, so it is reasonable for a user to switch the warning off, and the setting
/// defaults off because that is what the design mock's toggle shows.
///
/// With no hook server running, the session half is empty rather than pessimistic: without
/// hooks there is no evidence any session is busy, and inventing one would put a dialog in
/// front of every close for the life of that run. The unsaved half is unaffected — it is
/// read from the workspace tree, which is always there.
///
/// # This is advice, not enforcement
///
/// `tab_close`, `project_close` and `window_close` refuse unsaved work themselves and have
/// to be told `force`, so every close that arrives as a *command* is enforced.
///
/// A close that does not arrive as a command is not. The window manager's own close on a
/// shell window — Alt+F4, the compositor's button — goes to `WindowEvent::CloseRequested`,
/// which `cmd::window::intercept_close` deliberately lets through for shells, and from there
/// to `RunEvent::ExitRequested` and `lifecycle::shutdown`. No domain operation runs on that
/// path and there is nothing to refuse. Vetoing it would mean `prevent_close` plus a new
/// event asking the frontend for an answer, and a veto whose answer never comes is an app
/// that cannot be quit — so the honest state is that this half is advisory, and the mitigation
/// is that the editor writes through `file_write` on save rather than holding a buffer Rust
/// cannot see.
#[tauri::command(rename_all = "camelCase")]
pub fn app_quit_requested(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: Option<cide_ipc::ProjectId>,
) -> cide_ipc::QuitDecision {
    let unsaved = state.with(|ws| cide_core::workspace::unsaved_tabs(ws, project));

    let blocking = live_sessions(&app, &state, project);
    cide_ipc::QuitDecision { blocking, unsaved }
}

/// The `Busy | AwaitingPermission` sessions a close would interrupt.
///
/// Empty when the user has turned `confirm_close_with_live_session` off, or when no hook
/// server is running. Split out of [`app_quit_requested`] so that the setting is consulted
/// in exactly one place and the unsaved-tab half cannot accidentally inherit it.
fn live_sessions(
    app: &tauri::AppHandle,
    state: &State<'_, WorkspaceState>,
    project: Option<cide_ipc::ProjectId>,
) -> Vec<cide_ipc::SessionSummary> {
    if !state.with(|ws| ws.settings.confirm_close_with_live_session) {
        return vec![];
    }
    let Some(hooks) = app.try_state::<crate::hooks::HookServer>() else {
        return vec![];
    };

    let live: std::collections::HashSet<cide_ipc::SessionId> =
        hooks.live_sessions().into_iter().collect();
    if live.is_empty() {
        return vec![];
    }

    state.with(|ws| {
        let mut out = Vec::new();
        for (id, p) in &ws.projects {
            if project.is_some_and(|only| only != *id) {
                continue;
            }
            for tab in &p.tabs {
                for pane in tab.tree.panes.values() {
                    let Some(session) = pane.session else {
                        continue;
                    };
                    if !live.contains(&session) {
                        continue;
                    }
                    out.push(cide_ipc::SessionSummary {
                        session,
                        project: *id,
                        project_name: p.name.clone(),
                        pane_title: pane.title.clone(),
                        state: hooks.state(session),
                    });
                }
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialises every test in this binary that touches the process environment.
    ///
    /// Not a tidiness measure. `std::env::set_var` mutates a process-global that glibc may
    /// reallocate, and *any* concurrent environment access — including the environ walk
    /// `Command::spawn` does — can read through the freed array. That is undefined
    /// behaviour, which is why edition 2024 made `set_var` unsafe, and it is why this is a
    /// lock rather than two tests being careful. Reviewers of two separate agents watched
    /// these two flake against each other before it existed.
    ///
    /// `crates/cide-ide-mcp/src/lockfile.rs` carries the same lock for `HOME`, and for the
    /// same reason. A lock per binary is the most that can be had: a *different* test binary
    /// runs in a different process and shares nothing, so no test may ever assume the
    /// environment is its own beyond this guard.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A temporary `XDG_CONFIG_HOME`, restored when this drops.
    ///
    /// RAII rather than a restore at the end of the test, because a failing assertion
    /// unwinds — and leaving the variable pointing at a deleted temp directory would make
    /// every later test in this binary read a config that is not there.
    struct TempConfig {
        dir: std::path::PathBuf,
        previous: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TempConfig {
        fn new(tag: &str) -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("cide-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            // `persist::config_dir` appends `cide` to `XDG_CONFIG_HOME`, so the file this
            // writes lives one level below the directory handed over.
            std::fs::create_dir_all(dir.join("cide")).expect("temp config dir");
            let previous = std::env::var_os("XDG_CONFIG_HOME");
            // SAFETY: `ENV_LOCK` is held for the lifetime of this guard, and it is the only
            // thing in this binary that writes the environment.
            unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
            Self {
                dir,
                previous,
                _guard: guard,
            }
        }

        fn write_keymap(&self, contents: &str) {
            std::fs::write(self.dir.join("cide").join("keymap.json"), contents).expect("write");
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            // SAFETY: as above — the lock is still held until this struct is fully dropped.
            match self.previous.take() {
                Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
                None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The regression guard for the bug this function was written to fix.
    ///
    /// `app_get_bootstrap` used to resolve against an empty user layer, so a rebind in
    /// `keymap.json` reached neither entry point of the key gate. Every piece of the
    /// machinery — the layering, the conflict detector, the Settings section — was built
    /// and tested against a layer nothing ever filled, which is why nothing caught it.
    ///
    /// This asserts the file is *read*: the narrow fact that no other test could see.
    #[test]
    fn a_user_binding_in_keymap_json_reaches_the_resolved_keymap() {
        let config = TempConfig::new("keymap");
        config.write_keymap(r#"[{"key":"ctrl+alt+z","command":"workbench.showFilePicker"}]"#);

        let user = user_keymap();
        let resolved = keymap::resolve(&user);

        assert_eq!(user.len(), 1, "keymap.json was not read at all");
        assert!(
            resolved
                .iter()
                .any(|b| b.key == "ctrl+alt+z" && b.command == "workbench.showFilePicker"),
            "the user's override did not survive resolution; resolved {} bindings",
            resolved.len()
        );
    }

    #[test]
    fn a_broken_keymap_json_costs_the_overrides_and_not_the_app() {
        // A user who has broken their JSON needs a working editor to fix it in. Failing the
        // bootstrap would mean an app that will not start because of a trailing comma.
        let config = TempConfig::new("keymap-bad");
        config.write_keymap("{ not json");

        let user = user_keymap();
        assert!(
            user.is_empty(),
            "a malformed file must degrade, not propagate"
        );
        assert!(
            !keymap::resolve(&user).is_empty(),
            "the defaults must still be there"
        );
    }
}
