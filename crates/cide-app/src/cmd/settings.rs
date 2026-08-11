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
    KeymapConflict, KeymapProblem, KeymapReport, Pane, PaneId, PaneKind, PaneRole, ProjectId,
    RepoId, Settings, SettingsPatch, SettingsSection, TabId, TabKind,
};
use tauri::{AppHandle, Manager, State};

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
    // Read before the patch, so the comparison below is against what the windows are
    // actually wearing rather than against the value we just wrote.
    let was = state.with(|ws| ws.settings.theme);
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
    if settings.theme != was {
        windows::apply_theme(&app, settings.theme);
    }
    Ok(settings)
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
        sidebar,
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
    let cwd = project_root(&state, project)?;
    run_headless(cwd, request).await
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
) -> Result<HeadlessResult, HeadlessError> {
    // Latched, so this is a probe on the first one-shot of the process and free afterwards.
    // Worth doing here rather than only at startup: a user whose CLI self-updated mid-session
    // gets the warning next to the run it might explain.
    cide_claude::version::check_once(Path::new(CLAUDE_PROGRAM));

    // The bare name, resolved by `PATH` — the same thing every pane spawns. Resolving it
    // ourselves would pin whichever version was on `PATH` at launch, and the CLI updates
    // itself underneath a running app.
    let run = cide_claude::Headless::new(request, cwd);
    tauri::async_runtime::spawn_blocking(move || {
        cide_claude::headless::run(Path::new(CLAUDE_PROGRAM), &run)
    })
    .await
    .map_err(|e| HeadlessError::NotInstalled {
        detail: format!("the headless worker did not finish: {e}"),
    })?
}

/// Resolved by `PATH`, never by us. See [`run_headless`].
const CLAUDE_PROGRAM: &str = "claude";

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
    Ok(run_headless(root, request).await?)
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
    run_headless(cwd, request).await
}

// --- the CLI version check ----------------------------------------------------------------

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
}

/// Whether the installed CLI is one this build's IDE protocol was ever checked against.
///
/// The probe behind this is latched for the life of the process, so the Settings screen
/// re-reading it costs nothing and the warning reaches the log exactly once however many
/// panes are open — which is the requirement: a per-pane warning is a warning that has been
/// trained out of the reader by the time it means something.
///
/// `async`, with the probe on the blocking pool, and it has to be. A Tauri command that is not
/// `async` runs on the main thread, which here is the GTK loop; the *first* call is the one
/// that misses the latch and actually runs `claude --version`, and `claude` is a Node.js
/// program whose startup is measured in hundreds of milliseconds. Sync, that is every window
/// in the application frozen for the length of a Node boot at the exact moment the user opened
/// Settings — the same defect `session_spawn` and `session_scrollback` were moved off the main
/// thread for, and one that would never reproduce for whoever had already opened Settings once
/// in that session.
#[tauri::command(rename_all = "camelCase")]
pub async fn claude_cli_support() -> ClaudeCliSupport {
    // A join failure means the pool is going away, which is a shutting-down application. The
    // no-CLI answer is the honest one to draw with and this screen must still render.
    tauri::async_runtime::spawn_blocking(cli_support)
        .await
        .unwrap_or_else(|_| ClaudeCliSupport {
            version: None,
            verified_range: cide_claude::version::verified_range(),
            warning: None,
        })
}

/// The verdict, computed synchronously. Separate from the command so a test can call it
/// without an async runtime, and so the command body is only the threading decision.
fn cli_support() -> ClaudeCliSupport {
    let support = cide_claude::version::check_once(Path::new(CLAUDE_PROGRAM));
    ClaudeCliSupport {
        version: support.version().map(str::to_string),
        verified_range: cide_claude::version::verified_range(),
        warning: support.is_warning().then(|| support.message()),
    }
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
    fn a_patch_touches_only_the_fields_it_names() {
        let mut settings = Settings::default();
        settings.terminal.font_size = 17;

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

    /// The full trip a drag makes: patch in, clamped, stored, and the *other* panel's width
    /// still there afterwards.
    #[test]
    fn a_sidebar_patch_is_clamped_and_does_not_disturb_the_other_panel() {
        let mut settings = Settings::default();
        settings.terminal.font_size = 17;

        // What a drag past the right-hand limit sends. The frontend would have clamped it
        // against the viewport already; this asserts the store does not depend on that.
        apply_patch(
            &mut settings,
            SettingsPatch {
                sidebar: Some(SidebarSettings {
                    files_width: 5_000,
                    git_width: 500,
                }),
                ..SettingsPatch::default()
            },
        );
        assert_eq!(settings.sidebar.files_width, SIDEBAR_MAX_WIDTH);
        assert_eq!(
            settings.sidebar.git_width, 500,
            "the git width was rewritten"
        );
        assert_eq!(settings.terminal.font_size, 17, "an untouched group moved");

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

    #[test]
    fn the_cli_support_verdict_carries_the_range_and_warns_only_when_there_is_news() {
        // `claude` may or may not be installed on the machine running this, so the assertion
        // is on the invariants rather than on a version: the range is always populated, and a
        // warning is present exactly when the verdict is one.
        let support = cli_support();
        assert!(!support.verified_range.is_empty());
        let expected = cide_claude::version::check_once(Path::new(CLAUDE_PROGRAM)).is_warning();
        assert_eq!(support.warning.is_some(), expected);
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
