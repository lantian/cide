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
    // The revision only. See the tee's own header.
    let rev = workspace.rev;
    tee(|| cide_remote::RemoteEvent::WorkspaceRev { rev });
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

/// The last state [`session_state`] announced for each session.
///
/// Kept for the remote surface, which has to answer *what is every session doing* for a device
/// that heard none of the transitions. The hook server's own map cannot answer it: it holds only
/// what hooks said, so a shell's jobs, an exit and a pause are not in it, and the remote read
/// only its *live* subset (`Busy`/`AwaitingPermission`) — every finished turn went out as
/// `Spawning`, which a phone draws as nothing at all. This is the one place every transition
/// passes, so it is the one record that agrees with what the windows were told.
///
/// Written **before** the event is emitted or teed, and that order is load-bearing:
/// `cide_remote`'s `Ordered` replays only what arrived after a snapshot began, on the promise
/// that anything earlier is already visible to the snapshot's read.
fn last_states()
-> &'static parking_lot::Mutex<std::collections::HashMap<String, cide_ipc::SessionState>> {
    static LAST: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, cide_ipc::SessionState>>,
    > = std::sync::OnceLock::new();
    LAST.get_or_init(Default::default)
}

/// Every session's last announced state. See [`last_states`].
pub fn last_session_states()
-> std::collections::HashMap<cide_ipc::SessionId, cide_ipc::SessionState> {
    last_states()
        .lock()
        .iter()
        .filter_map(|(session, state)| Some((session.parse().ok()?, *state)))
        .collect()
}

pub fn session_state(app: &AppHandle, session: &str, state: cide_ipc::SessionState) {
    last_states().lock().insert(session.to_owned(), state);
    let payload = SessionStateChanged {
        session: session.to_owned(),
        state,
    };
    if let Err(error) = app.emit(SESSION_STATE, payload) {
        tracing::debug!(%error, "session-state reached no window");
    }
    if let Ok(session) = session.parse::<cide_ipc::SessionId>() {
        tee(|| cide_remote::RemoteEvent::SessionState { session, state });
    }
    // The single funnel for "a session moved", and the reason the header's badge is marked from
    // here rather than from each emitter. There are five of those today — `hooks::apply`,
    // `lifecycle::watch_jobs`, `lifecycle::report_exit` and the pause/thaw pair in `agents.rs` —
    // and a sixth is the ordinary way a feature like this goes quietly stale.
    //
    // `mark` is one mutex and two `Option<Instant>`s, which it has to stay: the hook applier is
    // a single ordered thread whose ordering is a correctness requirement, and this is on it.
    crate::running::mark(app);
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
    // Read back rather than derived from the argument: the wire shape three windows parse is a
    // list of ids, and a device needs each entry's arrival stamp to tell news from what it has
    // already announced. Widening the event to carry both would put a field on a payload nobody
    // on that wire reads. The set's lock was released before this call.
    tee(|| cide_remote::RemoteEvent::Awaiting {
        entries: crate::windows::awaiting_since(),
    });
}

/// What is *working* in each project — the header project tab's badge. (M94)
///
/// The whole set, every time, to every window, on [`SESSION_AWAITING`]'s two reasons one level
/// up: a window that opened after a run started has no history to derive it from, and the set is
/// one small entry per open project. Projects with nothing running are omitted, so absence is
/// the message.
///
/// Unlike the awaiting set, **Rust decides membership here as well as aggregating it**. A count
/// of runs and console panes needs the agent registry, the session registry and the hook states,
/// none of which a webview has, and needs them for projects that window is not drawing — which
/// is the whole point of the badge. `cide_app::running`'s header carries the argument.
///
/// Still only a *change* notification, so `cmd::project::project_running_counts` is the other
/// half, and `crate::running::flush` drops a recomputation that moved nothing rather than
/// broadcasting it.
///
/// Not teed to a device: a phone has no project tab strip, and the facts behind this number
/// reach it through `RemoteEvent::SessionState` already.
pub const PROJECT_RUNNING: &str = "cide://project-running";

pub fn project_running(app: &AppHandle, set: &cide_ipc::ProjectRunningSet) {
    if let Err(error) = app.emit(PROJECT_RUNNING, set) {
        tracing::debug!(%error, "project-running reached no window");
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
/// effect in the next window that hydrates — and hydrating is a boot-and-refresh event now,
/// not a per-gesture one, so this event is the only road a rebind has into a live window.
/// `store/workspace.ts::applySnapshot` builds `{ ...current, workspace }`, so it
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

/// The set of imported colour schemes changed. (M24)
///
/// Exists for `KEYMAP_CHANGED`'s reason, one field along: the schemes ride `Bootstrap`, and
/// `store/workspace.ts::applySnapshot` rebuilds `{ ...current, workspace }` — so
/// `cide://workspace-changed`, which is what carries the *setting* naming a scheme, is exactly
/// the path that cannot carry the scheme itself. Without this event, importing in one window
/// changes the setting everywhere and the palette in one place: every other window resolves the
/// new id against a list that does not contain it and falls back to the builtin, which reads as
/// "the import did not work" in the window the user was not looking at.
///
/// Carries the whole list, not a delta. It is a handful of small maps, the round trip is already
/// paid for, and a window answering by re-reading the directory would be reading a state that
/// may have moved again.
pub const SCHEMES_CHANGED: &str = "cide://schemes-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemesChanged {
    schemes: Vec<cide_ipc::ColorScheme>,
}

pub fn schemes_changed(app: &AppHandle, schemes: Vec<cide_ipc::ColorScheme>) {
    if let Err(error) = app.emit(SCHEMES_CHANGED, SchemesChanged { schemes }) {
        tracing::debug!(%error, "colour scheme broadcast reached no window");
    }
}

// --- capabilities -------------------------------------------------------------------------

/// The configured Claude CLI changed, so what this build can do changed with it.
/// Sent to **every** window after Settings → Claude writes a new binary path.
///
/// # Why this is not `workspace_changed`, on `KEYMAP_CHANGED`'s exact argument
///
/// Capabilities are not in the workspace — they ride `Bootstrap`, and
/// `store/workspace.ts::applySnapshot` builds `{ ...current, workspace }`, keeping the old
/// struct. The gesture-time hydrates that used to refresh it opportunistically are gone, so
/// without this event a binary changed in one window would leave every other window's header
/// naming the old CLI version until reload. Carries the whole struct rather than a signal to
/// re-fetch, and no `rev`, both for `KEYMAP_CHANGED`'s reasons.
pub const CAPABILITIES_CHANGED: &str = "cide://capabilities-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilitiesChanged {
    capabilities: cide_ipc::Capabilities,
}

pub fn capabilities_changed(app: &AppHandle, capabilities: cide_ipc::Capabilities) {
    if let Err(error) = app.emit(CAPABILITIES_CHANGED, CapabilitiesChanged { capabilities }) {
        tracing::debug!(%error, "capabilities broadcast reached no window");
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
/// Because **wry gets the press first and spends it on `window.history.back()`**, and taking it
/// away from wry can only be done in GTK. `wry-0.55.1/src/webkitgtk/synthetic_mouse_events.rs`
/// connects a `button-press-event` handler to the WebKitWebView during `create_webview`; for
/// buttons 8 and 9 it stops the emission and injects a synthetic DOM `mousedown` with `button: 3`
/// / `button: 4`, then calls `history.back()`/`.forward()` on the mouseup unless something
/// cancelled it. In an SPA with one history entry that is a silent no-op. `windows.rs`'s
/// `install_mouse_nav` carries the full account, including why the handler is on the generic
/// `event` signal and not on `button-press-event`.
///
/// This entry used to claim that WebKitGTK's `buttonForEvent` flattens both thumb buttons to
/// `button === 0` in the DOM, and that a "phantom left click" was live. Neither is true: wry
/// intercepts before WebKit's event factory runs, so what the DOM would see is `button` 3 or 4,
/// and `pathLinks.ts`'s gate rejects any `ev.button !== 0`. The correction is recorded rather
/// than quietly deleted because the false premise is what stopped anyone looking at wry.
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

// --- the task tracker (M18) ------------------------------------------------------------

/// One project's `.cide/tasks.json` changed.
///
/// # Why this one carries `rev`, when the two events beside it carry none
///
/// `git_status` and `diagnostics` deliberately carry no revision, and the argument is the same
/// for both: they describe something outside the workspace tree, and a revision on them would
/// make every window re-hydrate its whole workspace because somebody ran `git add`. That
/// argument does not reach this event, because `rev` here is not being used to order the *tree*
/// — it is being used to order **this file against itself**.
///
/// And it has to be. Every other broadcast in this module describes something with exactly one
/// writer in one process: the workspace behind a mutex, a `Repository` opened per call, a
/// language server's publish stream. `.cide/tasks.json` genuinely has several — two cide windows
/// can both edit it, the project's agents write it through the MCP socket, and a `git pull` or a
/// teammate can move it on disk with nobody in this process involved at all. So two snapshots
/// can reach one window in the opposite order to the one they were produced in, and the
/// receiver's only defence is to keep the highest revision it has seen and drop anything that is
/// not newer.
///
/// That is `cide://workspace-changed`'s situation exactly, so it gets `workspace_changed`'s
/// answer rather than a new one — carry the revision, carry the whole board, let the receiver
/// discard what it has already passed.
///
/// **This `rev` is the task file's, not the workspace's.** They are unrelated counters over
/// unrelated files, and feeding this one to `ui/src/store/workspace.ts` — whose `applySnapshot`
/// drops anything not newer than the workspace revision it holds — would either wedge the
/// workspace mirror shut or make it accept a stale tree, depending only on which file had seen
/// more edits. The Tasks panel keeps its own high-water mark.
///
/// The whole board rather than a signal to re-ask, on `workspace_changed`'s own argument: the
/// emitter has it in hand, and a window that answered by re-fetching would be reading a file that
/// may have changed again in between.
///
/// # What the board may carry, and the measurement that decided it (M68)
///
/// This doc used to finish that argument with *"it is a few kilobytes"*, and for two years it was
/// true. It stopped being true without anything failing: `.cide/tasks.json` on a four-month-old
/// project reached **2.19 MB** across 126 tasks, of which the comment text alone was 77.5%, and
/// the board carried every word of it.
///
/// That matters here rather than anywhere else because of how an event is delivered. `app.emit`
/// serialises the payload and hands it to `tauri`'s `event::emit_js_script`, which **inlines it
/// into a JavaScript source string** and `eval`s that string in every webview, on the GTK main
/// loop. So the cost is not a JSON parse, it is parsing two megabytes as *source code*, per
/// window, on every mutation — and a dispatched run writes a comment per tool call. Measured on
/// that board: 10.2 ms as JS source against 2.9 ms via `JSON.parse`, in V8; WebKitGTK is slower and
/// it is blocking paint while it does it. This is the same failure class `cide-pty`'s coalescer
/// exists for, where a `Channel` payload under 1024 bytes goes through `webview.eval` — and it is
/// why the ceiling on that coalescer is a correctness requirement rather than tuning.
///
/// So the board carries [`cide_ipc::TaskRow`]: the fields a *list* draws, plus a count of comments
/// and of attachments. A body, a log, a history and a task's file records are fetched by id through
/// `task_get` when something opens one, and searched through `task_search` where they live.
///
/// **Nothing may put content back in here.** The pressure to will come from a panel that wants one
/// more field, and the honest answer is a second command rather than a wider broadcast: this
/// payload is multiplied by every open window and by every comment every agent writes.
pub const TASKS_CHANGED: &str = "cide://tasks-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TasksChanged {
    project: cide_ipc::ProjectId,
    /// The task file's revision — see the constant's doc. Present for every board variant,
    /// including `Absent` and `Unreadable`, which carry none of their own: a snapshot that
    /// cannot be ordered is a snapshot the receiver cannot safely drop.
    rev: u64,
    board: cide_ipc::TaskBoard,
}

pub fn tasks_changed(
    app: &AppHandle,
    project: cide_ipc::ProjectId,
    rev: u64,
    board: &cide_ipc::TaskBoard,
) {
    let payload = TasksChanged {
        project,
        rev,
        board: board.clone(),
    };
    // The project and nothing else. The board itself may not cross that wire — it is rows now,
    // but the rule is about the *shape of the promise*, not today's size — so a device is told
    // its board moved and re-reads a projection. (M75)
    tee(|| cide_remote::RemoteEvent::TasksChanged { project });
    if let Err(error) = app.emit(TASKS_CHANGED, payload) {
        tracing::debug!(%error, "tasks-changed reached no window");
    }
}

// --- subagent orchestration (M18) --------------------------------------------------------

/// One project's subagent roster changed — its config, its role definitions, or both.
///
/// # Why this one carries no `rev`, when the event directly above it does
///
/// `tasks-changed` carries a revision because `.cide/tasks.json` genuinely has several writers:
/// two cide windows can both edit it, the project's agents write it over the RPC socket, and a
/// `git pull` can move it with nobody in this process involved. Two snapshots can therefore reach
/// one window in the opposite order to the one they were produced in, and the receiver's only
/// defence is a high-water mark.
///
/// None of that is true here. A roster is **derived**, not stored: one in-process read of
/// `.cide/config.json` and `.cide/agents/*.md`, plus a run registry that has exactly one writer
/// in one process. Every emit is built immediately after the change that caused it, from the
/// state that change produced, so the last one sent is by construction the newest — there is
/// nothing for an ordering number to protect. A receiver that dropped a roster for being "older"
/// would be discarding the only answer it was ever going to get.
///
/// A revision would also cost something real. Roster emits track a run's activity — a turn
/// starting, a tool call, a pause — and `cide://workspace-changed`'s own doc gives the reason
/// `git_status` and `diagnostics` carry no revision either: a number on this stream would make
/// every window re-hydrate its whole workspace because an agent started a tool call.
///
/// The whole roster rather than a signal to re-ask, on `workspace_changed`'s argument: the
/// emitter has it in hand, and a window that answered by re-reading would be reading files that
/// may have changed again in between. It was called "a few kilobytes" here, unmeasured; it
/// carries every role's system prompt, which in a project with a dozen roles is tens of
/// kilobytes, and [`agents_changed`] now logs the real figure at debug level. The coalescer's
/// flush sends through [`agents_changed_unless_repeat`], so an unchanged roster is not re-sent.
///
/// The markers have multiplied since that paragraph was first written — the Settings commands
/// (`agents_config_set`/`agents_save`/`agents_delete`), the run registry's activity, and
/// `crate::dotcide` translating a `.cide/agents` or `.cide/config.json` change on disk — but the
/// *emit* still has one funnel: everything except the three Settings commands routes through
/// `AgentRegistry::mark_changed`, whose coalescer (120 ms, 1 s ceiling) rebuilds the roster from
/// disk at flush. "Built immediately after the change, from the state that change produced"
/// therefore survives the fs-driven marker, which is what keeps the no-`rev` argument sound.
pub const AGENTS_CHANGED: &str = "cide://agents-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentsChanged {
    project: cide_ipc::ProjectId,
    roster: cide_ipc::AgentRoster,
}

pub fn agents_changed(
    app: &AppHandle,
    project: cide_ipc::ProjectId,
    roster: &cide_ipc::AgentRoster,
) {
    remember_roster(project, roster);
    let payload = AgentsChanged {
        project,
        roster: roster.clone(),
    };
    // Measured, not asserted: the paragraph on `AGENTS_CHANGED` called this "a few kilobytes",
    // the sentence `tasks_changed` carried until M68 measured 2.19 MB. Every role's whole system
    // prompt rides here. A second encode only when someone is reading the debug log.
    if tracing::enabled!(tracing::Level::DEBUG) {
        let bytes = serde_json::to_vec(&payload).map_or(0, |json| json.len());
        tracing::debug!(%project, bytes, "agents-changed payload");
    }
    // See `tasks_changed`: the project, and the device re-reads. An `AgentRoster` carries every
    // role's whole system prompt, which is exactly what `RemoteAgent` exists to leave behind.
    tee(|| cide_remote::RemoteEvent::AgentsChanged { project });
    if let Err(error) = app.emit(AGENTS_CHANGED, payload) {
        tracing::debug!(%error, "agents-changed reached no window");
    }
}

/// [`agents_changed`], unless this project's last roster sent was this very roster.
///
/// For the coalescer's flush (`AgentRegistry::flush`), which rebuilds the roster from disk on
/// every marker — a run's activity, a watcher event under `.cide/` — and used to broadcast it
/// whether or not anything in it had moved. Each broadcast is every role's prompt parsed by every
/// window and a fresh roster adopted by each store, so an identical one was a re-render of every
/// view that reads the roster, for nothing.
///
/// The memory is shared with [`agents_changed`] itself, so a roster the Settings commands sent
/// directly counts as sent: a flush that rebuilds what Settings just broadcast stays quiet, and
/// one that finds the roster moved *back* is not mistaken for a repeat. The Settings commands
/// keep the unconditional emit — their screen may be waiting on the event as its answer.
pub fn agents_changed_unless_repeat(
    app: &AppHandle,
    project: cide_ipc::ProjectId,
    roster: &cide_ipc::AgentRoster,
) {
    let repeat = LAST_ROSTER
        .lock()
        .get(&project)
        .is_some_and(|last| last == roster);
    if repeat {
        return;
    }
    agents_changed(app, project, roster);
}

/// The roster each project was last sent. A handful of projects, a few tens of kilobytes each;
/// cleared outright past [`LAST_ROSTER_CAP`] rather than evicted with care, because forgetting
/// only costs one broadcast that could have been skipped.
static LAST_ROSTER: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<cide_ipc::ProjectId, cide_ipc::AgentRoster>>,
> = std::sync::LazyLock::new(Default::default);

const LAST_ROSTER_CAP: usize = 64;

fn remember_roster(project: cide_ipc::ProjectId, roster: &cide_ipc::AgentRoster) {
    let mut last = LAST_ROSTER.lock();
    if last.len() >= LAST_ROSTER_CAP && !last.contains_key(&project) {
        last.clear();
    }
    last.insert(project, roster.clone());
}

// --- milestones (M83) --------------------------------------------------------------------

/// A project's milestones changed: the plan was written, a gate started or finished, one was
/// accepted.
///
/// A signal carrying only the project, and the window re-asks with `milestones_get`. The view is
/// small, but it is *derived* from three places — the committed config, this machine's gate
/// results and the board — and a snapshot built at emit time by a gate thread would race the
/// panel write it is reporting. One producer, `milestones::view`, read when it is wanted.
pub const MILESTONES_CHANGED: &str = "cide://milestones-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MilestonesChanged {
    project: cide_ipc::ProjectId,
}

pub fn milestones_changed(app: &AppHandle, project: cide_ipc::ProjectId) {
    // A paired device is owed it too (M91): a gate running, and its verdict, is exactly what
    // somebody away from the desk is waiting to hear. The project only, re-read by the server's
    // coalescer with the rest of the project's projections.
    tee(|| cide_remote::RemoteEvent::MilestonesChanged { project });
    if let Err(error) = app.emit(MILESTONES_CHANGED, MilestonesChanged { project }) {
        tracing::debug!(%error, "milestones-changed reached no window");
    }
}

/// A paired device pressed PgUp/PgDn on a session showing the normal screen. (M91)
///
/// The pane host holding `session` scrolls its xterm by `pages` — the desk's view and the
/// device's move together, which is what the button promises. Every window is told, because
/// which one holds the pane is the webview's knowledge (a detached pane is in its own window);
/// the others hold no host for the session and do nothing.
pub const REMOTE_SCROLL: &str = "cide://remote-scroll";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteScroll {
    session: cide_ipc::SessionId,
    pages: i8,
}

pub fn remote_scroll(app: &AppHandle, session: cide_ipc::SessionId, pages: i8) {
    if let Err(error) = app.emit(REMOTE_SCROLL, RemoteScroll { session, pages }) {
        tracing::debug!(%error, "remote-scroll reached no window");
    }
}

// --- extensions (M22) ------------------------------------------------------------------

/// The extension registry changed: a marketplace connected or refreshed, something installed,
/// enabled, disabled or removed.
///
/// # Why this one carries `rev`
///
/// The same test `tasks_changed` sets out, applied to a different file, and it comes out the same
/// way. `agents_changed` carries no revision because the roster is *derived* from one in-process
/// read plus a registry with exactly one writer, so the last one sent is by construction the
/// newest and there is nothing for an ordering number to protect.
///
/// `extensions.json` has several writers. Two windows can each be installing something; a
/// background refresh finishes on a thread of its own; and — the one that makes the difference —
/// the user can `git pull` a marketplace clone by hand, or edit the config file, at any moment.
/// Snapshots can therefore arrive out of order, and the receiver's only defence is a high-water
/// mark, which is what `ExtensionSnapshot::rev` is.
///
/// # The whole snapshot, not a signal to re-ask
///
/// `workspace_changed`'s argument: it is a few kilobytes, the emitter has it in hand, and a window
/// that answered by re-reading would be reading files that may have changed again in between —
/// which for a directory a `git pull` can move is not a hypothetical.
///
/// # It is not enough on its own
///
/// A window that receives this has a new *panel* to draw. What it does **not** get is a new fold
/// table: `foldSpecFor` is called inside the editor's mount dispatch and cannot await, so the
/// language registry is read from `Bootstrap` at first paint. Enabling a language extension
/// therefore updates the panel immediately and the open editors on their next mount — which is
/// what the panel says out loud rather than pretending otherwise.
pub const EXT_CHANGED: &str = "cide://ext-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtChanged {
    snapshot: cide_ipc::ext::ExtensionSnapshot,
}

// --- OpenSpec (M28) ----------------------------------------------------------------------

/// A project's `openspec/` changed — a spec, a change, or the directory arriving at all.
pub const SPEC_CHANGED: &str = "cide://spec-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpecChanged {
    project: cide_ipc::ProjectId,
    board: cide_ipc::SpecBoard,
}

/// Broadcast one project's spec board.
///
/// # Why this one carries no `rev`, when `tasks-changed` does
///
/// `agents-changed`'s argument, with a stronger version of the same guarantee. That event carries
/// none because the roster has a single writer; this one carries none because the board is
/// **derived** — every emit is preceded by a fresh read, and `spec_state::SpecBoards`' coalescer
/// runs exactly one flusher per burst, so only one read is ever in flight for a project. Two
/// boards therefore cannot land in an order other than the one they were read in, and there is
/// nothing an ordering number could protect against.
///
/// That is a property of the coalescer, not a coincidence: a second path that read a board and
/// emitted it directly would break it, which is why `cmd::spec` goes through `mark_changed` for
/// anything a watcher could also see.
pub fn spec_changed(app: &AppHandle, project: cide_ipc::ProjectId, board: &cide_ipc::SpecBoard) {
    let payload = SpecChanged {
        project,
        board: board.clone(),
    };
    if let Err(error) = app.emit(SPEC_CHANGED, payload) {
        tracing::debug!(%error, "spec-changed reached no window");
    }
}

pub fn ext_changed(app: &AppHandle, snapshot: &cide_ipc::ext::ExtensionSnapshot) {
    let payload = ExtChanged {
        snapshot: snapshot.clone(),
    };
    if let Err(error) = app.emit(EXT_CHANGED, payload) {
        tracing::debug!(%error, "ext-changed reached no window");
    }
}

/// The machine's Docker daemon changed — a container started, an image was pulled, the
/// connection was switched. (M41)
pub const DOCKER_CHANGED: &str = "cide://docker-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DockerChanged {
    board: cide_ipc::docker::DockerBoard,
}

/// Broadcast the machine's Docker board.
///
/// # Why this carries neither a `rev` nor a project
///
/// No `rev` for [`SPEC_CHANGED`]'s reason, which holds here in the same form: the board is
/// **derived**, every emit is preceded by a fresh read, and `docker_state::DockerState`'s
/// coalescer runs exactly one flusher per burst — so only one read is ever in flight and two
/// boards cannot land out of order. A second path that read a board and emitted it directly
/// would break that property silently, which is why this function is the only caller's road.
///
/// No project because a daemon is not one. It belongs to the machine, every window shows the
/// same containers, and keying this by project would send N copies of one answer that disagree
/// while they refresh.
pub fn docker_changed(app: &AppHandle, board: &cide_ipc::docker::DockerBoard) {
    let payload = DockerChanged {
        board: board.clone(),
    };
    if let Err(error) = app.emit(DOCKER_CHANGED, payload) {
        tracing::debug!(%error, "docker-changed reached no window");
    }
}

/// The remote listener's state, or its device list, moved.
///
/// Carries **no payload**: the panel asks. Three reasons, and the first is the one that decides
/// it — a payload here would be the device list, and a device list is the one thing on this
/// surface a webview has no business holding more copies of than it must. The second is that the
/// panel is open on one screen at most, so pushing a list to every window is work for nobody.
/// The third is that the readout has a live part (how many devices are connected) which is read
/// from the socket rather than stored, so a pushed snapshot would be stale the moment it landed.
pub const REMOTE_CHANGED: &str = "cide://remote-changed";

pub fn remote_changed(app: &AppHandle) {
    if let Err(error) = app.emit(REMOTE_CHANGED, ()) {
        tracing::debug!(%error, "remote-changed reached no window");
    }
}

/// Global GitLab connections, preferences and open MR identities; credentials never leave Rust.
pub const GITLAB_CHANGED: &str = "cide://gitlab-changed";
pub fn gitlab_changed(app: &AppHandle, board: &cide_ipc::gitlab::GitLabBoard) {
    let _ = app.emit(GITLAB_CHANGED, board);
}

/// A review's local drafts moved: an agent wrote, edited or discarded one. (M85) Carries the
/// review id only — the panel re-reads that review's drafts — because a draft list rides no
/// board and an MR nobody has open needs nothing read at all.
///
/// Not folded into [`GITLAB_CHANGED`]: that one's payload is the board, and a board with an
/// unchanged revision is one the panel is right to treat as nothing new.
pub const GITLAB_DRAFTS_CHANGED: &str = "cide://gitlab-drafts-changed";
pub fn gitlab_drafts_changed(app: &AppHandle, review: &str) {
    let _ = app.emit(GITLAB_DRAFTS_CHANGED, review);
}

// --- the remote tee ---------------------------------------------------------------------
//
// `AppHandle::emit` reaches this process's webviews and nothing else, so a paired device cannot
// subscribe to any of the above. The answer is a tee *here* rather than a second event surface,
// and that placement is the whole point: this module's header promises that every `cide://` event
// goes out through one file, `cargo xtask contract-check` reads that file textually to find them,
// and a parallel set of notifications emitted from wherever they happened to be convenient would
// quietly falsify both.
//
// Four properties make it safe to put a network consumer on this path:
//
// * **It adds no `cide://` name.** `scan_events` looks for the `pub const … = "cide://…"`
//   literals; the tee introduces none, so the contract file is untouched by the mechanism.
// * **It never blocks the emitter.** The channel is bounded and the send is `try_send`. The
//   threads that reach here are the hook applier and the GTK main loop, and neither may be made
//   to wait on a phone's socket. A full queue is not an error: `cide-remote` tells the device it
//   fell behind and the device re-reads. That is `cide-pty`'s credit protocol's argument, one
//   consumer further out.
// * **It carries no `Workspace`.** The tree holds `Settings`, which holds provider API keys and
//   proxy passwords in plaintext. `workspace_changed` tees its *revision* and nothing else, and a
//   device answers a revision by asking for a projection.
// * **It is absent when the feature is off**, which is the default, so an installation with no
//   paired device pays one uncontended read lock per emit.

use parking_lot::RwLock;
use std::sync::OnceLock;

type Tee = RwLock<Option<tokio::sync::mpsc::Sender<cide_remote::RemoteEvent>>>;

fn remote_tee() -> &'static Tee {
    static TEE: OnceLock<Tee> = OnceLock::new();
    TEE.get_or_init(|| RwLock::new(None))
}

/// Send a copy of every teed event to `tx`. Called by `remote.rs` when the listener starts.
pub fn install_remote_tee(tx: tokio::sync::mpsc::Sender<cide_remote::RemoteEvent>) {
    *remote_tee().write() = Some(tx);
}

/// Stop teeing. Called when the listener stops, and on shutdown.
pub fn clear_remote_tee() {
    *remote_tee().write() = None;
}

/// Hand one event to the remote pump, if there is one.
///
/// The event is built by a closure rather than passed in, so an installation with the feature off
/// does not clone an awaiting set several times a second in order to drop it.
fn tee(make: impl FnOnce() -> cide_remote::RemoteEvent) {
    let held = remote_tee().read();
    let Some(tx) = held.as_ref() else {
        return;
    };
    if let Err(error) = tx.try_send(make()) {
        // Expected under load and handled downstream by a `Desync` to the device. Not a warning:
        // it would be one per dropped frame, from the thread least able to afford them.
        tracing::debug!(%error, "remote tee: an event was dropped");
    }
}

// --- the New project wizard (M97) -----------------------------------------------------------

/// A step of `project_new` started, finished or failed.
///
/// To every window rather than the one that asked: the wizard lives in whichever window the
/// gesture came from, and `emit_to` would need that window's label threaded through a command
/// that otherwise has no reason to know it. A second window with no wizard open has no listener
/// and drops it, which is the whole cost. Not teed to the remote surface — a device has no wizard.
pub const PROJECT_NEW_PROGRESS: &str = "cide://project-new-progress";

pub fn project_new_progress(app: &AppHandle, progress: &cide_ipc::NewProjectProgress) {
    if let Err(error) = app.emit(PROJECT_NEW_PROGRESS, progress.clone()) {
        tracing::debug!(%error, "project-new-progress reached no window");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One test rather than four, because the tee is a process-global and four tests would race
    /// each other for it. `cargo test` runs a crate's tests in threads by default, and a shared
    /// `static` is exactly the thing that makes that visible.
    #[test]
    fn the_tee_is_lazy_when_absent_built_when_present_and_lossy_when_full() {
        static BUILT: AtomicUsize = AtomicUsize::new(0);
        let event = || {
            BUILT.fetch_add(1, Ordering::Relaxed);
            cide_remote::RemoteEvent::WorkspaceRev { rev: 9 }
        };

        // Absent: the event is never built. This is the case that is true on every installation
        // with the feature off, several times a second during a turn.
        clear_remote_tee();
        tee(event);
        assert_eq!(BUILT.load(Ordering::Relaxed), 0);

        // Present: it arrives.
        let (tx, mut rx) = tokio::sync::mpsc::channel(2);
        install_remote_tee(tx);
        tee(event);
        assert_eq!(BUILT.load(Ordering::Relaxed), 1);
        assert!(matches!(
            rx.try_recv(),
            Ok(cide_remote::RemoteEvent::WorkspaceRev { rev: 9 })
        ));

        // Full: dropped, and — the property the emitter depends on — the call still returns.
        // A blocking send here would be the hook-apply thread waiting on a phone's socket.
        for _ in 0..8 {
            tee(event);
        }
        assert!(rx.try_recv().is_ok());
        clear_remote_tee();
    }
}
