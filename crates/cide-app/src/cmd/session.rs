//! Session commands: spawn a PTY, attach a webview sink to it, write, resize.

use std::path::PathBuf;
use std::sync::Arc;

use cide_ipc::{Geometry, SessionId};
use cide_pty::{Geometry as PtyGeometry, PtySession, Sink, SpawnSpec};
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody, Response};

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

/// Convert a wire geometry into the PTY crate's own, which clamps and derives pixel dims.
fn pty_geometry(g: Geometry) -> PtyGeometry {
    PtyGeometry::new(g.cols, g.rows, g.cell_width, g.cell_height)
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_spawn(
    registry: State<'_, SessionRegistry>,
    program: String,
    args: Vec<String>,
    cwd: String,
    geometry: Geometry,
) -> Result<SessionId, SessionError> {
    let mut spec = SpawnSpec::new(program, PathBuf::from(cwd)).geometry(pty_geometry(geometry));
    for a in args {
        spec = spec.arg(a);
    }
    let session =
        PtySession::spawn(base_env(spec)).map_err(|e| SessionError::Pty(e.to_string()))?;

    let id = SessionId::new();
    registry.insert(id, session);
    Ok(id)
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
