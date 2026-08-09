//! The event surface.
//!
//! Every `cide://` event goes out through this module and nowhere else, so the set of
//! things the frontend can be told is one file long and `cargo xtask contract-check` has a
//! single place to read it from.
//!
//! # Why this exists at all
//!
//! Until M5 there was one window, and a command could get away with mutating the tree and
//! letting its own caller re-read it. Detaching a pane creates a second window onto the
//! *same* workspace, and from that moment a local re-read is not enough: a pane detached in
//! the shell window has to disappear from the shell window and appear in the new one, and
//! neither can learn that by asking about its own last command.
//!
//! Every event carries `rev`. The frontend keeps the highest it has seen and drops anything
//! older, because two windows mutating one tree means snapshots can arrive out of order.

use cide_ipc::Workspace;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Sent after every accepted mutation, to every window.
pub const WORKSPACE_CHANGED: &str = "cide://workspace-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceChanged {
    rev: u64,
    workspace: Workspace,
}

/// Broadcast the workspace to every window.
///
/// Sends the whole tree rather than a patch. The tree is small — a workspace with three
/// projects and a dozen panes serialises to a few kilobytes — and a patch protocol between
/// two windows that can both mutate is a source of divergence for no measurable gain.
pub fn workspace_changed(app: &AppHandle, workspace: &Workspace) {
    let payload = WorkspaceChanged {
        rev: workspace.rev,
        workspace: workspace.clone(),
    };
    // A failed emit means a window went away mid-broadcast, which is normal during a close
    // and not worth propagating into the command that triggered it.
    if let Err(error) = app.emit(WORKSPACE_CHANGED, payload) {
        tracing::debug!(%error, "workspace broadcast reached no window");
    }
}

// --- session events ---------------------------------------------------------------------
//
// These do not carry the workspace, and deliberately not `rev` either: they say nothing
// about the tree. A session changing state does not reorder a tab or move a pane, so
// broadcasting a revision with them would make every window re-hydrate its whole workspace
// on every tool call — several times a second during an active turn.

/// A session moved between idle, busy, awaiting permission, or exited.
pub const SESSION_STATE: &str = "cide://session-state";

/// Live model, token and cost figures from the statusline.
///
/// The statusline is the only supported source for these. Nothing else the CLI exposes
/// reports token usage as it accrues.
pub const SESSION_STATUS: &str = "cide://session-status";

/// A tool touched files. The fast path for reloading an open editor buffer.
///
/// Separate from the `notify` watcher rather than replacing it: this arrives sooner and
/// names the file exactly, but only covers edits the CLI made through its own tools. A
/// `sed -i` in a shell pane, or `cargo fmt`, is still the watcher's job.
pub const SESSION_TOOL: &str = "cide://session-tool";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionStateChanged {
    session: String,
    state: cide_ipc::SessionState,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionStatus {
    session: String,
    /// The statusline's own JSON, forwarded whole.
    ///
    /// Not destructured into typed fields here. Its shape is the CLI's and is not versioned;
    /// a release that adds a figure should reach the status bar rather than be dropped by a
    /// deserialiser that had not heard of it.
    status: serde_json::Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionTool {
    session: String,
    paths: Vec<String>,
}

pub fn session_state(app: &AppHandle, session: &str, state: cide_ipc::SessionState) {
    let payload = SessionStateChanged {
        session: session.to_owned(),
        state,
    };
    if let Err(error) = app.emit(SESSION_STATE, payload) {
        tracing::debug!(%error, "session-state reached no window");
    }
}

pub fn session_status(app: &AppHandle, session: &str, status: serde_json::Value) {
    let payload = SessionStatus {
        session: session.to_owned(),
        status,
    };
    if let Err(error) = app.emit(SESSION_STATUS, payload) {
        tracing::debug!(%error, "session-status reached no window");
    }
}

pub fn session_tool(app: &AppHandle, session: &str, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let payload = SessionTool {
        session: session.to_owned(),
        paths,
    };
    if let Err(error) = app.emit(SESSION_TOOL, payload) {
        tracing::debug!(%error, "session-tool reached no window");
    }
}

// --- git events ---------------------------------------------------------------------------

/// The changes tree for one project was recomputed.
///
/// Carries the whole tree rather than a delta, for the same reason `workspace_changed` does:
/// the commit tool window is a tri-state tree where moving one file changes the state of every
/// group above it, and a patch protocol between two windows that can both mutate is a source
/// of divergence for no measurable gain. It does not carry `rev` — a commit touches no part of
/// the workspace tree, and broadcasting a revision here would make every window re-hydrate its
/// whole workspace on every `git add`.
pub const GIT_STATUS: &str = "cide://git-status";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitStatus {
    project: cide_ipc::ProjectId,
    tree: cide_ipc::git::ChangesTree,
}

pub fn git_status(
    app: &AppHandle,
    project: cide_ipc::ProjectId,
    tree: &cide_ipc::git::ChangesTree,
) {
    let payload = GitStatus {
        project,
        tree: tree.clone(),
    };
    if let Err(error) = app.emit(GIT_STATUS, payload) {
        tracing::debug!(%error, "git-status reached no window");
    }
}
