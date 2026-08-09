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
