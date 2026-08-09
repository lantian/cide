//! Session commands: spawn a PTY, attach a webview sink to it, write, resize.

use std::path::PathBuf;
use std::sync::Arc;

use cide_ipc::{Geometry, SessionId};
use cide_pty::{Geometry as PtyGeometry, PtySession, Sink, SpawnSpec};
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{Manager, State};

use crate::state::{AttachmentKey, SessionRegistry};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("no such session")]
    NoSuchSession,
    #[error("{0}")]
    Pty(String),
}

impl serde::Serialize for SessionError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Tagged, so the frontend branches on a variant rather than matching on prose.
        let (kind, message) = match self {
            Self::NoSuchSession => ("noSuchSession", self.to_string()),
            Self::Pty(_) => ("pty", self.to_string()),
        };
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("SessionError", 2)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &message)?;
        st.end()
    }
}

/// Environment every PTY child gets.
///
/// `TERM=xterm-256color` rather than plain `xterm` is not cosmetic: with `xterm` the
/// Claude Code TUI falls back to 8 colours and ASCII box-drawing, and the alternate screen
/// does not engage. Scrubbing `TMUX` matters for the same reason — its presence triggers
/// an unconditional 256-colour clamp that visibly desaturates the accent colour.
///
/// Deliberately absent: `ANTHROPIC_API_KEY`. It outranks subscription OAuth in the
/// credential precedence order, so injecting one would silently bill a Console org for a
/// user on Claude Max. The child inherits its auth by inheriting the environment; cide
/// never reads `~/.claude/.credentials.json`.
fn base_env(spec: SpawnSpec) -> SpawnSpec {
    spec.env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "cide")
        .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
        // xterm.js reports one wheel event per notch, unamplified, which makes the TUI
        // scroll a single line at a time and feel broken.
        .env("CLAUDE_CODE_SCROLL_SPEED", "3")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        // A terminal that inherits stale COLUMNS/LINES lies to the child about its size
        // until the first SIGWINCH.
        .env_remove("COLUMNS")
        .env_remove("LINES")
        .env_remove("CI")
}

/// Whether this program is the Claude Code CLI, and so has hooks worth registering.
///
/// Matched on the file name rather than the whole path: it may be invoked as `claude`, as an
/// absolute path into a version directory, or through a shim.
fn program_is_claude(program: &str) -> bool {
    std::path::Path::new(program)
        .file_name()
        .map(|n| n == "claude")
        .unwrap_or(false)
}

/// The inline `--settings` JSON, or `None` when `cide-hook` cannot be located.
///
/// Looked up beside our own executable, which is where every packaging format this project
/// ships puts the two binaries together.
fn hook_settings() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let hook = exe.parent()?.join("cide-hook");
    if !hook.exists() {
        return None;
    }
    let settings =
        cide_claude::inline_settings(&hook.to_string_lossy(), &cide_claude::StatusLine::Ours);
    serde_json::to_string(&settings).ok()
}

/// Convert a wire geometry into the PTY crate's own, which clamps and derives pixel dims.
fn pty_geometry(g: Geometry) -> PtyGeometry {
    PtyGeometry::new(g.cols, g.rows, g.cell_width, g.cell_height)
}

/// Spawn a child for a pane.
///
/// `resume` continues an existing conversation; `resume` with `fork` branches from it, so
/// the new session shares history up to this point and then diverges while the parent is
/// left untouched. Both are ignored for anything that is not the Claude CLI.
#[tauri::command(rename_all = "camelCase")]
// A Tauri command's parameters are its wire shape: the frontend passes a flat object and the
// macro destructures it. Grouping these into a struct to satisfy the lint would add a type
// that exists only to be immediately taken apart, and would change the JSON the frontend
// sends. `pane_split` carries the same allow for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn session_spawn(
    app: tauri::AppHandle,
    registry: State<'_, SessionRegistry>,
    program: String,
    args: Vec<String>,
    cwd: String,
    geometry: Geometry,
    project: Option<cide_ipc::ProjectId>,
    resume: Option<SessionId>,
    fork: bool,
) -> Result<SessionId, SessionError> {
    let mut spec = SpawnSpec::new(program, PathBuf::from(cwd)).geometry(pty_geometry(geometry));
    for a in args {
        spec = spec.arg(a);
    }
    let mut spec = base_env(spec);
    // Only Claude children get the settings payload. A shell has no hooks to register, and
    // handing it a `--settings` argument would simply be a bad argv.
    let is_claude = program_is_claude(&spec.program);

    // Minted before the spawn, not after, because for a Claude pane this id *is* the value
    // passed to `--session-id`. That equality is what makes everything downstream work: a
    // hook reports the CLI's `session_id`, and unless the CLI was told to use ours, every
    // frame it sends names a uuid this process has never heard of and is dropped. It is also
    // what lets a restored pane resume with `--resume <id>` and no extra bookkeeping.
    let id = SessionId::new();
    if is_claude {
        if let Some(parent) = resume {
            spec = spec.arg("--resume").arg(parent.to_string());
            if fork {
                // Verified against 2.1.226 rather than assumed, because the plan flagged it
                // as unknown and the failure would be silent: `--fork-session` does compose
                // with `--session-id`, the id we pass *is* honoured, and the parent's
                // transcript is left intact beside the fork's own. Both are independently
                // resumable afterwards.
                //
                // This is the operation a terminal multiplexer structurally cannot offer —
                // splitting a conversation to try two approaches from a shared history.
                spec = spec.arg("--fork-session");
            }
        }
        spec = spec.arg("--session-id").arg(id.to_string());
    }

    // `CLAUDE_CODE_SSE_PORT` is load-bearing, not a hint. It makes a port match alone mark our
    // lockfile valid — skipping the cwd-containment, pid-liveness and PID-ancestry checks —
    // and it is what gates the CLI's port-filtered selection, so without it a `claude` here
    // falls back to disambiguating among every lockfile in a shared directory and may bind to
    // another editor entirely. See `cide-ide-mcp::lockfile`.
    //
    // Set for every pane, not only Claude ones: a user who types `claude` into a cide shell
    // should reach this project's server too.
    if let Some(project) = project
        && let Some(servers) = app.try_state::<crate::ide::IdeServers>()
        && let Some(port) = servers.port(project)
    {
        spec = spec.env("CLAUDE_CODE_SSE_PORT", port.to_string());
    }

    // Hooks are what make the status bar's token figures, the fast buffer reload and the
    // busy-vs-idle close confirm possible. They are registered inline via `--settings`
    // rather than by editing `~/.claude/settings.json`, which is the user's file and would
    // otherwise carry cide's hooks into every `claude` they ever run.
    if let Some(server) = app.try_state::<crate::hooks::HookServer>() {
        spec = spec.env(
            "CIDE_HOOK_SOCK",
            server.socket().to_string_lossy().to_string(),
        );

        if is_claude {
            match hook_settings() {
                Some(json) => spec = spec.arg("--settings").arg(json),
                // Without an absolute path to `cide-hook` the child cannot run it: its cwd is
                // the project root and its PATH is the user's. Skipping the flag leaves a
                // working session with no hooks, which is the right way to fail here.
                None => tracing::warn!("cannot locate cide-hook; this session reports no state"),
            }
        }
    }

    let session = PtySession::spawn(spec).map_err(|e| SessionError::Pty(e.to_string()))?;

    // The pid→pane binding is not done here: a session exists before it belongs to a pane,
    // and `pane_bind_session` is the one place that knows both. Binding early would have to
    // invent a pane id and then correct it.

    registry.insert(id, session);
    Ok(id)
}

/// Answer a diff that Claude Code is blocked on.
///
/// The three outcomes are the protocol's, not ours: accepted-with-edits carries the buffer
/// the user actually has on screen (which is why the diff pane reads its editor at click
/// time rather than trusting the proposal it was handed), accepted-as-proposed writes the
/// model's version, and rejected leaves the file alone.
#[tauri::command(rename_all = "camelCase")]
pub fn claude_diff_result(
    app: tauri::AppHandle,
    project: cide_ipc::ProjectId,
    request_id: String,
    outcome: cide_ipc::DiffAnswer,
) -> Result<(), SessionError> {
    let Some(servers) = app.try_state::<crate::ide::IdeServers>() else {
        return Err(SessionError::Pty("the IDE subsystem is not running".into()));
    };
    crate::ide::resolve_or_cancel(&servers, project, &request_id, Some(to_outcome(outcome)));
    Ok(())
}

/// The two documents a diff pane shows.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffContent {
    pub original: String,
    pub proposed: String,
}

/// Fetch the documents for a pending diff.
///
/// Deliberately *not* stored in [`cide_ipc::DiffSpec`]. That struct lives in `Workspace`,
/// which is serialised to `workspace.json` on a debounce — putting `new_file_contents` in it
/// would write the full text of every proposed edit into the user's saved layout, grow the
/// file without bound, and persist file contents long after the diff was answered. The
/// broker already holds the proposal for exactly as long as it is relevant, so the frontend
/// asks for it when it renders the tab and never afterwards.
#[tauri::command(rename_all = "camelCase")]
pub fn claude_diff_content(
    app: tauri::AppHandle,
    project: cide_ipc::ProjectId,
    request_id: String,
) -> Result<DiffContent, SessionError> {
    let servers = app
        .try_state::<crate::ide::IdeServers>()
        .ok_or_else(|| SessionError::Pty("the IDE subsystem is not running".into()))?;
    let broker = servers
        .broker(project)
        .ok_or_else(|| SessionError::Pty("no IDE server for that project".into()))?;

    let request = broker
        .pending()
        .into_iter()
        .find(|r| r.id == request_id)
        .ok_or_else(|| SessionError::Pty("that diff is no longer pending".into()))?;

    // A file the model is creating has no previous version; an empty left side is the
    // honest rendering of that, and is what makes the diff show as all-additions.
    let original = std::fs::read_to_string(&request.params.old_file_path).unwrap_or_default();

    Ok(DiffContent {
        original,
        proposed: request.params.new_file_contents,
    })
}

/// Bridge the wire form to the protocol's own enum.
///
/// Written here rather than in `cide-ipc` because that crate must not depend on the MCP
/// implementation — it is the contract, and the contract cannot know how the contract is served.
fn to_outcome(a: cide_ipc::DiffAnswer) -> cide_ide_mcp::DiffOutcome {
    match a {
        cide_ipc::DiffAnswer::AcceptedEdited { contents } => {
            cide_ide_mcp::DiffOutcome::Saved { contents }
        }
        cide_ipc::DiffAnswer::AcceptedAsIs => cide_ide_mcp::DiffOutcome::TabClosed,
        cide_ipc::DiffAnswer::Rejected => cide_ide_mcp::DiffOutcome::Rejected,
    }
}

/// Attach a webview sink to a session.
///
/// The caller receives the current screen first (see [`session_scrollback`]) so a pane
/// that opens onto an already-running session paints immediately instead of waiting for
/// the child's next output.
#[tauri::command(rename_all = "camelCase")]
pub fn session_attach(
    registry: State<'_, SessionRegistry>,
    window: tauri::Window,
    session: SessionId,
    sink: Channel<InvokeResponseBody>,
    geometry: Geometry,
) -> Result<(), SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    s.resize(pty_geometry(geometry))
        .map_err(|e| SessionError::Pty(e.to_string()))?;

    let sink: Arc<dyn Sink> =
        Arc::new(move |bytes: &[u8]| sink.send(InvokeResponseBody::Raw(bytes.to_vec())).is_ok());
    let id = s.attach(sink);

    registry.record_attachment(
        AttachmentKey {
            session,
            window: window.label().to_string(),
        },
        id,
    );
    Ok(())
}

/// Report that this window has finished processing `bytes` of the session's output.
///
/// Called from `term.write`'s completion callback, which is the only moment xterm has
/// actually parsed the bytes rather than merely received them. Until this arrives the bytes
/// count as outstanding, and a sink that accumulates enough of them stops being sent raw
/// output — see [`cide_pty::CreditPolicy`].
///
/// Silently ignores a session or attachment that has gone away. An ack racing a pane close
/// is ordinary rather than exceptional, and there is nothing useful to tell the caller.
#[tauri::command(rename_all = "camelCase")]
pub fn session_ack(
    registry: State<'_, SessionRegistry>,
    window: tauri::Window,
    session: SessionId,
    bytes: usize,
) {
    let key = AttachmentKey {
        session,
        window: window.label().to_string(),
    };
    if let Some(sink) = registry.attachment(&key)
        && let Some(s) = registry.get(session)
    {
        s.ack(sink, bytes);
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_detach(
    registry: State<'_, SessionRegistry>,
    window: tauri::Window,
    session: SessionId,
) {
    let key = AttachmentKey {
        session,
        window: window.label().to_string(),
    };
    if let Some(sink) = registry.take_attachment(&key)
        && let Some(s) = registry.get(session)
    {
        s.detach(sink);
    }
}

/// The byte sequence that reconstructs the current screen on a fresh terminal.
///
/// Sent as the first frame after attaching. For a fullscreen TUI the frontend follows it
/// with a one-frame `cols-1 → cols` resize nudge, so the application repaints from its own
/// model — covering sequences the screen mirror does not track (OSC 8 hyperlinks, OSC 52
/// clipboard traffic, DEC 2026 synchronized-output framing).
#[tauri::command(rename_all = "camelCase")]
pub fn session_scrollback(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
) -> Result<Response, SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    Ok(Response::new(s.screen_state()))
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_in_alternate_screen(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
) -> Result<bool, SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    Ok(s.in_alternate_screen())
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_write(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
    data: String,
) -> Result<(), SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    s.write(data.into_bytes());
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_resize(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
    geometry: Geometry,
) -> Result<(), SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    s.resize(pty_geometry(geometry))
        .map_err(|e| SessionError::Pty(e.to_string()))
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_has_exited(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
) -> Result<bool, SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    Ok(s.has_exited())
}

/// Every session the registry currently holds a child for.
///
/// The registry, not the workspace tree. That distinction is the whole claim M5 makes: a
/// pane names a session, but the session outlives the pane's position, its tab and its
/// window, and only the registry knows what is actually running.
#[tauri::command(rename_all = "camelCase")]
pub fn session_list(registry: State<'_, SessionRegistry>) -> Vec<SessionId> {
    registry.ids()
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_kill(registry: State<'_, SessionRegistry>, session: SessionId) {
    if let Some(s) = registry.get(session) {
        s.kill();
    }
}
