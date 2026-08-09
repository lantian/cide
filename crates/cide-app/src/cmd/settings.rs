//! Settings, the keymap report, the graphics ladder, and the headless Claude lane.
//!
//! # Where settings live, and why there is no `cide://settings-changed`
//!
//! Settings are a field of the workspace, so every write goes through
//! [`WorkspaceState::update`] — which validates, marks `workspace.json` dirty and broadcasts
//! `cide://workspace-changed` to every window. A dedicated event would carry the same
//! information a few microseconds earlier and give two windows two orderings to reconcile
//! instead of one. The plan lists a `settings-changed` event; the tree it would announce is
//! already announced, so it is not implemented rather than duplicated.
//!
//! # The graphics ladder is not a runtime setting and this module does not pretend otherwise
//!
//! `WEBKIT_DISABLE_*` are read while the webview is being created. Setting one afterwards
//! does nothing whatsoever — no error, no warning, no effect — so the screen reports what
//! this process actually has and marks the difference as needing a restart. See
//! [`apply_graphics_overrides`] for the launch-time half, which is called from `main` before
//! `graphics::apply` and before anything touches GTK.

use std::path::PathBuf;

use cide_core::keymap::{self, KeymapDiagnostic};
use cide_core::{CoreError, persist, workspace};
use cide_ipc::{
    GraphicsRung, GraphicsSettings, GraphicsStatus, HeadlessError, HeadlessRequest, HeadlessResult,
    KeymapConflict, KeymapProblem, KeymapReport, Pane, PaneId, PaneKind, PaneRole, ProjectId,
    Settings, SettingsPatch, SettingsSection, TabId, TabKind,
};
use tauri::State;

use crate::workspace_state::WorkspaceState;

// --- stored settings ---------------------------------------------------------------------

/// The settings this workspace currently holds.
///
/// `app.get_bootstrap` already carries them inside the workspace; this exists for the
/// screen's own refresh and for a caller that wants them without a tree.
#[tauri::command(rename_all = "camelCase")]
pub fn settings_get(state: State<'_, WorkspaceState>) -> Settings {
    state.with(|ws| ws.settings.clone())
}

/// Apply a patch and return the settings as they now stand.
///
/// Returns the whole struct rather than a revision: the caller is a form, and a form that
/// has to re-fetch to learn what it just did will render the old value for one frame.
#[tauri::command(rename_all = "camelCase")]
pub fn settings_set(
    state: State<'_, WorkspaceState>,
    patch: SettingsPatch,
) -> Result<Settings, CoreError> {
    state.update(|ws| {
        apply_patch(&mut ws.settings, patch);
        // Settings are not part of any structural invariant, but they are part of the tree,
        // and every consumer of `cide://workspace-changed` compares revisions to decide
        // whether a snapshot is news. A settings write that did not bump would be dropped by
        // a window that had already seen this revision.
        workspace::bump(ws);
        Ok(())
    })?;
    Ok(state.with(|ws| ws.settings.clone()))
}

/// Fold a patch into stored settings. `None` leaves a field alone.
///
/// Split out from the command so it can be tested without a Tauri state, which is the whole
/// reason the domain crates exist.
fn apply_patch(settings: &mut Settings, patch: SettingsPatch) {
    let SettingsPatch {
        theme,
        each_project_keeps_claude_tab,
        reopen_last_project,
        keep_sessions_on_window_close,
        confirm_close_with_live_session,
        editor,
        terminal,
        graphics,
        claude,
    } = patch;

    // Destructured rather than field-by-field on purpose: adding a field to `SettingsPatch`
    // and forgetting to apply it here is a toggle that moves on screen, saves nothing, and
    // springs back on the next snapshot. This way the compiler names the omission.
    if let Some(v) = theme {
        settings.theme = v;
    }
    if let Some(v) = each_project_keeps_claude_tab {
        settings.each_project_keeps_claude_tab = v;
    }
    if let Some(v) = reopen_last_project {
        settings.reopen_last_project = v;
    }
    if let Some(v) = keep_sessions_on_window_close {
        settings.keep_sessions_on_window_close = v;
    }
    if let Some(v) = confirm_close_with_live_session {
        settings.confirm_close_with_live_session = v;
    }
    if let Some(v) = editor {
        settings.editor = v;
    }
    if let Some(v) = terminal {
        settings.terminal = v;
    }
    if let Some(v) = graphics {
        settings.graphics = v;
    }
    if let Some(v) = claude {
        settings.claude = v;
    }
}

// --- the settings tab --------------------------------------------------------------------

/// Open the project's Settings tab, or activate and re-point the one it already has.
///
/// Settings is a singleton per project. Opening a second one would give a project two tabs
/// that disagree about which section is showing, and closing "the" settings tab would leave
/// another behind.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_settings(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    section: Option<SettingsSection>,
) -> Result<TabId, CoreError> {
    state.update(|ws| {
        let section = section.unwrap_or_default();
        let p = workspace::project(ws, project)?;
        let existing = p
            .tabs
            .iter()
            .find(|t| matches!(t.kind, TabKind::Settings { .. }))
            .map(|t| t.id);

        if let Some(id) = existing {
            let p = workspace::project_mut(ws, project)?;
            if let Some(tab) = p.tabs.iter_mut().find(|t| t.id == id) {
                tab.kind = TabKind::Settings { section };
            }
            workspace::activate_tab(ws, project, id)?;
            // `activate_tab` only bumps when the active tab actually changed, and re-pointing
            // the section of an already-active tab is a change the frontend has to be told
            // about.
            workspace::bump(ws);
            return Ok(id);
        }

        let id = workspace::open_tab(
            ws,
            project,
            TabKind::Settings { section },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                role: PaneRole::Auxiliary,
                // No process. The tree exists because every tab has one — the invariant is
                // what makes "promote pane to tab" and "split editor" one code path — and
                // the settings screen simply renders over it.
                session: None,
                title: "settings".into(),
            },
        )?;
        Ok(id)
    })
}

// --- keymap ------------------------------------------------------------------------------

/// The resolved keymap, the keystrokes more than one command answers to, and anything in the
/// user's file that did not do what it looks like it does.
///
/// The conflict detection itself is `cide_core::keymap::conflicts` — a pure function over
/// resolved bindings, tested there. This handler is the wire adapter and nothing more.
#[tauri::command(rename_all = "camelCase")]
pub fn keymap_report() -> KeymapReport {
    let path = persist::keymap_path();
    // A missing or unreadable overrides file is the default state of a fresh install, not an
    // error: resolution proceeds on the compiled-in layers so the screen still shows every
    // default binding. A parse failure is reported as a problem against the file itself
    // rather than swallowed, because "my keymap.json does nothing" with no visible cause is
    // exactly the failure this screen exists to end.
    let (user, parse_problem) = match keymap::load_user(&path) {
        Ok(user) => (user, None),
        Err(error) => (
            Vec::new(),
            Some(KeymapProblem {
                key: String::new(),
                command: String::new(),
                message: format!("{} could not be read: {error}", path.display()),
            }),
        ),
    };

    let (bindings, diagnostics) = keymap::resolve_with_diagnostics(&user);
    let conflicts = keymap::conflicts(&bindings)
        .into_iter()
        .map(|c| KeymapConflict {
            key: c.key,
            commands: c.commands,
            when: c.when,
        })
        .collect();

    let mut problems: Vec<KeymapProblem> = parse_problem.into_iter().collect();
    problems.extend(diagnostics.into_iter().map(problem_of));

    KeymapReport {
        bindings,
        conflicts,
        problems,
        path: path.display().to_string(),
    }
}

/// Flatten a diagnostic into the line the screen renders.
fn problem_of(diagnostic: KeymapDiagnostic) -> KeymapProblem {
    match diagnostic {
        KeymapDiagnostic::UnparseableKey {
            key,
            command,
            reason,
        } => KeymapProblem {
            key,
            command,
            message: format!("this key cannot be typed: {reason}"),
        },
        KeymapDiagnostic::RemovalMatchedNothing { key, command } => KeymapProblem {
            key,
            command,
            // The sharp edge of VS Code's removal semantics, and worth spelling out: a
            // removal matches on key *and* command *and* `when`, so a `-command` entry that
            // omits a `when` the original binding carries silently removes nothing.
            message: "this `-command` entry matched no binding — check that its `when` \
                      clause matches the binding you meant to remove"
                .into(),
        },
    }
}

// --- the graphics ladder -----------------------------------------------------------------

/// One rung: the variable, what it costs, and how to read the setting that drives it.
struct Rung {
    variable: &'static str,
    label: &'static str,
    cost: &'static str,
    get: fn(&GraphicsSettings) -> Option<bool>,
}

/// The ladder, cheapest first. Each rung is more aggressive than the last.
///
/// Ordered by what it costs the user, not by how often it helps — the screen is a ladder to
/// walk down when the app will not paint, and walking it in this order means never paying
/// for compositing being off if explicit sync was the problem.
const LADDER: [Rung; 3] = [
    Rung {
        variable: "__NV_DISABLE_EXPLICIT_SYNC",
        label: "Disable NVIDIA explicit sync",
        cost: "Free on other hardware. A frequent cause of hangs on the proprietary driver.",
        get: |g| g.disable_nvidia_explicit_sync,
    },
    Rung {
        variable: "WEBKIT_DISABLE_DMABUF_RENDERER",
        label: "Disable the DMABUF renderer",
        cost: "A little compositing throughput. Applied automatically on Linux: without it \
               this app exits with a Wayland protocol error on a stock KDE desktop.",
        get: |g| g.disable_dmabuf_renderer,
    },
    Rung {
        variable: "WEBKIT_DISABLE_COMPOSITING_MODE",
        label: "Disable accelerated compositing",
        cost: "Turns off accelerated compositing for the whole webview. A four-terminal \
               grid feels it immediately. Last resort.",
        get: |g| g.disable_compositing_mode,
    },
];

/// Set by the launcher to disable every workaround, for bisecting a rendering bug.
const SUPPRESS: &str = "CIDE_NO_GRAPHICS_WORKAROUNDS";

/// Whether the environment cide was *launched from* set [`SUPPRESS`].
///
/// Latched on first call, and the first call is [`apply_graphics_overrides`] at the top of
/// `main` — deliberately, because that function sets [`SUPPRESS`] itself when the user has
/// taken the ladder off automatic. After it runs, the live environment can no longer tell the
/// user's own `CIDE_NO_GRAPHICS_WORKAROUNDS=1` from ours, and the screen draws a different and
/// contradictory banner for each ("nothing below applies" versus "only what you chose
/// applies"). Reading it once, first, is the only way to keep the two distinguishable.
fn launch_suppressed() -> bool {
    static LATCHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LATCHED.get_or_init(|| std::env::var_os(SUPPRESS).is_some())
}

/// What the ladder asked for and what this process actually got.
#[tauri::command(rename_all = "camelCase")]
pub fn graphics_status(state: State<'_, WorkspaceState>) -> GraphicsStatus {
    let settings = state.with(|ws| ws.settings.graphics);
    GraphicsStatus {
        rungs: LADDER
            .iter()
            .map(|rung| GraphicsRung {
                variable: rung.variable.to_string(),
                label: rung.label.to_string(),
                cost: rung.cost.to_string(),
                setting: (rung.get)(&settings),
                // Read from the live environment rather than inferred. That is the only
                // honest answer to "is this on", because the launcher's own heuristic, the
                // user's shell and this setting can all have had a say.
                active: std::env::var_os(rung.variable).is_some(),
            })
            .collect(),
        automatic: !manual(&settings),
        // Not `env::var_os(SUPPRESS)`: `apply_graphics_overrides` sets that same variable in
        // this process whenever the user has set any rung explicitly, so reading it live would
        // report "your environment disabled everything" to every user who touched the ladder —
        // which is both false and the opposite of what actually happened to their choices.
        suppressed_by_env: launch_suppressed(),
    }
}

/// Whether the user has taken the ladder off automatic by setting any rung explicitly.
fn manual(settings: &GraphicsSettings) -> bool {
    LADDER.iter().any(|rung| (rung.get)(settings).is_some())
}

/// Put the stored graphics choices into this process's environment.
///
/// **Must be called from `main` before `graphics::apply` and before anything touches GTK.**
/// These variables are read as the webview is created; after that, setting one has no effect
/// of any kind.
///
/// The composition with `graphics::apply` is the interesting part. That function applies its
/// defaults with set-if-unset semantics, so there is no value this side could write that
/// means "and do not apply the default either" — which would leave a rung the user turned
/// *off* still on. So an explicit choice on **any** rung stands the whole automatic ladder
/// down (via the launcher's own `CIDE_NO_GRAPHICS_WORKAROUNDS` escape hatch) and the stored
/// settings become the entire ladder. The alternative — writing `VAR=0` and hoping WebKit
/// reads it as "off" — is a guess about another project's parsing that would fail silently
/// in exactly the case the user is trying to fix. `GraphicsStatus::automatic` exists so the
/// screen can say this out loud rather than leaving it to be discovered.
///
/// A variable already present in the environment always wins, matching `graphics::apply`: a
/// user who exported one knows something we do not. `CIDE_NO_GRAPHICS_WORKAROUNDS` in the
/// launching environment is the strongest form of that and short-circuits everything here —
/// it is the "give me stock behaviour" switch, and stock cannot mean "plus whatever
/// `workspace.json` happens to say".
///
/// Returns what it set, for the launcher's log.
pub fn apply_graphics_overrides() -> Vec<(&'static str, &'static str)> {
    // `launch_suppressed` before `set_if_unset`, always: this is the call that latches it, and
    // the loop below is what would otherwise poison the reading.
    let suppressed = launch_suppressed();
    overrides_for(suppressed, &stored_graphics())
        .into_iter()
        .filter(|key| set_if_unset(key, "1"))
        .map(|key| (key, "1"))
        .collect()
}

/// Which variables the stored ladder wants set, in ladder order.
///
/// Pure, so the decision is testable; [`apply_graphics_overrides`] is only the shell that puts
/// the answer into the process environment.
fn overrides_for(launch_suppressed: bool, settings: &GraphicsSettings) -> Vec<&'static str> {
    // The escape hatch means *stock* behaviour, and stock includes not applying what is
    // stored. Somebody bisecting a rendering bug with `CIDE_NO_GRAPHICS_WORKAROUNDS=1` must
    // not be handed rungs out of a `workspace.json` they had no reason to look at — that is
    // the one variable whose whole job is "give me the unmodified environment".
    if launch_suppressed || !manual(settings) {
        return Vec::new();
    }

    // `SUPPRESS` first, because it is what stands `graphics::apply`'s automatic ladder down;
    // the rungs after it are then the entire ladder.
    let mut wanted = vec![SUPPRESS];
    wanted.extend(
        LADDER
            .iter()
            .filter(|rung| (rung.get)(settings) == Some(true))
            .map(|rung| rung.variable),
    );
    wanted
}

/// The graphics settings on disk.
///
/// Read straight from `workspace.json` rather than through `WorkspaceState`, because this
/// runs before the Tauri application — and therefore before managed state — exists.
fn stored_graphics() -> GraphicsSettings {
    persist::load(&persist::workspace_path()).settings.graphics
}

/// Set an environment variable only if it has no value yet.
fn set_if_unset(key: &'static str, value: &'static str) -> bool {
    if std::env::var_os(key).is_some() {
        return false;
    }
    // SAFETY: called from `main` before any thread is spawned. Rust 2024 marks `set_var`
    // unsafe because a concurrent `getenv` on another thread is a data race; there is no
    // other thread at this point. This mirrors `graphics::set_if_unset`, which runs
    // immediately after it for the same reason.
    unsafe { std::env::set_var(key, value) };
    true
}

// --- the headless lane -------------------------------------------------------------------

/// Run one non-interactive prompt against the project's root and return the answer.
///
/// `async` so Tauri runs it off the main thread: a sync command blocks the GTK loop, and this
/// one waits on a model turn. The blocking work then goes to `spawn_blocking` because
/// `cide_claude::headless::run` is deliberately synchronous — it owns three pipe-draining
/// threads and a poll loop, and none of that wants an async runtime.
#[tauri::command(rename_all = "camelCase")]
pub async fn claude_headless(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    request: HeadlessRequest,
) -> Result<HeadlessResult, HeadlessError> {
    let cwd: Option<PathBuf> = state.with(|ws| {
        workspace::project(ws, project)
            .ok()
            .and_then(|p| p.roots.first().map(|r| r.path.clone()))
    });
    let Some(cwd) = cwd else {
        return Err(HeadlessError::NoProject {
            project: project.to_string(),
        });
    };

    // The bare name, resolved by `PATH` — the same thing every pane spawns. Resolving it
    // ourselves would pin whichever version was on `PATH` at launch, and the CLI updates
    // itself underneath a running app.
    let run = cide_claude::Headless::new(request, cwd);
    tauri::async_runtime::spawn_blocking(move || {
        cide_claude::headless::run(std::path::Path::new("claude"), &run)
    })
    .await
    .map_err(|e| HeadlessError::NotInstalled {
        detail: format!("the headless worker did not finish: {e}"),
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{Theme, WindowMode};

    #[test]
    fn a_patch_touches_only_the_fields_it_names() {
        let mut settings = Settings::default();
        settings.terminal.font_size = 17;

        apply_patch(
            &mut settings,
            SettingsPatch {
                theme: Some(Theme::Light),
                ..SettingsPatch::default()
            },
        );

        assert_eq!(settings.theme, Theme::Light);
        assert_eq!(settings.terminal.font_size, 17, "an untouched group moved");
        assert!(settings.reopen_last_project, "an untouched toggle moved");
    }

    #[test]
    fn a_false_in_a_patch_is_not_the_same_as_an_absent_field() {
        // The reason a patch is not a `Settings` with holes in it.
        let mut settings = Settings::default();
        assert!(settings.reopen_last_project);
        apply_patch(
            &mut settings,
            SettingsPatch {
                reopen_last_project: Some(false),
                ..SettingsPatch::default()
            },
        );
        assert!(!settings.reopen_last_project);
    }

    #[test]
    fn the_window_mode_is_not_reachable_through_a_patch() {
        // It moves windows, so it belongs to `window.set_mode`. Asserted rather than
        // documented alone: adding the field back here would give the desktop and the
        // workspace two ways to disagree.
        let mut settings = Settings {
            window_mode: WindowMode::PerProject,
            ..Settings::default()
        };
        apply_patch(&mut settings, SettingsPatch::default());
        assert_eq!(settings.window_mode, WindowMode::PerProject);
    }

    #[test]
    fn the_ladder_is_ordered_cheapest_first() {
        let variables: Vec<&str> = LADDER.iter().map(|r| r.variable).collect();
        assert_eq!(
            variables,
            [
                "__NV_DISABLE_EXPLICIT_SYNC",
                "WEBKIT_DISABLE_DMABUF_RENDERER",
                "WEBKIT_DISABLE_COMPOSITING_MODE",
            ]
        );
    }

    #[test]
    fn every_rung_reads_a_distinct_setting() {
        // A copy-paste in `LADDER` would give two rungs one switch: flipping either would
        // move both, and the screen would look like it had lost a control.
        for (i, rung) in LADDER.iter().enumerate() {
            let mut settings = GraphicsSettings::default();
            match i {
                0 => settings.disable_nvidia_explicit_sync = Some(true),
                1 => settings.disable_dmabuf_renderer = Some(true),
                _ => settings.disable_compositing_mode = Some(true),
            }
            for (j, other) in LADDER.iter().enumerate() {
                assert_eq!(
                    (other.get)(&settings),
                    (i == j).then_some(true),
                    "rung {} reads the setting rung {i} owns",
                    other.variable
                );
            }
            assert!((rung.get)(&settings) == Some(true));
        }
    }

    #[test]
    fn an_all_default_ladder_stays_automatic() {
        assert!(!manual(&GraphicsSettings::default()));
    }

    #[test]
    fn turning_any_rung_off_takes_the_ladder_off_automatic() {
        // The one non-obvious rule in this module: an explicit `false` has to disable the
        // launcher's heuristic, because set-if-unset defaults cannot be voted down.
        let settings = GraphicsSettings {
            disable_dmabuf_renderer: Some(false),
            ..GraphicsSettings::default()
        };
        assert!(manual(&settings));
    }

    #[test]
    fn an_automatic_ladder_overrides_nothing() {
        // Nothing set here means `graphics::apply`'s heuristic is left in charge, and setting
        // SUPPRESS would silently disable it.
        assert!(overrides_for(false, &GraphicsSettings::default()).is_empty());
    }

    #[test]
    fn one_explicit_rung_stands_the_automatic_ladder_down_and_becomes_the_whole_ladder() {
        let settings = GraphicsSettings {
            disable_compositing_mode: Some(true),
            ..GraphicsSettings::default()
        };
        assert_eq!(
            overrides_for(false, &settings),
            [SUPPRESS, "WEBKIT_DISABLE_COMPOSITING_MODE"],
            "an explicit choice has to disable the heuristic, or a rung turned off would \
             still be applied by it"
        );
    }

    #[test]
    fn a_ladder_turned_entirely_off_still_suppresses_the_heuristic() {
        // The case the whole design exists for: the user says "no workarounds" and gets none,
        // rather than getting the launcher's set-if-unset defaults anyway.
        let settings = GraphicsSettings {
            disable_dmabuf_renderer: Some(false),
            ..GraphicsSettings::default()
        };
        assert_eq!(overrides_for(false, &settings), [SUPPRESS]);
    }

    #[test]
    fn the_environment_escape_hatch_beats_the_stored_ladder() {
        // `CIDE_NO_GRAPHICS_WORKAROUNDS=1` is documented — in `graphics.rs`, on
        // `GraphicsStatus::suppressed_by_env` and on the screen — as disabling the lot
        // regardless of what is stored. Applying stored rungs under it would make bisecting a
        // rendering bug depend on a file the person bisecting never opened.
        let settings = GraphicsSettings {
            disable_compositing_mode: Some(true),
            disable_nvidia_explicit_sync: Some(true),
            ..GraphicsSettings::default()
        };
        assert!(overrides_for(true, &settings).is_empty());
    }

    #[test]
    fn a_removal_that_matched_nothing_explains_the_when_clause_trap() {
        let problem = problem_of(KeymapDiagnostic::RemovalMatchedNothing {
            key: "ctrl+w".into(),
            command: "tab.close".into(),
        });
        assert_eq!(problem.key, "ctrl+w");
        assert!(problem.message.contains("when"), "{}", problem.message);
    }

    #[test]
    fn the_compiled_in_keymap_has_no_conflicts() {
        // The defaults ship in the binary, so a collision between two of them is a bug in
        // this repository rather than something the user can fix — and the screen that would
        // have reported it is the one the user never opens because everything looks fine.
        //
        // Resolved with no user layer rather than through `keymap_report`, which reads the
        // developer's own `~/.config/cide/keymap.json` and would make this test's result
        // depend on whose machine it ran on.
        let resolved = keymap::resolve(&[]);
        let conflicts = keymap::conflicts(&resolved);
        assert!(
            conflicts.is_empty(),
            "the compiled-in keymap conflicts with itself: {conflicts:?}"
        );
        assert!(!resolved.is_empty());
    }
}
