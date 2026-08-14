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

/// Which sessions have finished processing and are waiting for the user.
///
/// The whole set, every time, and to every window. Two reasons it is not a per-session delta:
///
/// * a window that opened *after* a session started waiting has no history to derive the
///   answer from — `cide://session-state` carries transitions, and it heard none of them — so
///   the only thing that can paint its markers correctly is the complete set; and
/// * the set is tiny. It is the number of Claude panes that are waiting, which is a handful.
///
/// It is still only a *change* notification, which is the half this cannot do on its own: a
/// window that opens between two changes hears nothing, and a freshly detached window is
/// exactly that. It asks instead, once, through `window_awaiting_sessions`.
///
/// The decision about membership is *not* made here. "Finished a turn and is waiting" needs a
/// bit of history that `SessionState` does not carry (see `ui/src/panes/awaitingRule.ts`), so
/// the frontend reports it and `windows.rs` aggregates. This event is Rust telling every
/// window what it now believes, which is what keeps three windows showing one answer.
pub const SESSION_AWAITING: &str = "cide://session-awaiting";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionAwaiting {
    sessions: Vec<cide_ipc::SessionId>,
}

pub fn session_awaiting(app: &AppHandle, sessions: Vec<cide_ipc::SessionId>) {
    if let Err(error) = app.emit(SESSION_AWAITING, SessionAwaiting { sessions }) {
        tracing::debug!(%error, "session-awaiting reached no window");
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

// --- file system events (M8) ------------------------------------------------------------
//
// No `rev` either, and for the same reason: a file changing on disk moves no pane. The file
// tree and the open editors listen; nothing else has to.

/// One coalesced burst of filesystem change, already filtered through the walk's ignore
/// rules. A `cargo build` is one of these, not fifty thousand.
pub const FS_CHANGED: &str = "cide://fs-changed";

/// A project's index or watcher changed state: indexing started or finished, or the watcher
/// degraded to polling and the banner has to say why.
pub const FS_STATUS: &str = "cide://fs-status";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsChanged {
    project: cide_ipc::ProjectId,
    change: cide_ipc::FsChange,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsStatusChanged {
    project: cide_ipc::ProjectId,
    status: cide_ipc::FsStatus,
}

pub fn fs_changed(app: &AppHandle, project: cide_ipc::ProjectId, change: &cide_ipc::FsChange) {
    let payload = FsChanged {
        project,
        change: change.clone(),
    };
    if let Err(error) = app.emit(FS_CHANGED, payload) {
        tracing::debug!(%error, "fs-changed reached no window");
    }
}

pub fn fs_status(app: &AppHandle, project: cide_ipc::ProjectId, status: &cide_ipc::FsStatus) {
    let payload = FsStatusChanged {
        project,
        status: status.clone(),
    };
    if let Err(error) = app.emit(FS_STATUS, payload) {
        tracing::debug!(%error, "fs-status reached no window");
    }
}

// --- the keymap ---------------------------------------------------------------------------

/// The user's keybindings changed. Sent to **every** window after Settings → Keymap writes.
///
/// # Why this is not `workspace_changed`
///
/// The keymap is not in the workspace. `app_get_bootstrap` resolves it from
/// `~/.config/cide/keymap.json` on every call, which is what makes a hand-edited file take
/// effect in the next window that hydrates for any reason at all — and there are three dozen
/// reasons. But `store/workspace.ts::applySnapshot` builds `{ ...current, workspace }`, so it
/// keeps the *old* `keymap` array: `cide://workspace-changed`, the one thing already broadcast
/// to every window, is precisely the path that cannot refresh a binding. Without this event a
/// rebind reaches the window that made it and no other, and the second window keeps the old
/// chord until something makes it hydrate — which is the "my keybinding does nothing, with no
/// visible cause" failure `cide_core::keymap` exists to prevent, arriving a window later.
///
/// Carries the whole resolved table rather than a signal to re-fetch, on `workspace_changed`'s
/// own argument: it is a few kilobytes, the round trip is already paid for, and a window that
/// answered by re-reading would be resolving a file that may have changed again. The array
/// identity matters as much as its contents — `keys/gate.ts` caches its indexed keymap against
/// the identity of what `bindings()` returns, so a fresh array is what makes it rebuild.
///
/// No `rev`: the keymap is not the tree, and a revision here would make every window
/// re-hydrate its whole workspace because somebody changed a shortcut.
pub const KEYMAP_CHANGED: &str = "cide://keymap-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeymapChanged {
    keymap: Vec<cide_ipc::ResolvedBinding>,
}

pub fn keymap_changed(app: &AppHandle, keymap: &[cide_ipc::ResolvedBinding]) {
    let payload = KeymapChanged {
        keymap: keymap.to_vec(),
    };
    if let Err(error) = app.emit(KEYMAP_CHANGED, payload) {
        tracing::debug!(%error, "keymap broadcast reached no window");
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

// --- diagnostics (M12) ---------------------------------------------------------------------

/// A project's merged diagnostics changed.
///
/// Carries no payload beyond the project id, and no `rev`. Both are deliberate.
///
/// **No payload**, because the snapshot depends on settings the emitter would have to read and
/// the receiver already has: the frontend answers with `diagnostics_get`, which projects the
/// store through the *current* `InspectionSettings`. Pushing a snapshot would mean pushing one
/// per window whenever a filter changed, for a surface that is usually closed.
///
/// **No `rev`**, for the reason `git_status` carries none: a diagnostic moves no pane, and a
/// revision here would re-hydrate every window's whole workspace on every `publishDiagnostics`.
///
/// **Coalesced before it gets here.** rust-analyzer publishes per file and a workspace check is
/// hundreds of publishes in a burst; ADR 0003's rule about the GTK main loop applies to this the
/// same way it applies to terminal output. See `crate::lsp`'s 250 ms debounce and 1 s ceiling —
/// that coalescing is a correctness requirement, not tuning.
pub const DIAGNOSTICS: &str = "cide://diagnostics";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostics {
    project: cide_ipc::ProjectId,
}

pub fn diagnostics(app: &AppHandle, project: cide_ipc::ProjectId) {
    if let Err(error) = app.emit(DIAGNOSTICS, Diagnostics { project }) {
        tracing::debug!(%error, "diagnostics reached no window");
    }
}

/// A window-manager close was refused because it would discard unsaved work.
///
/// The only path that needs an event rather than an error. Alt+F4 and the compositor's own
/// close button never reach a command, so there is no call whose `Err` the frontend could
/// catch — the refusal has to travel the other way. Everything else (`tab_close`,
/// `project_close`, `window_close`) refuses in its return value, which is stronger: a
/// command that is never issued cannot lose anything.
pub const CLOSE_BLOCKED: &str = "cide://close-blocked";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloseBlocked {
    window: cide_ipc::WindowLabel,
    unsaved: Vec<cide_ipc::UnsavedTab>,
}

pub fn close_blocked(
    app: &AppHandle,
    window: &cide_ipc::WindowLabel,
    unsaved: Vec<cide_ipc::UnsavedTab>,
) {
    let payload = CloseBlocked {
        window: window.clone(),
        unsaved,
    };
    if let Err(error) = app.emit(CLOSE_BLOCKED, payload) {
        // If no window is listening the close is about to happen anyway; there is nothing
        // useful to do and nothing to warn about.
        tracing::debug!(%error, "close-blocked reached no window");
    }
}

// --- mouse navigation (M12) ---------------------------------------------------------------

/// A thumb button (GDK 8 or 9) was pressed in one window.
///
/// # Why this has to come from Rust at all
///
/// The webview cannot tell Back from Forward. WebKitGTK's `buttonForEvent`
/// (`Source/WebKit/Shared/gtk/WebEventFactory.cpp`) maps GDK buttons 1, 2 and 3 and leaves
/// everything else at `WebMouseEventButton::None`; `MouseEvent`'s constructor then turns `None`
/// into `Left`. So both thumb buttons arrive in JavaScript as `button === 0`, `buttons === 0`,
/// indistinguishable from each other and from an ordinary click. A DOM-only implementation is
/// impossible rather than merely awkward — and, worse, the phantom left click is *live today*:
/// a Ctrl+thumb-press over a path in terminal output already opens the file, and over a buffer
/// already fires Go to definition. Swallowing the press in GTK fixes that as a side effect.
///
/// # Why `emit_to` and not a broadcast
///
/// The press happened in one window. `workspace_changed` goes to every window because the tree
/// is shared; a mouse gesture is not, and two windows both walking their own history from one
/// press is the kind of bug that reads as "Back sometimes goes back twice".
///
/// # Why the payload names a button and not a command
///
/// `ui/src/keys/gate.ts` is the one place input becomes a command, and `mouseback` /
/// `mouseforward` are ordinary keymap entries there. Naming a command here would be a second
/// resolver, with its own copy of the prefix machine and the `when` evaluation.
pub const MOUSE_NAV: &str = "cide://mouse-nav";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MouseNav {
    /// Which window the press happened in.
    ///
    /// **Carried in the payload because `emit_to` cannot narrow this event.** Tauri's
    /// `emit_to(label, ..)` filters by `EventTarget`, and `@tauri-apps/api`'s `listen()`
    /// registers `EventTarget::Any` — whose arm in `match_any_or_filter` short-circuits past
    /// every target filter. So the emit fans out to every webview regardless of the label,
    /// and with two shell windows open one thumb press ran Back in both, each walking its own
    /// history and stealing focus from the other.
    ///
    /// The alternative was `emit_to` with an explicit `EventTarget::Webview`, which would
    /// mean changing how *every* listener in `client.ts` registers — a wide change to fix one
    /// event. Filtering on a field the receiver already knows how to check (`windowLabel()`)
    /// keeps the blast radius at this event.
    window: String,
    /// `mouseback` or `mouseforward` — the key token, not a command.
    button: &'static str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
}

pub fn mouse_nav(
    app: &AppHandle,
    window: &str,
    button: &'static str,
    modifiers: (bool, bool, bool, bool),
) {
    let (ctrl, alt, shift, meta) = modifiers;
    let payload = MouseNav {
        window: window.to_string(),
        button,
        ctrl,
        alt,
        shift,
        meta,
    };
    // Still `emit_to` rather than `emit`: it is the honest statement of intent, it narrows
    // delivery for any listener that ever registers a real target, and the payload's `window`
    // is what actually enforces it today.
    if let Err(error) = app.emit_to(window, MOUSE_NAV, payload) {
        tracing::debug!(%error, window, "mouse-nav reached no window");
    }
}

// --- diagnostics ------------------------------------------------------------------------

/// The IPC transport is on the slow path.
///
/// Promised by `cmd/diag.rs` since M0 and never actually sent, which meant the one failure
/// mode the M0 instrumentation exists to catch was the one nothing could report. It matters
/// because the degradation is *silent and permanent*: `ipc-protocol.js` catches a rejection
/// from the custom-protocol fetch, sets `customProtocolIpcFailed = true` for the life of the
/// page, and from then on every payload is JSON-encoded and `eval`'d. Nothing crashes. The app
/// simply becomes slow, which is precisely the report a user files as "typing is laggy" —
/// with no way for anyone to tell that from a renderer problem or a PTY problem.
///
/// Broadcast to every window rather than answered to the caller: the probe runs in one window
/// and the transport is the process's.
pub const IPC_DEGRADED: &str = "cide://ipc-degraded";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct IpcDegraded {
    health: cide_ipc::IpcHealth,
}

pub fn ipc_degraded(app: &AppHandle, health: &cide_ipc::IpcHealth) {
    let payload = IpcDegraded {
        health: health.clone(),
    };
    if let Err(error) = app.emit(IPC_DEGRADED, payload) {
        tracing::debug!(%error, "ipc-degraded reached no window");
    }
}
