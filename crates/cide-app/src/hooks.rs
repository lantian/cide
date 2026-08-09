//! The unix socket every `claude` child's hooks report to.
//!
//! # Shape
//!
//! One listener per process, at `$XDG_RUNTIME_DIR/cide-hooks-<pid>.sock`. Every child is
//! spawned with `CIDE_HOOK_SOCK` pointing at it and an inline `--settings` registering
//! `cide-hook <Event>` for all ten hook points. A hook connects, writes one newline-delimited
//! JSON frame, and disconnects.
//!
//! # Connection per frame, on purpose
//!
//! Each hook invocation is a fresh short-lived process, so a persistent connection is not
//! available to it even in principle. That makes the accept loop trivial and — more usefully
//! — means a hook that dies mid-write costs one truncated line that this reader discards,
//! rather than desynchronising a stream that later frames share.
//!
//! # Blocking threads rather than the async runtime
//!
//! `ide.rs` already owns a tokio runtime for the MCP server, but this listener has no reason
//! to share it: the work per frame is a JSON parse and a map lookup, connections are
//! short-lived, and putting it on OS threads keeps a wedged hook from occupying a runtime
//! worker that the IDE server needs to answer a blocked `openDiff`.

use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use cide_claude::{HookEvent, HookFrame};
use cide_ipc::{SessionId, SessionState};
use dashmap::DashMap;
use tauri::AppHandle;

/// The socket, and every session's last known state.
pub struct HookServer {
    path: PathBuf,
    /// Last known state per session. The state machine needs a "current" to transition from,
    /// and a session with no entry yet is treated as `Spawning`.
    states: Arc<DashMap<SessionId, SessionState>>,
}

impl HookServer {
    /// Bind the socket and start accepting.
    ///
    /// Failure costs hooks — no live token figures, no fast buffer reload, and a close
    /// confirm that cannot tell busy from idle — but not the terminal. Claude Code runs
    /// perfectly well with no hooks configured, so this degrades rather than refusing.
    pub fn start(app: AppHandle) -> std::io::Result<Self> {
        let path = socket_path();

        // A socket left by a previous run of this same pid would make `bind` fail with
        // AddrInUse. Removing it is safe: the path carries our pid, so nothing else owns it.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path)?;
        // The socket is a control channel into this process. 0700 on the parent directory is
        // not guaranteed (XDG_RUNTIME_DIR usually is, /tmp is not), so the socket itself is
        // narrowed rather than trusted to inherit.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        let states: Arc<DashMap<SessionId, SessionState>> = Arc::new(DashMap::new());
        let accept_states = Arc::clone(&states);

        thread::Builder::new()
            .name("cide-hook-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let app = app.clone();
                    let states = Arc::clone(&accept_states);
                    // One thread per connection, each one short-lived. A hook that connects
                    // and never writes therefore blocks only itself.
                    let _ = thread::Builder::new()
                        .name("cide-hook-conn".into())
                        .spawn(move || handle(stream, &app, &states));
                }
            })?;

        tracing::info!(path = %path.display(), "hook socket listening");
        Ok(Self { path, states })
    }

    /// The value for a child's `CIDE_HOOK_SOCK`.
    pub fn socket(&self) -> &std::path::Path {
        &self.path
    }

    pub fn state(&self, session: SessionId) -> SessionState {
        self.states
            .get(&session)
            .map(|s| *s)
            .unwrap_or(SessionState::Spawning)
    }

    /// Sessions the user would mind interrupting.
    ///
    /// `Busy | AwaitingPermission`, not "a process exists" — see `cide_claude::state`. This
    /// is what the close confirm asks, and a definition that warned about every running
    /// session would train people to dismiss the dialog.
    pub fn live_sessions(&self) -> Vec<SessionId> {
        self.states
            .iter()
            .filter(|e| e.value().is_live())
            .map(|e| *e.key())
            .collect()
    }

    /// Forget a session that has gone.
    pub fn forget(&self, session: SessionId) {
        self.states.remove(&session);
    }
}

impl Drop for HookServer {
    fn drop(&mut self) {
        // A socket file outliving its process is a path children would connect to and write
        // into with nothing on the other end.
        let _ = std::fs::remove_file(&self.path);
    }
}

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("cide-hooks-{}.sock", std::process::id()))
}

fn handle(stream: UnixStream, app: &AppHandle, states: &DashMap<SessionId, SessionState>) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        // A frame we cannot parse is dropped with a note rather than killing the connection:
        // a CLI release that changes a payload should cost one event, not the hook channel.
        match serde_json::from_str::<HookFrame>(&line) {
            Ok(frame) => apply(&frame, app, states),
            Err(error) => tracing::debug!(%error, "unparseable hook frame"),
        }
    }
}

/// Route one frame: update state, emit what the frontend needs.
fn apply(frame: &HookFrame, app: &AppHandle, states: &DashMap<SessionId, SessionState>) {
    // The statusline is not a hook event and carries no `session_id` in the same shape; it is
    // handled first so it does not fall through the state machine.
    if frame.event == "statusline" {
        if let Some(session) = frame.session_id() {
            crate::emit::session_status(app, session, frame.payload.clone());
        }
        return;
    }

    let Some(event) = frame.kind() else {
        tracing::debug!(event = %frame.event, "hook event this build does not know");
        return;
    };

    let Some(raw) = frame.session_id() else {
        return;
    };
    let Ok(session) = raw.parse::<SessionId>() else {
        // A session uuid we did not mint. Reachable when a user runs `claude` by hand inside
        // a cide shell: it inherits `CIDE_HOOK_SOCK` and reports here, but no pane owns it.
        tracing::debug!(session = raw, "hook from a session this app does not own");
        return;
    };

    // Files first, so a reload is not held up behind a state transition that may not happen.
    if matches!(event, HookEvent::PostToolUse | HookEvent::PostToolBatch) {
        crate::emit::session_tool(app, raw, frame.touched_paths());
    }

    let current = states
        .get(&session)
        .map(|s| *s)
        .unwrap_or(SessionState::Spawning);

    // A permission request is a `Notification` whose text says so; the state machine cannot
    // see the payload, so that one case is decided here.
    let next = if event == HookEvent::Notification {
        cide_claude::is_permission_request(&frame.payload)
            .then_some(SessionState::AwaitingPermission)
            .filter(|s| *s != current)
    } else {
        cide_claude::next_state(current, event)
    };

    if let Some(next) = next {
        states.insert(session, next);
        crate::emit::session_state(app, raw, next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_path_names_this_process() {
        // Two cide processes must not fight over one socket, and a stale socket from a
        // previous run must be identifiable as removable.
        let p = socket_path();
        let name = p
            .file_name()
            .expect("has a name")
            .to_string_lossy()
            .into_owned();
        assert!(name.contains(&std::process::id().to_string()));
        assert!(name.ends_with(".sock"));
    }

    #[test]
    fn an_unknown_session_is_ignored_rather_than_tracked() {
        // A `claude` a user starts by hand in a cide shell inherits CIDE_HOOK_SOCK and
        // reports here. It has no pane, so it must not appear in `live_sessions` and block a
        // close confirm on something the user never opened.
        let states: DashMap<SessionId, SessionState> = DashMap::new();
        assert!("not-a-uuid".parse::<SessionId>().is_err());
        assert!(states.is_empty());
    }
}
