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

use std::path::{Path, PathBuf};

use cide_core::keymap::{self, KeymapDiagnostic};
use cide_core::{CoreError, persist, workspace};
use cide_ipc::git::DiffSide;
use cide_ipc::{
    GraphicsRung, GraphicsSettings, GraphicsStatus, HeadlessError, HeadlessRequest, HeadlessResult,
    KeymapConflict, KeymapEdit, KeymapEditResult, KeymapProblem, KeymapReport, Pane, PaneId,
    PaneKind, PaneRole, ProjectId, RepoId, Settings, SettingsPatch, SettingsSection, TabId,
    TabKind,
};
use tauri::{AppHandle, Manager, State};

use crate::emit;
use crate::windows;
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
    app: AppHandle,
    patch: SettingsPatch,
) -> Result<Settings, CoreError> {
    // Read before the patch, so the comparisons below are against what the windows are
    // actually wearing rather than against the value we just wrote.
    let (
        was_theme,
        was_explorer,
        was_binaries,
        was_working_set,
        was_memory_limit,
        was_cli,
        was_job_notify,
    ) = state.with(|ws| {
        (
            ws.settings.theme,
            ws.settings.explorer,
            ws.settings.inspections.server_binaries.clone(),
            ws.settings.inspections.server_index_working_set_pct,
            ws.settings.inspections.server_memory_limit_mb,
            ws.settings.claude.cli.binary.clone(),
            ws.settings.terminal.job_notify_after_secs,
        )
    });
    state.update(|ws| {
        apply_patch(&mut ws.settings, patch);
        // Settings are not part of any structural invariant, but they are part of the tree,
        // and every consumer of `cide://workspace-changed` compares revisions to decide
        // whether a snapshot is news. A settings write that did not bump would be dropped by
        // a window that had already seen this revision.
        workspace::bump(ws);
        Ok(())
    })?;
    let settings = state.with(|ws| ws.settings.clone());
    // The webview repaints itself from `data-theme`; the *native* fill behind and around it
    // does not, and it is what shows through while a window is being dragged or resized.
    // Outside the `update` closure deliberately: `windows::apply_theme` walks the window
    // list, and the workspace lock is `parking_lot` and not reentrant.
    if settings.theme != was_theme {
        windows::apply_theme(&app, settings.theme);
    }
    // The file tree's two visibility toggles are the only settings whose value is not enough:
    // the rows they ask for were never walked, so the index has to be built again. See
    // `reindex_open_projects`.
    if settings.explorer != was_explorer {
        reindex_open_projects(&app, crate::files::visibility_of(&settings));
    }
    // A server whose Builtin/System choice moved is running the wrong binary right now, in
    // every project that has it — the choice is global. Restarting is the honest reaction and
    // the expensive one (a rust-analyzer restart is a re-index), which is why it happens only
    // for servers whose *effective* choice changed: writing `Builtin` into a map where absence
    // already meant Builtin restarts nothing.
    for binary in rebinaried(&was_binaries, &settings.inspections.server_binaries) {
        if let Some(registry) = app.try_state::<crate::lsp::DiagnosticsRegistry>() {
            registry.restart_everywhere(&app, cide_ipc::DiagnosticSourceId::for_server(&binary));
        }
    }
    // The working-set percentage is baked into the handshake (`initializationOptions`), so a
    // running server keeps yesterday's caps until it restarts — the same shape as a binary
    // choice, and the same honest-but-expensive reaction. rust-analyzer alone, by name: the
    // tuning only ever reaches the shipped rust-analyzer (see `cide_lsp::config`), and
    // restarting gopls over a knob it never sees would be pure cost. With the disk index this
    // restart is a warm load, which is what makes a settings row acceptable at all.
    if was_working_set != settings.inspections.server_index_working_set_pct
        && let Some(registry) = app.try_state::<crate::lsp::DiagnosticsRegistry>()
    {
        registry.restart_everywhere(
            &app,
            cide_ipc::DiagnosticSourceId::for_server("rust-analyzer"),
        );
    }
    // The memory limit is two mechanisms with two lifetimes. The watchdog half reads the
    // setting live on every tick — no reaction needed. The `GOMEMLIMIT` half is baked into
    // the shipped gopls's *environment* at spawn (see `cide_lsp::config::extra_env`), so a
    // running gopls keeps yesterday's soft limit until it restarts — without this, the row
    // half-lies. gopls alone, by name: rust-analyzer never receives the variable, and its
    // restart would be pure cost even now that it is a warm load.
    if was_memory_limit != settings.inspections.server_memory_limit_mb
        && let Some(registry) = app.try_state::<crate::lsp::DiagnosticsRegistry>()
    {
        registry.restart_everywhere(&app, cide_ipc::DiagnosticSourceId::for_server("gopls"));
    }
    // The JSON-log toggle, which reaches running shells for the same reason as the threshold
    // below — and needs no comparison and no registry walk to do it: every shell's renderer
    // reads this one flag per line. Unconditional because storing the value it already holds
    // costs an atomic store on a settings write.
    crate::lifecycle::set_json_logs(settings.terminal.json_logs);
    // The job threshold reaches sessions that are already running, and it has to: a session
    // outlives every pane, tab and window — most shells are spawned once per app run — so a
    // value baked in at spawn would leave the settings row doing nothing visible until the
    // next relaunch. Cheap enough not to gate on anything beyond the comparison: one channel
    // send per session, serviced on a coalescer thread that is already awake, and a no-op on
    // every session that watches no jobs (every Claude pane — its state comes from hooks).
    if was_job_notify != settings.terminal.job_notify_after_secs
        && let Some(registry) = app.try_state::<crate::state::SessionRegistry>()
    {
        let after = crate::lifecycle::job_notify_after(&settings);
        for id in registry.ids() {
            if let Some(session) = registry.get(id) {
                session.set_job_announce_after(after);
            }
        }
    }
    // A new Claude binary means a new version string in every window's header. The probe is
    // a fork of a Node process, so it runs on a blocking worker rather than making the
    // settings form wait ~70ms for its own submit to resolve; the event is what reaches the
    // windows, because capabilities ride `Bootstrap` and no workspace snapshot carries them
    // (`emit::CAPABILITIES_CHANGED`).
    if settings.claude.cli.binary != was_cli {
        let versions = app.state::<crate::caps::ClaudeVersion>().inner().clone();
        let binary = settings.claude.cli.binary.clone();
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            versions.invalidate();
            let capabilities = crate::cmd::app::capabilities(&binary, &versions);
            emit::capabilities_changed(&handle, capabilities);
        });
    }
    Ok(settings)
}

/// The binaries whose *effective* choice differs between two settings values.
///
/// Effective, via [`cide_ipc::InspectionSettings::binary_choice`]'s absent-means-default rule:
/// the map gaining an explicit entry that spells the default out is not a change, and dropping
/// one back to absence is not either. Pure, so the rule is testable without a Tauri state.
fn rebinaried(
    before: &std::collections::BTreeMap<String, cide_ipc::settings::ServerBinaryChoice>,
    after: &std::collections::BTreeMap<String, cide_ipc::settings::ServerBinaryChoice>,
) -> Vec<String> {
    let effective =
        |map: &std::collections::BTreeMap<_, cide_ipc::settings::ServerBinaryChoice>,
         key: &String| { map.get(key).copied().unwrap_or_default() };
    before
        .keys()
        .chain(after.keys())
        .filter(|binary| effective(before, binary) != effective(after, binary))
        .cloned()
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Walk every open project again, because *Show hidden files* or *Show ignored files* moved.
///
/// # Why this is here rather than in the webview
///
/// The obvious alternative is for the settings screen to call `fs.index` after saving. It is
/// wrong in three ways, and each one is a bug that would only appear on somebody else's
/// machine: the screen knows about *its* project and a workspace has several open at once; a
/// second window would keep its stale tree until something else happened to it; and a user who
/// edits `workspace.json` by hand — the file is documented as editable — would get no re-walk
/// at all. The setting lives in Rust, the indexes live in Rust, so the reaction lives in Rust.
///
/// Fire-and-forget, deliberately. A walk of a large repository is seconds, and `settings_set`
/// answers a form: blocking the toggle on the walk would freeze the Settings tab and, with
/// several projects open, freeze it for the sum of them. The tree is not left guessing in the
/// meantime — `Indexing::run` emits `cide://fs-status` at the start of the walk and again at
/// the end, which is exactly what `Explorer` already re-reads its rows on.
fn reindex_open_projects(app: &AppHandle, visibility: cide_fs::Visibility) {
    let open = app.state::<crate::files::FsRegistry>().indexed();
    for (project, roots) in open {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let registry = app.state::<crate::files::FsRegistry>();
            let events: std::sync::Arc<dyn crate::files::FsEvents> =
                std::sync::Arc::new(app.clone());
            match crate::cmd::fs::index_project(events, &registry, project, roots, visibility).await
            {
                Ok(status) => tracing::debug!(
                    ?project,
                    files = status.files,
                    "re-indexed a project for a visibility change"
                ),
                // `NoIndex` (a project with no roots) is the only error this can produce, and
                // a project with no roots had nothing to re-walk. Logged rather than surfaced:
                // the toggle itself succeeded and is already saved.
                Err(error) => tracing::warn!(?project, %error, "could not re-index a project"),
            }
        });
    }
}

/// Fold a patch into stored settings. `None` leaves a field alone.
///
/// Split out from the command so it can be tested without a Tauri state, which is the whole
/// reason the domain crates exist.
fn apply_patch(settings: &mut Settings, patch: SettingsPatch) {
    let SettingsPatch {
        theme,
        ui_font_size,
        each_project_keeps_claude_tab,
        reopen_last_project,
        keep_sessions_on_window_close,
        confirm_close_with_live_session,
        editor,
        terminal,
        graphics,
        claude,
        proxy,
        sidebar,
        explorer,
        inspections,
        git,
    } = patch;

    // Destructured rather than field-by-field on purpose: adding a field to `SettingsPatch`
    // and forgetting to apply it here is a toggle that moves on screen, saves nothing, and
    // springs back on the next snapshot. This way the compiler names the omission.
    if let Some(v) = theme {
        settings.theme = v;
    }
    // Clamped for the two code sizes' reason below, and one more of its own: this number is a
    // *divisor* by the time CSS sees it, so a `NaN` does not merely paint something odd — it
    // invalidates every `calc()` in the size ladder at once, and an invalid `calc()` drops the
    // declaration. That is 405 rules with no `font-size` and a window of UA-default serif.
    if let Some(v) = ui_font_size {
        settings.ui_font_size = cide_ipc::clamp_ui_font_size(v);
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
    // Clamped where the patch lands, for the same reason `sidebar` is: `settings.json` is a
    // file a user can edit, and a zero font size reaches xterm as a zero-wide cell — a blank
    // pane with no error, indistinguishable from every other way a pane comes up blank. The
    // frontend's number input clamps too, and has to, but a clamp that lives only there is one
    // `invoke` away from being bypassed.
    if let Some(mut v) = editor {
        v.font_size = cide_ipc::clamp_font_size(v.font_size);
        settings.editor = v;
    }
    if let Some(mut v) = terminal {
        v.font_size = cide_ipc::clamp_font_size(v.font_size);
        settings.terminal = v;
    }
    if let Some(v) = graphics {
        settings.graphics = v;
    }
    if let Some(v) = claude {
        settings.claude = v;
    }
    // Stored verbatim, credentials and all: a proxy that needs a password cannot be used
    // without one, and cide has no keyring yet. What is guarded is where the value can go
    // afterwards — `ProxySettings` implements `Debug` by hand so that no log line, here or in
    // any future caller, can print it. Nothing on this path logs the patch either way.
    if let Some(v) = proxy {
        settings.proxy = v;
    }
    // Nothing to clamp — an enum and a boolean — and nothing to react to either: the pull
    // strategy is read fresh by `cmd::git::git_pull` on every pull, and the resolver reads its
    // own toggle when it opens a file. Neither is mirrored anywhere that would go stale.
    if let Some(v) = git {
        settings.git = v;
    }
    // The one patched field that is clamped rather than taken at face value.
    //
    // Every other field here is a toggle or an enum, where the wire type already excludes
    // the illegal values; a width is a number, and the caller is a pointer gesture. The
    // frontend clamps too — it has to, because it is the only side that knows how wide the
    // window is — but a clamp that lives only there is one `invoke` away from being
    // bypassed, and the value being written is the one every future launch restores. Clamp
    // where it lands, so `workspace.json` cannot hold a width no window can honour.
    if let Some(v) = sidebar {
        settings.sidebar = v.clamped();
    }
    // Nothing to clamp — two booleans — but the *consequence* is the largest of any field on
    // this type: see `settings_set`, which re-walks every open project when this one moves.
    if let Some(v) = explorer {
        settings.explorer = v;
    }
    // Clamped for the same reason, and the failure is louder than a bad width. `pushToClaude`
    // writes a line into a live Claude pane's PTY; with a debounce of zero, one `cargo check`
    // over a broken crate would type several hundred of them into a prompt the user is in the
    // middle of using. The frontend's number input clamps too, and — as above — a clamp that
    // lives only there is one `invoke` away from being bypassed.
    if let Some(v) = inspections {
        settings.inspections = v.clamped();
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
                conversation: None,
                conversation_since: None,
                title: "settings".into(),
                docker: None,
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
        Err(error) => (Vec::new(), Some(unreadable(&path, &error.to_string()))),
    };

    let readable = parse_problem.is_none();
    report_for(&path, user, parse_problem, readable)
}

/// Apply edits to `keymap.json` and answer with the keymap as it now stands.
///
/// # Why this is one command taking a list
///
/// A single gesture on the screen is often two edits — *bind Ctrl+P to this, and take it off
/// whatever had it* — and those must be one file write. Two commands would leave a window in
/// which the chord is bound twice, broadcast that state to every window, and lose the second
/// half outright if the process died between them.
///
/// # The lock
///
/// Read-modify-write on a file, from a command handler, in an app that can have Settings open
/// in two windows at once. Nothing else serialises this: `WorkspaceState` guards the tree and
/// knows nothing about `keymap.json`, and Tauri runs command handlers on a thread pool. Two
/// saves a moment apart would both read the same file and the second would write a vector that
/// never saw the first one's entries. A process-global lock rather than managed state because
/// the thing being protected is a path, not a value — a second `State` would have to be
/// threaded through every future caller to mean anything, and the one that forgot would be
/// exactly as unserialised as no lock at all.
static KEYMAP_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tauri::command(rename_all = "camelCase")]
pub fn keymap_edit(app: AppHandle, edits: Vec<KeymapEdit>) -> Result<KeymapEditResult, CoreError> {
    // Poisoning would mean a previous edit panicked mid-write. The data behind this lock is a
    // file, re-read from scratch below, so there is no invariant a panic could have left
    // broken and nothing to recover — taking the guard is strictly better than refusing every
    // subsequent edit for the rest of the session.
    let _guard = KEYMAP_WRITE.lock().unwrap_or_else(|e| e.into_inner());

    let path = persist::keymap_path();
    let mut user = match keymap::load_user(&path) {
        Ok(user) => user,
        Err(error) => {
            /*
             * **A file we cannot parse is a file we must not rewrite.**
             *
             * `load_user` answers `Err` for a `keymap.json` with a stray comma — or with the
             * `//` comment a user assumed was legal, since this is VS Code's *shape* and not
             * its JSONC parser. Treating that as "no overrides" and saving would replace
             * everything they had written with the one line they just recorded, and the
             * screen would report success.
             *
             * The exception is **Reset all**, alone, which is the one gesture that means
             * "throw away what is in there" — and it is the escape hatch out of this refusal
             * for a user who does not want to go and find the file. Anything else refuses and
             * names the file, which is what the screen puts on the button it disables.
             */
            if !keymap::may_replace_unreadable(&edits) {
                return Err(CoreError::Serde(format!(
                    "{} could not be read ({error}), so it will not be rewritten — fix it by \
                     hand, or use Reset all to replace it",
                    path.display()
                )));
            }
            Vec::new()
        }
    };

    let mut removed = 0usize;
    let mut added = 0usize;
    for edit in &edits {
        let outcome = keymap::apply_edit(&mut user, edit);
        removed += outcome.removed;
        added += outcome.added;
    }

    keymap::save_user(&path, &user)?;

    // Resolved once and used twice: the windows get it as an event, the caller gets it inside
    // the report. Emitting before the return so the window that made the edit is refreshed by
    // the same path as every other window rather than by its own answer — one mechanism, so a
    // rebind cannot work in the tab you are looking at and nowhere else.
    let resolved = keymap::resolve(&user);
    emit::keymap_changed(&app, &resolved);

    Ok(KeymapEditResult {
        removed: removed as u32,
        added: added as u32,
        report: report_for(&path, user, None, true),
    })
}

/// Everything Settings → Keymap draws, for a user layer that has already been read.
///
/// Shared by [`keymap_report`] and [`keymap_edit`] so the screen is looking at the same
/// arithmetic whichever way it got there — and so an edit answers from the vector it just
/// wrote rather than by re-reading the file it wrote it to, which is a second chance for the
/// answer to disagree with the action.
fn report_for(
    path: &Path,
    user: Vec<cide_ipc::Binding>,
    parse_problem: Option<KeymapProblem>,
    readable: bool,
) -> KeymapReport {
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
        overrides: user,
        readable,
        path: path.display().to_string(),
    }
}

/// The problem line for a `keymap.json` that could not be parsed at all.
///
/// Worth more than the `tracing::error!` `app_get_bootstrap` logs beside it: a file with one
/// stray comma contributes **zero** overrides, so every binding the user set is silently gone
/// and the only visible symptom is that their keymap stopped working.
fn unreadable(path: &Path, error: &str) -> KeymapProblem {
    KeymapProblem {
        key: String::new(),
        command: String::new(),
        message: format!(
            "{} could not be read: {error}. None of your overrides are in effect, and the \
             editor below will not overwrite the file until it parses. Note that this is \
             strict JSON, not JSONC — a `//` comment fails the whole file.",
            path.display()
        ),
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
    let cwd = project_root(&state, project)?;
    run_headless(cwd, request, claude_proxy(&state), claude_cli(&state)).await
}

/// The proxy environment a `claude` one-shot is spawned with.
///
/// Reads `ProxyScope::claude`, deliberately: the scope has three columns and a one-shot is a
/// `claude`, not a shell and not cide's own `git`. A user who takes `claude` out of scope
/// means both lanes, and a one-shot that read `shells` would be answering a question about a
/// pane nobody opened.
///
/// Read here rather than inside `run_headless` so the lock is taken at the point that owns the
/// `State`, and released before the `await`.
fn claude_proxy(state: &WorkspaceState) -> cide_core::proxy::ProxyEnv {
    let proxy = state.with(|ws| ws.settings.proxy.clone());
    cide_core::proxy::ProxyEnv::for_target(&proxy, proxy.scope.claude)
}

/// The project's first root, which is the directory a one-shot runs in.
///
/// It decides which `CLAUDE.md` and which settings the CLI picks up, so a guess would quietly
/// answer about the wrong repository — hence the hard error rather than a fallback to `$HOME`.
fn project_root(state: &WorkspaceState, project: ProjectId) -> Result<PathBuf, HeadlessError> {
    state
        .with(|ws| {
            workspace::project(ws, project)
                .ok()
                .and_then(|p| p.roots.first().map(|r| r.path.clone()))
        })
        .ok_or_else(|| HeadlessError::NoProject {
            project: project.to_string(),
        })
}

/// Run one prompt off the UI thread.
///
/// The blocking work goes to `spawn_blocking` because `cide_claude::headless::run` is
/// deliberately synchronous — it owns three pipe-draining threads and a poll loop, and none of
/// that wants an async runtime. The thread it lands on also stays blocked until the child is
/// gone, which is what makes `PR_SET_PDEATHSIG` on that child safe: the signal fires when the
/// *forking thread* exits, so a spawn from a thread that returns first would kill the run.
async fn run_headless(
    cwd: PathBuf,
    request: HeadlessRequest,
    proxy: cide_core::proxy::ProxyEnv,
    cli: cide_ipc::ClaudeCli,
) -> Result<HeadlessResult, HeadlessError> {
    // Latched, so this is a probe on the first one-shot of the process and free afterwards.
    // Worth doing here rather than only at startup: a user whose CLI self-updated mid-session
    // gets the warning next to the run it might explain.
    //
    // **The latch describes whichever binary was probed first**, which since M16 is a binary
    // the user can change. That is left as it is deliberately: this call exists for one log
    // line, and re-probing on every one-shot would mean re-warning on every one-shot. The
    // *screen's* verdict is unlatched and per-binary — see [`cli_support`] — which is the place
    // the answer has to be current.
    cide_claude::version::check_once(Path::new(&cli.binary));

    // The program **exactly as configured**, which for the default is the bare name `claude`
    // resolved by `PATH` — the same thing every pane spawns. Resolving it ourselves would pin
    // whichever version was on `PATH` at launch, and the CLI updates itself underneath a
    // running app.
    //
    // The proxy is resolved by the caller, not here, and it is `ProxyScope::claude` that
    // decides it: a one-shot *is a claude*, and a user who takes `claude` out of scope means
    // both lanes. Until this argument existed the one-shot lane read no proxy setting at all
    // — it inherited cide's raw environment — so a corporate user whose panes worked got a
    // commit-message generation that hung.
    //
    // # The environment travels, the arguments do not
    //
    // `ProxyScope::claude` already covers "claude panes **and** the headless one-shot lane…
    // one field for both because they are one thing to a user", and the launch configuration's
    // environment follows the same rule for the same reason: a user who points `claude` at a
    // gateway means both lanes.
    //
    // The **arguments** deliberately do not. `cide_claude::headless::argv` is cide's own
    // machinery — `-p --output-format json --no-session-persistence --tools` — and a user
    // `--model` or `--tools` folded into it does not customise a pane, it breaks commit-message
    // generation with a parse error against an envelope that never arrives. The Settings screen
    // says which of the two travels; see `ClaudeCliSection`.
    let plan = cide_core::claude_cli::plan_here(&cli);
    let program = PathBuf::from(cli.binary);
    let run = cide_claude::Headless::new(request, cwd)
        .proxy(proxy)
        .env(plan.env);
    tauri::async_runtime::spawn_blocking(move || cide_claude::headless::run(&program, &run))
        .await
        .map_err(|e| HeadlessError::NotInstalled {
            detail: format!("the headless worker did not finish: {e}"),
        })?
}

/// The user's launch configuration, or the default when there is no workspace.
///
/// One reader rather than four copies of `state.with(|ws| ws.settings.claude.cli.clone())`: the
/// hazard is a call site that forgets and silently keeps spawning the bare `claude`, which is
/// indistinguishable from working right up until somebody configures a binary.
fn claude_cli(state: &WorkspaceState) -> cide_ipc::ClaudeCli {
    state.with(|ws| ws.settings.claude.cli.clone())
}

// --- the two named uses of the headless lane ----------------------------------------------

/// Why a one-shot built on top of the workspace could not even be started.
///
/// A local enum rather than a `cide-ipc` DTO, matching `cmd::session::SessionError`: nothing
/// here crosses the wire as *data*, only as a rejection, and Tauri types a rejected promise as
/// `unknown` on the frontend regardless. Putting it in `cide-ipc` would mint a `ts-rs` type
/// with no reader.
///
/// Tagged, so the caller branches on a variant rather than matching on prose — "nothing is
/// staged" wants a hint and a disabled button, while a failed run wants a retry.
#[derive(Debug, thiserror::Error)]
pub enum ClaudeTaskError {
    #[error("{0}")]
    Headless(#[from] HeadlessError),
    /// Discovering the repository, opening it, or diffing it failed. Carries `cide-git`'s own
    /// message: "not a git repository" and "index changed externally" are different problems
    /// with different remedies, and flattening them into one would lose that.
    #[error("{message}")]
    Git { message: String },
    /// The commit would be empty. Not an error the user should see as a toast — it is a
    /// disabled button — which is why it is its own variant rather than a `Git` message.
    #[error("there is nothing staged to describe")]
    NothingToDescribe,
}

impl serde::Serialize for ClaudeTaskError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let kind = match self {
            Self::Headless(_) => "headless",
            Self::Git { .. } => "git",
            Self::NothingToDescribe => "nothingToDescribe",
        };
        let mut st = s.serialize_struct("ClaudeTaskError", 2)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

impl From<cide_ipc::git::GitError> for ClaudeTaskError {
    fn from(e: cide_ipc::git::GitError) -> Self {
        Self::Git {
            message: e.to_string(),
        }
    }
}

/// Draft a commit message for what is staged, for the git panel's commit box.
///
/// # Why the diff is read here and not passed in
///
/// The panel has a tree of changed *files*; it has never held the patch text, and having it
/// fetch one diff per file to concatenate them would be a round trip per file and a different
/// answer from what `git commit` would actually record. Reading it in one pass through
/// `cide-git` is both cheaper and the same diff the commit itself will contain.
///
/// # Staged, with a deliberate fallback
///
/// Staging-area mode commits the index, so `Staged` is the right diff. Changelist mode — this
/// app's default — commits from the working tree and may have nothing in the index at all, so
/// an empty staged diff falls back to `Combined` rather than refusing. Refusing would make the
/// button dead for the majority of this app's users while looking like a model failure.
///
/// `async` for the same reason as [`claude_headless`]: this waits on a model turn, and a
/// synchronous command would hold the GTK loop for the duration.
#[tauri::command(rename_all = "camelCase")]
pub async fn claude_commit_message(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
) -> Result<HeadlessResult, ClaudeTaskError> {
    let roots: Vec<PathBuf> = state.with(|ws| {
        workspace::project(ws, project)
            .map(|p| p.roots.iter().map(|r| r.path.clone()).collect())
            .unwrap_or_default()
    });
    if roots.is_empty() {
        return Err(ClaudeTaskError::Headless(HeadlessError::NoProject {
            project: project.to_string(),
        }));
    }
    let root = cide_git::repo::find(&roots, repo)
        .map_err(ClaudeTaskError::from)?
        .root;

    let (diff, branch) = staged_diff(&root)?;
    if diff.trim().is_empty() {
        return Err(ClaudeTaskError::NothingToDescribe);
    }

    let request = cide_claude::prompt::commit_message(&diff, branch.as_deref());
    Ok(run_headless(root, request, claude_proxy(&state), claude_cli(&state)).await?)
}

/// The patch text a commit would record, and the branch it would land on.
///
/// Split out from the command so the fallback rule above is one readable function rather than
/// a nested match inside a handler. Everything it touches is `cide-git`'s; nothing here knows
/// what a hunk is.
fn staged_diff(root: &Path) -> Result<(String, Option<String>), ClaudeTaskError> {
    let repo = cide_git::repo::open(root)?;

    // A closure rather than a free function because its parameter would have to be named, and
    // its type is `git2::Repository` — which `cide-git` does not re-export and this crate must
    // not depend on. Adding `git2` to the app crate to spell one signature would put a git
    // implementation inside the glue layer for the sake of tidiness.
    let render = |side: DiffSide| -> Result<String, cide_ipc::git::GitError> {
        let diff = cide_git::diff::build(&repo, cide_git::diff::DiffRequest::new(side), None)?;
        let mut out = String::new();
        for file in cide_git::diff::raw_files(&diff)? {
            // Lossy, and that is the right call: a diff containing one file with invalid
            // UTF-8 should still produce a commit message about the other twelve.
            out.push_str(&String::from_utf8_lossy(&file.render()));
        }
        Ok(out)
    };

    let mut text = render(DiffSide::Staged)?;
    if text.trim().is_empty() {
        text = render(DiffSide::Combined)?;
    }

    // Best-effort: a detached HEAD or an unborn branch is a perfectly ordinary state to
    // commit from, and losing the branch hint is not worth failing the whole call for.
    let branch = cide_git::status::branch_info(&repo)
        .ok()
        .filter(|info| !info.detached && !info.unborn)
        .map(|info| info.head);

    Ok((text, branch))
}

/// Explain a selected region of a file, for the editor's context menu.
///
/// The selection travels as text rather than as a path and a range for the child to read: the
/// buffer on screen may be dirty, and explaining what is on disk when the user asked about
/// what they are looking at is the kind of wrong answer that is hard to notice. It also means
/// the run needs no tools at all — see `cide_claude::prompt`.
#[tauri::command(rename_all = "camelCase")]
// The parameters *are* the wire shape: the macro destructures a flat object the frontend
// sends, so grouping them into a struct would add a type that exists only to be taken apart
// again. `session_spawn` carries the same allow for the same reason.
#[allow(clippy::too_many_arguments)]
pub async fn claude_explain_selection(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    path: String,
    start_line: u32,
    end_line: u32,
    text: String,
    language: Option<String>,
) -> Result<HeadlessResult, HeadlessError> {
    let cwd = project_root(&state, project)?;
    let request = cide_claude::prompt::explain_selection(
        &path,
        language.as_deref(),
        start_line,
        end_line,
        &text,
    );
    run_headless(cwd, request, claude_proxy(&state), claude_cli(&state)).await
}

// --- the CLI version check ----------------------------------------------------------------

/// One refusal or warning sentence, and the flag or variable it belongs to.
///
/// # Why the prose crosses the wire instead of living in TypeScript
///
/// `cide_core::claude_cli` writes each sentence beside the rule it explains, and
/// `RefusedArg::reason`'s own doc says it is "printed beside the struck-out token. Prose, because
/// the screen prints it." It was not printed: `Verdict::note()` had exactly one caller, a
/// `tracing` line, so a user who typed `CLAUDE_CODE_USE_BEDROCK=1` got a yellow border and no
/// sentence — while the words "Bills an AWS account rather than your subscription" existed, were
/// asserted non-empty by a test, and reached nobody.
///
/// `ui/src/settings/claudeCli.ts` deliberately carries only the *names*, because a sentence
/// written twice is a sentence that will say two things. So the names are matched there for
/// instant feedback with no round trip, and the prose is shipped once, from here, and looked up.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliReason {
    /// The long-form flag or the variable name, as the table spells it.
    pub name: String,
    pub reason: String,
}

/// What `claude --version` says, against what the IDE protocol was verified with.
///
/// A separate command from `app.getBootstrap`'s `claudeVersion`, which is the raw string.
/// This is the *verdict*, and the verdict has to come from Rust: the range lives in
/// `cide_ide_mcp::protocol::SUPPORTED_CLI`, and a copy of it in TypeScript would be a second
/// place to update on the next release — which is exactly the drift this is meant to detect.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCliSupport {
    /// The version as parsed, or `None` when nothing answered `--version`.
    pub version: Option<String>,
    /// The versions this build's protocol description was checked against, for a human.
    pub verified_range: String,
    /// One sentence, when there is something to say. `None` on a verified CLI: a note that
    /// says "everything is fine" on every launch is a note nobody reads on the launch it
    /// matters.
    pub warning: Option<String>,
    /// The last `claude` to complete the IDE handshake **on this machine**.
    ///
    /// # A different question from every other field here
    ///
    /// The three above are all derived from `SUPPORTED_CLI` and `claude --version`: a property
    /// of the *source tree* measured against a probe of `PATH`. They answer "has anyone
    /// checked this version of the protocol", and a version-string comparison against a
    /// constant verifies nothing about protocol drift — it verifies that a human typed a
    /// number.
    ///
    /// This is a *per-machine observation*: a real CLI read our lockfile, chose the WebSocket
    /// transport, presented the auth header, and had its `initialize` reply accepted, here.
    /// It is the only thing on this screen that can honestly be shown as evidence, precisely
    /// because it does not claim anything about anybody else's machine.
    ///
    /// `None` before anything has ever connected — a fresh install, or a machine where the
    /// IDE integration has never worked, which are different situations the screen words
    /// differently. See `cide_core::handshake`.
    pub handshake: Option<cide_core::handshake::Handshake>,

    // --- M16: which binary all of the above is about ---------------------------------------
    /// The configured binary, echoed back.
    ///
    /// Echoed rather than assumed by the screen, because the three fields above describe *this*
    /// value and a screen drawing them beside a field the user has since edited would be
    /// captioning one binary's version with another's name.
    pub binary: String,
    /// Where it resolves to — the absolute path the OS would `exec`.
    ///
    /// Shown, and never spawned. What a pane runs is [`Self::binary`] exactly as stored, so a
    /// bare `claude` is resolved afresh at every spawn; the CLI updates itself underneath a
    /// running app and pinning this answer would keep panes on a version that no longer exists.
    /// The path is here because "which one is that, actually" is the question a wrapper, a shim
    /// or a `~/.local/bin` ordering makes unanswerable from the field alone.
    pub resolved: Option<String>,
    /// Why it cannot be run at all, in one sentence. `None` when it can.
    ///
    /// The **only** hard verdict on this screen. Everything else about a binary — a wrapper
    /// script, a `--version` that prints something unparseable, a version outside the verified
    /// range — is a warning, because `mise`, `asdf`, `direnv` and a plain shell wrapper are all
    /// legitimate ways to name a `claude` and none of them answers `--version` in a shape worth
    /// refusing over. The project already paid for the opposite arrangement:
    /// `~/.cargo/bin/rust-analyzer` is a symlink to `rustup`, which passes any
    /// on-PATH-and-executable probe and then fails at exec.
    pub problem: Option<String>,
    /// Every argument sentence, keyed by the flag **as cide will actually spell it**. See
    /// [`CliReason`] and [`arg_reasons`].
    pub arg_reasons: Vec<CliReason>,
    /// Every environment sentence, keyed by variable name.
    pub env_reasons: Vec<CliReason>,

    // --- M17: the arguments cide adds ------------------------------------------------------
    /// Why a rename typed into the injection rows was thrown away, keyed by the injection's
    /// wire name (`sessionId`, `resume`, `forkSession`, `settings`).
    ///
    /// Keyed by the injection rather than by the text, because the text is exactly what was
    /// discarded — and because the same string may legitimately appear as a user argument
    /// elsewhere on the screen, where it means something else and has a different sentence.
    /// Empty in the common case: a rename that survived says nothing.
    pub inject_reasons: Vec<CliReason>,
}

/// Whether the configured CLI can be run, and whether this build's IDE protocol was ever
/// checked against its version.
///
/// # It stopped being latched in M16, and that is the point
///
/// The probe used to be `version::check_once`, whose `OnceLock` describes whichever binary was
/// probed first in this process. With `claude` a bare name that is the same binary every time
/// and the latch is free. With a **configurable** binary it is a lie the moment the user edits
/// the field: they would type a path, press nothing, and read back the version of the `claude`
/// that answered ten minutes ago — which is the exact failure this screen exists to prevent.
///
/// So the screen's answer is unlatched and per-binary, and the latch stays where it belongs:
/// `run_headless` still calls `check_once` for its one-line-per-process log warning, and its
/// own comment now says which binary that line describes.
///
/// The cost is one `claude --version` per Settings tab mount and per edit of the binary field —
/// a Node boot, so hundreds of milliseconds — on the blocking pool. `SettingsTab` keys its
/// effect on the configured binary so it is one probe per distinct value, not one per keystroke.
///
/// `async`, with the probe on the blocking pool, and it has to be. A Tauri command that is not
/// `async` runs on the main thread, which here is the GTK loop, and `claude` is a Node.js
/// program whose startup is measured in hundreds of milliseconds. Sync, that is every window in
/// the application frozen for the length of a Node boot at the exact moment the user opened
/// Settings — the same defect `session_spawn` and `session_scrollback` were moved off the main
/// thread for.
/// `AppHandle` rather than `State<'_, WorkspaceState>`, which is the shape every other reader
/// on this screen uses. A `State` borrow cannot cross an `await`, so an async command taking one
/// is forced to return a `Result` whose error variant nothing can produce — a rejected promise
/// the frontend would have to handle and never see. `session_spawn` reaches the workspace the
/// same way and for the same reason.
#[tauri::command(rename_all = "camelCase")]
pub async fn claude_cli_support(app: AppHandle) -> ClaudeCliSupport {
    // Read on this thread and cloned out: the workspace lock is `parking_lot` and must not be
    // held across an await, let alone across a `fork`.
    // The whole launch configuration, not only the binary: since the injection switches exist,
    // *which* arguments are refused depends on which ones cide is still going to pass, so a
    // verdict computed from the binary alone would strike out a `--session-id` the user is now
    // entitled to write.
    let cli = app
        .try_state::<WorkspaceState>()
        .map(|state| claude_cli(&state))
        .unwrap_or_default();
    // A join failure means the pool is going away, which is a shutting-down application. The
    // no-CLI answer is the honest one to draw with and this screen must still render.
    let fallback = cli.clone();
    tauri::async_runtime::spawn_blocking(move || cli_support(&cli))
        .await
        .unwrap_or_else(|_| ClaudeCliSupport {
            version: None,
            verified_range: cide_claude::version::verified_range(),
            warning: None,
            handshake: None,
            binary: String::new(),
            resolved: None,
            problem: None,
            arg_reasons: arg_reasons(&fallback),
            env_reasons: env_reasons(),
            inject_reasons: inject_reasons(&fallback),
        })
}

/// The verdict, computed synchronously. Separate from the command so a test can call it
/// without an async runtime, and so the command body is only the threading decision.
///
/// **Blocking, and it forks.** `resolve` is a `stat` per `PATH` entry; `probe` is a whole Node
/// boot. Neither is `arm`ed, deliberately: `arm` hands the kernel a pid to kill when the
/// *forking thread* exits, and a `--version` that outlives this function by a millisecond has
/// nothing to leak — it is not a session, it holds no subscription slot and it exits on its
/// own. Arming it would only add a `pre_exec` to a process that is already gone.
fn cli_support(cli: &cide_ipc::ClaudeCli) -> ClaudeCliSupport {
    let resolved = cide_core::claude_cli::resolve(&cli.binary);
    // Only probed when there is something to probe. Forking a binary already known to be
    // missing would spend a Node boot to learn what `resolve` just said, and would produce a
    // second, worse sentence for the same problem.
    let support = match &resolved {
        Ok(path) => cide_claude::version::support_of(cide_claude::version::probe(path).as_deref()),
        Err(_) => cide_claude::version::Support::Missing,
    };
    let handshake = cide_core::handshake::load(&cide_core::handshake::handshake_path());
    verdict(&support, handshake, cli, resolved)
}

/// Build the answer from a verdict and a record, without touching `PATH` or the disk.
///
/// # This split exists to make a test possible that was not
///
/// What stood here was one function reading `check_once`, and its test asserted
/// `support.warning.is_some() == check_once(...).is_warning()` — the same latched value on
/// both sides of an equals sign. It could not fail: not on a machine with no `claude`, not on
/// one with a CLI five releases past the range, not if this function had returned
/// `warning: None` unconditionally. A test that cannot fail is worse than no test, because it
/// occupies the place where the real one would go.
///
/// With the inputs handed in, every branch is drivable: a `Support::Newer` really does produce
/// a warning, a `Support::Verified` really does produce none, and a record from an older build
/// really does keep its own range rather than being relabelled with this one's. The binary
/// verdict is handed in for the same reason — a test can drive a missing binary on a machine
/// where `claude` is installed, which is every machine this is developed on.
fn verdict(
    support: &cide_claude::version::Support,
    handshake: Option<cide_core::handshake::Handshake>,
    cli: &cide_ipc::ClaudeCli,
    resolved: Result<PathBuf, cide_core::claude_cli::BinaryProblem>,
) -> ClaudeCliSupport {
    ClaudeCliSupport {
        version: support.version().map(str::to_string),
        verified_range: cide_claude::version::verified_range(),
        warning: support.is_warning().then(|| support.message()),
        // Passed through exactly as stored, range included. Overwriting the stored range with
        // this build's would erase the one signal that says the record is from a different
        // build — which is the case the screen has its own sentence for.
        handshake,
        binary: cli.binary.clone(),
        resolved: resolved
            .as_ref()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        problem: resolved.err().map(|problem| problem.message()),
        arg_reasons: arg_reasons(cli),
        env_reasons: env_reasons(),
        inject_reasons: inject_reasons(cli),
    }
}

/// Every argument sentence, refused and warned alike, keyed by the flag the screen will match.
///
/// Both tables in one list because the *screen* does not need them separated — the row already
/// knows its own verdict from matching the name locally; what it lacks is the prose.
///
/// # Why this takes the configuration, and what would go wrong without it
///
/// Since the injection switches, a refusal can be *lifted* and a flag can be *renamed*, and
/// this table is looked up by name. Keyed by `entry.flag` unconditionally it would produce two
/// wrong screens: a sentence beside a `--session-id` the user is now entitled to pass — which
/// is a refusal claimed for a token that reaches the child — and no sentence at all beside the
/// `--sid` cide has been told to write instead, which is the state the whole `CliReason`
/// mechanism was added to end.
///
/// So a conditional entry is keyed by the **effective** spelling and dropped entirely when its
/// injection is off. An unconditional one (`--bare`, `--print`) and every warned one are keyed
/// by their own name, as before: nothing cide passes is involved in either.
fn arg_reasons(cli: &cide_ipc::ClaudeCli) -> Vec<CliReason> {
    use cide_core::claude_cli::{REFUSED_ARGS, WARNED_ARGS, injected, spec_of};

    let inject = injected(cli).0;
    REFUSED_ARGS
        .iter()
        .filter_map(|entry| {
            let name = match entry.because {
                None => entry.flag.to_string(),
                // Nothing to explain about a flag the user may now pass.
                Some(which) => {
                    let effective = inject.flag(which)?;
                    // Only the entry that *is* the injected flag follows the rename;
                    // `--continue` is the CLI's own flag, merely illegal beside ours.
                    if entry.flag == spec_of(which).default_flag {
                        effective.to_string()
                    } else {
                        entry.flag.to_string()
                    }
                }
            };
            Some(CliReason {
                name,
                reason: entry.reason.to_string(),
            })
        })
        .chain(WARNED_ARGS.iter().map(|entry| CliReason {
            name: entry.flag.to_string(),
            reason: entry.reason.to_string(),
        }))
        .collect()
}

/// Why a rename typed into the injection rows was discarded, keyed by the injection's wire
/// name. Empty in the common case.
///
/// The prose is `cide_core::claude_cli`'s, like every other sentence on this screen and for
/// the same reason: written twice it would say two things, and this side has no way to notice.
fn inject_reasons(cli: &cide_ipc::ClaudeCli) -> Vec<CliReason> {
    use cide_core::claude_cli::{INJECTIONS, injected};

    injected(cli)
        .1
        .into_iter()
        .filter_map(|note| {
            let spec = INJECTIONS.get(note.index)?;
            Some(CliReason {
                name: spec.key.to_string(),
                reason: note.verdict.note()?.to_string(),
            })
        })
        .collect()
}

/// The same for environment variables.
fn env_reasons() -> Vec<CliReason> {
    use cide_core::claude_cli::{REFUSED_ENV, WARNED_ENV};
    REFUSED_ENV
        .iter()
        .chain(WARNED_ENV.iter())
        .map(|(name, reason)| CliReason {
            name: (*name).to_string(),
            reason: (*reason).to_string(),
        })
        .collect()
}

// --- colour schemes -----------------------------------------------------------------------

/// Import colour themes from a `.vsix` or a theme `.json`, and return what landed.
///
/// **A list, because a `.vsix` usually carries a pair.** Themes ship light-and-dark far more
/// often than not, and the setting is keyed by polarity — importing one of a pair would leave
/// the other theme on the builtin and the user back at the file dialog. `cide_core::scheme`
/// reads every theme the package declares.
///
/// An **empty** list means the user cancelled, which is not an error and must not be reported as
/// one — `project_pick` answers a cancelled folder pick the same way. A file that is not a theme
/// is an error, because the user picked it on purpose.
///
/// The conversion is `cide_core::scheme`, which is where the interesting decisions are and where
/// they can be tested. This command is the three things that need a running app: a parented file
/// dialog, a write, and the broadcast that stops a second window from resolving a new id against
/// a list it has not been given.
///
/// It deliberately does **not** select anything. Selecting is a settings patch, and the two are
/// separate because an imported theme's polarity may not be the one on screen: the frontend knows
/// which theme this window is showing and can say so, whereas a command that silently wrote
/// `color_scheme_dark` would leave a user staring at an unchanged light editor with nothing to
/// explain it.
#[tauri::command(rename_all = "camelCase")]
pub async fn scheme_import(
    app: AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Vec<cide_ipc::ColorScheme>, CoreError> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    crate::cmd::project::show_picker(
        &app,
        window,
        crate::cmd::project::PickerSpec {
            title: "Import a colour theme",
            accept: "Import",
            folders: false,
            // One *file* at a time — which is not one theme: a `.vsix` normally holds a pair.
            // A multi-select would have to answer "which of these did you want selected?" across
            // files as well as within one, and the answer is a dialog nobody asked for.
            multiple: false,
            // `.vsix` first, because it is what a marketplace hands you and the reason this
            // filter grew: a user who downloads one and finds the picker will not open it has no
            // way to guess that the theme is a `.json` inside the archive.
            filter: Some(("Colour themes", &["*.vsix", "*.json", "*.jsonc"])),
        },
        tx,
    )?;

    // `spawn_blocking` rather than blocking this task: the answer arrives only when the user
    // has finished browsing, which is unbounded, and the async runtime's worker pool is shared
    // with every other command in flight. Exactly `project_pick`'s reasoning.
    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or_default())
        .await
        .map_err(|e| CoreError::Io(format!("colour theme picker: {e}")))?;
    let Some(path) = picked.into_iter().next() else {
        return Ok(Vec::new());
    };

    // Off the async runtime's worker pool: a `.vsix` is read whole and inflated, which is
    // milliseconds but is unbounded blocking work on a thread shared with every command in
    // flight. `project_pick`'s reasoning about `recv`, applied to the step after it.
    let imported = tauri::async_runtime::spawn_blocking(move || {
        cide_core::scheme::import(&path).map(cide_core::scheme::save_all)
    })
    .await
    .map_err(|e| CoreError::Io(format!("importing a colour theme: {e}")))??;

    emit::schemes_changed(&app, cide_core::scheme::load_all());
    Ok(imported)
}

/// Forget an imported colour scheme.
///
/// The *setting* is left alone on purpose. A window still naming this id falls back to the
/// builtin scheme at paint time — `ui/src/editor/scheme.ts` does that with no round trip — and
/// repairing the two settings fields here would mean deciding, on the user's behalf, that they
/// did not mean to re-import it. Removing a scheme is reversible; overwriting a preference is
/// the kind of quiet correction this project keeps out of the persistence layer.
#[tauri::command(rename_all = "camelCase")]
pub fn scheme_remove(app: AppHandle, id: String) -> Result<(), CoreError> {
    cide_core::scheme::remove(&id)?;
    emit::schemes_changed(&app, cide_core::scheme::load_all());
    Ok(())
}

// --- the log directory --------------------------------------------------------------------

/// Open the directory `tauri-plugin-log` writes to, in the desktop's file manager.
///
/// The plugin has been installed since the first milestone and there has been no way to reach
/// what it writes, which makes "send me your log" a request the user cannot act on. Returns
/// the path as well as opening it, so a screen can show it for the case where no file manager
/// is installed — a headless-ish Linux desktop being exactly where the logs are wanted.
///
/// Opened from Rust rather than through the `opener` plugin's JS command on purpose: the JS
/// path is capability-gated per window, and a detached pane window deliberately has no
/// `opener` permission. Doing it here means the same gesture works from every window without
/// widening what a webview may open to include arbitrary paths.
#[tauri::command(rename_all = "camelCase")]
pub fn app_open_log_dir(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt;

    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("this platform has no log directory: {e}"))?;

    // The directory does not exist until the plugin writes its first line, and asking a file
    // manager to open a path that is not there is an error dialog with no explanation in it.
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("could not open {}: {e}", dir.display()))?;

    Ok(dir.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{SIDEBAR_MAX_WIDTH, SidebarSettings, Theme, WindowMode};

    #[test]
    fn only_an_effective_binary_choice_change_restarts_a_server() {
        use cide_ipc::settings::ServerBinaryChoice;
        let map = |entries: &[(&str, ServerBinaryChoice)]| {
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect::<std::collections::BTreeMap<_, _>>()
        };

        // The move that matters, in both directions.
        assert_eq!(
            rebinaried(
                &map(&[]),
                &map(&[("rust-analyzer", ServerBinaryChoice::System)])
            ),
            vec!["rust-analyzer".to_string()]
        );
        assert_eq!(
            rebinaried(
                &map(&[("rust-analyzer", ServerBinaryChoice::System)]),
                &map(&[])
            ),
            vec!["rust-analyzer".to_string()]
        );
        // Spelling the default out is not a change — absent already means Builtin, and a
        // restart is a re-index nobody asked for.
        assert!(
            rebinaried(
                &map(&[]),
                &map(&[("rust-analyzer", ServerBinaryChoice::Builtin)])
            )
            .is_empty()
        );
        // An unrelated patch of the same map restarts nothing.
        let held = map(&[("rust-analyzer", ServerBinaryChoice::System)]);
        assert!(rebinaried(&held, &held.clone()).is_empty());
    }

    /// A launch configuration with nothing but the binary set — every injection at its
    /// shipped default, which is what these verdict tests are about.
    fn named(binary: &str) -> cide_ipc::ClaudeCli {
        cide_ipc::ClaudeCli {
            binary: binary.into(),
            ..Default::default()
        }
    }

    /// The sentences follow the flag cide will actually write, and disappear when it stops
    /// writing one.
    ///
    /// Keyed by `entry.flag` unconditionally — which is what stood here before the injections
    /// existed — this screen would strike nothing out beside a renamed `--sid` while claiming a
    /// refusal for a `--session-id` that now reaches the child untouched. Both halves are a
    /// wrong label on the only surface a user has for this.
    #[test]
    fn an_argument_sentence_follows_the_injection_it_belongs_to() {
        let names = |cli: &cide_ipc::ClaudeCli| -> Vec<String> {
            arg_reasons(cli).into_iter().map(|r| r.name).collect()
        };

        let shipped = names(&named("claude"));
        assert!(shipped.contains(&"--session-id".to_string()));
        assert!(shipped.contains(&"--continue".to_string()));
        assert!(shipped.contains(&"--bare".to_string()));
        assert!(shipped.contains(&"--safe-mode".to_string()));

        let mut off = named("claude");
        off.inject.session_id.enabled = false;
        let off = names(&off);
        assert!(
            !off.contains(&"--session-id".to_string()),
            "cide no longer passes it, so there is nothing to explain about the user's own"
        );
        assert!(
            !off.contains(&"--continue".to_string()),
            "it was only illegal beside an injected session id"
        );
        assert!(
            off.contains(&"--bare".to_string()),
            "an authentication failure is not an injection's business"
        );

        let mut renamed = named("claude");
        renamed.inject.session_id.flag = "--sid".into();
        let renamed = names(&renamed);
        assert!(renamed.contains(&"--sid".to_string()));
        assert!(!renamed.contains(&"--session-id".to_string()));
        assert!(
            renamed.contains(&"--continue".to_string()),
            "`--continue` is the CLI's own flag; the rename is cide's, so it stays put"
        );
    }

    /// A discarded rename gets its sentence, keyed by the injection rather than by the text
    /// that was thrown away.
    #[test]
    fn a_discarded_rename_says_why_and_an_accepted_one_says_nothing() {
        assert!(inject_reasons(&named("claude")).is_empty());

        let mut cli = named("claude");
        cli.inject.session_id.flag = "sid".into();
        let reasons = inject_reasons(&cli);
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].name, "sessionId");
        assert!(
            reasons[0].reason.contains("prompt"),
            "{}",
            reasons[0].reason
        );

        let mut cli = named("claude");
        cli.inject.settings.flag = "--sid".into();
        assert!(inject_reasons(&cli).is_empty(), "a usable rename is silent");
    }

    #[test]
    fn a_patch_touches_only_the_fields_it_names() {
        let mut settings = Settings::default();
        settings.terminal.font_size = 17.0;

        // Dark, because Light is now `Theme::default()`: patching a field to the value it
        // already holds asserts nothing, and this half of the test would pass against an
        // `apply_patch` that ignored `theme` entirely. Pinned rather than left implicit —
        // the default has moved once already.
        assert_ne!(
            Theme::Dark,
            Settings::default().theme,
            "patch the non-default theme, or this assertion is vacuous"
        );
        apply_patch(
            &mut settings,
            SettingsPatch {
                theme: Some(Theme::Dark),
                ..SettingsPatch::default()
            },
        );

        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(
            settings.terminal.font_size, 17.0,
            "an untouched group moved"
        );
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

    /// The full trip a drag makes: patch in, clamped, stored, and the *other* panel's width
    /// still there afterwards.
    #[test]
    fn a_sidebar_patch_is_clamped_and_does_not_disturb_the_other_panel() {
        let mut settings = Settings::default();
        settings.terminal.font_size = 17.0;

        // What a drag past the right-hand limit sends. The frontend would have clamped it
        // against the viewport already; this asserts the store does not depend on that.
        apply_patch(
            &mut settings,
            SettingsPatch {
                sidebar: Some(SidebarSettings {
                    files_width: 5_000,
                    git_width: 500,
                    // Spread rather than a literal: `SidebarSettings` gained `agents_width`
                    // in M18 and will gain the next panel's width too, and a test that has to
                    // be edited for every new panel is a test that will be edited carelessly.
                    ..SidebarSettings::default()
                }),
                ..SettingsPatch::default()
            },
        );
        assert_eq!(settings.sidebar.files_width, SIDEBAR_MAX_WIDTH);
        assert_eq!(
            settings.sidebar.git_width, 500,
            "the git width was rewritten"
        );
        assert_eq!(
            settings.terminal.font_size, 17.0,
            "an untouched group moved"
        );

        // And a patch that names nothing leaves the widths where the drag left them —
        // the case where flipping an unrelated toggle would reset the panel.
        apply_patch(
            &mut settings,
            SettingsPatch {
                theme: Some(Theme::Dark),
                ..SettingsPatch::default()
            },
        );
        assert_eq!(settings.sidebar.files_width, SIDEBAR_MAX_WIDTH);
        assert_eq!(settings.sidebar.git_width, 500);
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
    fn a_task_error_serialises_as_a_kind_the_frontend_can_branch_on() {
        // The whole reason this is not a `String`. "nothing to describe" is a disabled button
        // and a hint; a failed run is a retry; a git failure is neither. A frontend matching
        // on prose would break the moment one of these messages was reworded.
        let kinds = [
            (ClaudeTaskError::NothingToDescribe, "nothingToDescribe"),
            (
                ClaudeTaskError::Git {
                    message: "not a repository".into(),
                },
                "git",
            ),
            (
                ClaudeTaskError::Headless(HeadlessError::TimedOut { seconds: 180 }),
                "headless",
            ),
        ];
        for (error, expected) in kinds {
            let value = serde_json::to_value(&error).expect("serialises");
            assert_eq!(value["kind"], expected);
            assert!(
                value["message"].as_str().is_some_and(|m| !m.is_empty()),
                "{expected} carried no message"
            );
        }
    }

    /// A throwaway repository with one commit, for the diff tests below.
    ///
    /// Driven through the `git` binary rather than `git2`, for the reason `staged_diff` gives
    /// for not naming a `git2` type: this crate must not depend on a git implementation. It is
    /// also the right reference — the question these tests ask is "does this produce what a
    /// commit would record", and only git can answer that authoritatively.
    ///
    /// Every `git` invocation carries `-c` overrides for the settings a developer's own
    /// `~/.gitconfig` could otherwise change the answer with. `commit.gpgsign` is the one that
    /// would not merely alter the output but fail the commit outright, on the machine of
    /// anybody who signs by default.
    struct TempRepo {
        root: PathBuf,
    }

    impl TempRepo {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("cide-claude-task-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("temp repo");
            let repo = Self { root };
            repo.git(&["init", "-q", "-b", "main"]);
            repo.write("kept.txt", "one\ntwo\nthree\n");
            repo.git(&["add", "."]);
            repo.git(&["commit", "-qm", "base"]);
            repo
        }

        fn git(&self, args: &[&str]) {
            let output = std::process::Command::new("git")
                .current_dir(&self.root)
                .args([
                    "-c",
                    "user.name=cide tests",
                    "-c",
                    "user.email=tests@cide.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.autocrlf=false",
                ])
                .args(args)
                .output()
                .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
            assert!(
                output.status.success(),
                "git {args:?} failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn write(&self, name: &str, body: &str) {
            std::fs::write(self.root.join(name), body).expect("write");
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_staged_change_becomes_the_patch_text_a_commit_would_record() {
        // The end-to-end question, and the one nothing else here asks: `claude_commit_message`
        // is only worth anything if what reaches the prompt is a real unified diff. Every
        // piece between the command and the model — `repo::open`, `diff::build`,
        // `raw_files`, `RawFile::render` — is exercised by this and by nothing else, and a
        // silent empty string here is `NothingToDescribe` on a repository with staged work.
        let repo = TempRepo::new("staged");
        repo.write("kept.txt", "one\nTWO\nthree\n");
        repo.git(&["add", "kept.txt"]);

        let (diff, branch) = staged_diff(&repo.root).expect("a staged change");

        assert!(diff.contains("diff --git"), "no patch header:\n{diff}");
        assert!(diff.contains("+++ b/kept.txt"), "no file header:\n{diff}");
        assert!(diff.contains("+TWO"), "the addition is missing:\n{diff}");
        assert!(diff.contains("-two"), "the deletion is missing:\n{diff}");
        assert_eq!(branch.as_deref(), Some("main"));
    }

    #[test]
    fn an_empty_index_falls_back_to_the_working_tree_instead_of_refusing() {
        // Changelist mode is this app's default and commits from the working tree, so for most
        // users the index is empty at the moment they press the button. Without the fallback
        // the staged diff is empty, the command answers `NothingToDescribe`, and the feature
        // is dead for the majority of this application's users while looking like a model that
        // declined to answer.
        let repo = TempRepo::new("unstaged");
        repo.write("kept.txt", "one\ntwo\nCHANGED\n");

        let (diff, _) = staged_diff(&repo.root).expect("a working-tree change");

        assert!(
            diff.contains("+CHANGED"),
            "an unstaged change produced no diff, so the button is dead in changelist mode:\n{diff}"
        );
    }

    #[test]
    fn a_clean_repository_produces_nothing_to_describe_rather_than_an_empty_prompt() {
        // The other half of the fallback: it must not be so eager that a clean tree yields a
        // whitespace-only diff, which would send the model a prompt with no diff in it and
        // bill for whatever it invented.
        let repo = TempRepo::new("clean");

        let (diff, _) = staged_diff(&repo.root).expect("a clean work tree is not an error");

        assert!(
            diff.trim().is_empty(),
            "a clean repository diffed as:\n{diff}"
        );
    }

    #[test]
    fn a_commit_message_over_something_that_is_not_a_repository_is_a_git_error() {
        // Rather than a panic or a `NoProject`. The project's root is perfectly real here —
        // it is simply not a work tree, which is an ordinary state for a project opened on a
        // scratch directory.
        let outside = std::env::temp_dir();
        let error = staged_diff(&outside).expect_err("a temp dir is not a work tree");
        assert!(
            matches!(error, ClaudeTaskError::Git { .. }),
            "{error:?} is not the variant the panel branches on"
        );
    }

    /// **This replaces a test that could not fail.**
    ///
    /// What was here asserted `support.warning.is_some() == check_once(...).is_warning()` —
    /// and both sides were the same `OnceLock`. It was true on a machine with no `claude`, on
    /// a machine five releases past the range, and would have been true if `cli_support` had
    /// returned `warning: None` unconditionally. Driving `verdict` with a constructed
    /// `Support` is what makes each branch a real assertion.
    #[test]
    fn a_warning_is_produced_exactly_when_the_verdict_is_one() {
        use cide_claude::version::Support;

        let newer = Support::Newer {
            version: "9.9.9".into(),
        };
        let answer = verdict(
            &newer,
            None,
            &named("claude"),
            Ok(PathBuf::from("/usr/bin/claude")),
        );
        assert_eq!(answer.version.as_deref(), Some("9.9.9"));
        let warning = answer.warning.expect("a CLI past the range is news");
        assert!(warning.contains("9.9.9"), "{warning}");
        assert!(
            warning.contains(&answer.verified_range),
            "the message has to name both numbers or it is unactionable: {warning}"
        );

        // Verified: no note. One that reads "everything is fine" on every launch is one
        // nobody reads on the launch it matters.
        let verified = Support::Verified {
            version: "2.1.226".into(),
        };
        assert_eq!(
            verdict(
                &verified,
                None,
                &named("claude"),
                Ok(PathBuf::from("/usr/bin/claude"))
            )
            .warning,
            None
        );

        // Missing: also no *protocol* warning. "claude is not installed" is a different
        // message and the screen's own heading already says it.
        let missing = || {
            verdict(
                &Support::Missing,
                None,
                &named("claude"),
                Err(cide_core::claude_cli::BinaryProblem::NotOnPath {
                    name: "claude".into(),
                }),
            )
        };
        assert_eq!(missing().warning, None);
        assert_eq!(missing().version, None);
        assert!(
            missing().problem.is_some(),
            "the *binary* verdict is where a missing claude is reported, and it is the only \
             hard refusal on this screen"
        );
        assert_eq!(missing().resolved, None);

        // The range is always populated, whatever the verdict.
        for support in [&newer, &verified, &Support::Missing] {
            assert!(
                !verdict(
                    support,
                    None,
                    &named("claude"),
                    Ok(PathBuf::from("/usr/bin/claude"))
                )
                .verified_range
                .is_empty()
            );
        }
    }

    /// A record written by an older build keeps the range it was written against.
    ///
    /// That disagreement is the whole reason the range is stored beside the version: it is
    /// what lets the screen say "this was recorded against an older build" rather than
    /// silently presenting a stale observation as though it were about this one.
    #[test]
    fn a_stored_handshake_keeps_its_own_range_rather_than_being_relabelled() {
        let stored = cide_core::handshake::Handshake {
            version: Some("2.1.227".into()),
            at_unix_ms: 1_700_000_000_000,
            verified_range: "an older build's range".into(),
        };
        let answer = verdict(
            &cide_claude::version::Support::Verified {
                version: "2.1.227".into(),
            },
            Some(stored.clone()),
            &named("claude"),
            Ok(PathBuf::from("/usr/bin/claude")),
        );

        let carried = answer.handshake.expect("the record is carried through");
        assert_eq!(carried, stored);
        assert_ne!(
            carried.verified_range, answer.verified_range,
            "the two ranges are allowed to differ, and the screen needs to be able to see it"
        );
    }

    /// And the real one still assembles on whatever machine this runs on.
    ///
    /// Kept as a smoke test rather than as the assertion: it reads `PATH` and the state
    /// directory, so the only things it can honestly claim are the invariants that hold
    /// either way.
    #[test]
    fn the_real_verdict_assembles_with_a_range_whatever_is_installed() {
        let support = cli_support(&named("claude"));
        assert!(!support.verified_range.is_empty());
        assert_eq!(
            support.warning.is_some(),
            support.version.is_some()
                && !matches!(
                    cide_claude::version::support_of(support.version.as_deref()),
                    cide_claude::version::Support::Verified { .. }
                ),
            "a warning appears exactly when the parsed version is outside the range"
        );
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
