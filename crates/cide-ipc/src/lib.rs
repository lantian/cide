//! Wire types shared by the Rust core and the webview.
//!
//! Every type here is `#[serde(rename_all = "camelCase")]` and derives `TS` so that
//! `cargo xtask codegen` can emit `ui/src/ipc/generated.ts`. Inbound types additionally
//! use `deny_unknown_fields` so a frontend/backend drift fails loudly instead of
//! silently dropping a field.
//!
//! This crate must never depend on `tauri`.

pub mod ids;
pub mod keymap;
pub mod settings;
pub mod workspace;

pub use ids::*;
pub use keymap::{Binding, Command, KeymapLayer, ResolvedBinding};
pub use settings::{
    ClaudeSettings, EditorSettings, GraphicsSettings, Settings, TerminalRenderer, TerminalSettings,
};
pub use workspace::{
    DiffAnswer, DiffOrigin, DiffSpec, Direction, LayoutNode, MAX_RATIO, MIN_RATIO, Pane, PaneTree,
    Project, ProjectRoot, SettingsSection, Tab, TabKind, WindowRole, Workspace,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Everything a freshly opened window needs in one round trip.
///
/// One call rather than several: a window that has to make four requests before it can
/// paint will show three intermediate states, and on a slow IPC path that is visible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Bootstrap {
    pub window: WindowLabel,
    pub role: WindowRole,
    pub workspace: Workspace,
    pub keymap: Vec<ResolvedBinding>,
    pub commands: Vec<Command>,
    pub capabilities: Capabilities,
}

/// Whether a pane's conversation can be picked up where it left off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum SessionRestore {
    /// Claude Code still holds a transcript for this id: spawn with `--resume`.
    Resumable { session: SessionId },
    /// Spawn clean. Every shell, and every Claude pane whose transcript is gone.
    Fresh,
}

/// What should happen to one pane on launch.
///
/// Advice, not instruction: the frontend owns spawning, because only the frontend knows a
/// pane's size, and a child spawned before its slot is laid out draws a ruined first frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PaneRestore {
    /// The window this pane appears in, so a window can keep the entries that are its own.
    pub window: WindowLabel,
    pub project: ProjectId,
    /// The tab holding the pane — for a detached pane, the tab it re-docks into.
    pub tab: TabId,
    pub pane: PaneId,
    pub kind: PaneKind,
    /// Where the session must be spawned for `restore` to hold. Claude Code files a
    /// transcript under the directory it was started in, so resuming from elsewhere finds
    /// nothing.
    #[ts(type = "string")]
    pub cwd: std::path::PathBuf,
    pub restore: SessionRestore,
    /// Whether to spawn on launch rather than waiting to be asked.
    ///
    /// True for exactly one pane per project: the one bound to its primary session. Every
    /// other Claude pane renders a splash with a resume affordance, because reopening a
    /// six-pane project must not silently start six agents at once.
    pub eager: bool,
}

/// What this build can actually do, so the frontend never offers a dead control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Capabilities {
    pub version: String,
    /// `claude --version`, or `None` when the binary is not on `PATH`.
    pub claude_version: Option<String>,
    /// False until the LSP phase; the status bar's diagnostics counts stay a placeholder
    /// and `getDiagnostics` answers empty rather than pretending.
    pub diagnostics: bool,
    /// Whether the IDE-integration MCP server is running for this project.
    pub ide_protocol: bool,
}

/// Result of the mandatory boot-time IPC probe.
///
/// Tauri's fast path (`ipc:` custom protocol) degrades *silently*: if WebKitGTK is older
/// than 2.40 or a CSP rule blocks the scheme, `ipc-protocol.js` catches the fetch
/// rejection, sets `customProtocolIpcFailed = true` permanently and falls back to string
/// `postMessage`. Throughput collapses with no crash and no Rust-side signal, so the
/// frontend has to tell us which path it actually got.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpcHealth {
    /// False means we are on the slow `postMessage` path.
    pub custom_protocol: bool,
    /// Measured throughput of the raw-`Response` pull path.
    pub mib_per_sec: f64,
    /// Reported by the webview, e.g. "2.52.3".
    pub webkit_version: String,
}

/// Terminal geometry, in cells and in cell pixels.
///
/// The pixel fields must be real. Passing zeroes is the usual shortcut and it breaks any
/// program that asks the terminal for its cell size — sixel and inline-image output, and
/// pixel-resolution mouse reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 17,
        }
    }
}

/// Orientation of a split. `Row` places children left/right, `Col` top/bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Axis {
    Row,
    Col,
}

/// Which half of a new split the freshly created pane occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Side {
    Before,
    After,
}

/// What a pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PaneKind {
    Claude,
    Shell,
    Diff,
    Editor,
}

/// Whether a pane may be closed.
///
/// The pinned Claude tab's first pane is `Primary`: closing it while it is the only leaf
/// returns `Err(PanePrimary)` so the project console can never be emptied by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PaneRole {
    Primary,
    Auxiliary,
}

/// What a newly split pane should contain.
///
/// This enum *is* the multiplexing model: "one session but splittable" resolves as one
/// **primary** session plus panes that each declare what they want.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SplitIntent {
    /// `claude --session-id <fresh uuid>` — renders the splash until first input.
    NewClaude,
    /// `claude --resume <src> --fork-session` — shares history up to the split point.
    ForkPrimary,
    /// A second sink on an existing session. No new process.
    Mirror { session: SessionId },
    /// `$SHELL -l`
    Shell,
    /// No process; driven by an `openDiff` RPC or the git panel.
    Diff,
}

/// Lifecycle of a session, driven by Claude Code hooks with a PTY-quiet fallback.
///
/// "Live session" for the close-confirm setting means `Busy | AwaitingPermission` —
/// not "the process exists", which would warn constantly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "state")]
#[ts(export)]
pub enum SessionState {
    Spawning,
    Splash,
    Idle,
    Busy,
    AwaitingPermission,
    AwaitingInput,
    Exited { code: i32 },
}

impl SessionState {
    /// Whether closing this session should prompt for confirmation.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Busy | Self::AwaitingPermission)
    }
}

/// How the app is laid out across OS windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WindowMode {
    /// Every open project docks into the task bar of one window.
    #[default]
    Stacked,
    /// Each project opens as its own OS window.
    PerProject,
}

/// Which theme the webview should paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// A session that would be interrupted by closing something.
///
/// Carries enough to name it on screen without a second round trip. A dialog that says "3
/// sessions are busy" and cannot say *which* leaves the user no way to decide, so the pane
/// title and project name travel with the answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionSummary {
    pub session: SessionId,
    pub project: ProjectId,
    pub project_name: String,
    pub pane_title: String,
    pub state: SessionState,
}

/// Whether closing may proceed, and what it would interrupt.
///
/// `blocking` is empty when nothing is `Busy` or `AwaitingPermission` — the common case, and
/// the one where no dialog should appear at all. A confirmation that fires whenever a
/// process exists is one users learn to dismiss without reading, which is worse than none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QuitDecision {
    pub blocking: Vec<SessionSummary>,
}

/// What a split produced.
///
/// The resolved intent travels back with the pane id because the caller may not know it: a
/// `None` intent asks the domain for the tab's default, and the frontend still has to spawn
/// the right thing afterwards. Returning only the id would leave it guessing — and guessing
/// `NewClaude` where the domain said `ForkPrimary` starts a fresh conversation where the
/// user asked to branch an existing one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SplitOutcome {
    pub pane: PaneId,
    pub intent: SplitIntent,
}
