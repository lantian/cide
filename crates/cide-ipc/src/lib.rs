//! Wire types shared by the Rust core and the webview.
//!
//! Every type here is `#[serde(rename_all = "camelCase")]` and derives `TS` so that
//! `cargo xtask codegen` can emit `ui/src/ipc/generated.ts`. Inbound types additionally
//! use `deny_unknown_fields` so a frontend/backend drift fails loudly instead of
//! silently dropping a field.
//!
//! This crate must never depend on `tauri`.

pub mod git;
pub mod headless;
pub mod ids;
pub mod keymap;
pub mod settings;
pub mod settings_ops;
pub mod workspace;

pub use headless::{HeadlessError, HeadlessRequest, HeadlessResult};
pub use ids::*;
pub use keymap::{Binding, Command, KeymapLayer, ResolvedBinding};
pub use settings::{
    ClaudeSettings, EditorSettings, GraphicsSettings, ProxyMode, ProxySettings, SIDEBAR_MAX_WIDTH,
    SIDEBAR_MIN_WIDTH, Settings, SidebarSettings, TerminalRenderer, TerminalSettings,
    normalize_proxy_url, redact_proxy_url,
};
pub use settings_ops::{
    GraphicsRung, GraphicsStatus, KeymapConflict, KeymapProblem, KeymapReport, SettingsPatch,
};
pub use workspace::{
    DiffAnswer, DiffOrigin, DiffSpec, Direction, DockAnchor, DockSibling, LayoutNode, MAX_RATIO,
    MIN_RATIO, Pane, PaneTree, Project, ProjectRoot, SettingsSection, Tab, TabKind, WindowRole,
    Workspace,
};

// --- M8: file tree, watcher, pickers ---
pub mod fs;
pub mod search;

pub use fs::{FsChange, FsStatus, TreeRow, TreeRowKind, WatchBackend, WatchStatus};
pub use search::{PickerFrame, PickerItem, PickerRow};
// M11: the content search's own wire types. Same module, different job — see the section
// comment in `search.rs` for why they are not the picker's.
pub use search::{SearchFrame, SearchHit, SearchMode, SearchQuery};

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
/// The pinned Claude tab's first pane is `Primary`: closing *or detaching* it returns
/// `Err(PanePrimary)` however many panes the tab holds, so the project console can never
/// lose its conversation — not by accident, and not by splitting first.
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
    // There is deliberately no `Diff`. A diff pane is not something a *split* can produce:
    // what it renders comes from its tab's `TabKind::Diff { spec }`, and the two gestures
    // that open one — an `openDiff` RPC and the git panel — both create the tab and the pane
    // together (`ide::open_diff_tab`, `cmd::file::tab_open_diff`). The variant existed, was
    // reachable from no code path, and the arm handling it minted a pane titled
    // "<project> : claude — diff" that would have shown nothing: a `PaneKind::Diff` leaf
    // inside a Claude tab has no spec to read.
}

/// Lifecycle of a session, driven by Claude Code hooks with a PTY-quiet fallback.
///
/// "Live session" for the close-confirm setting means `Busy | AwaitingPermission` —
/// not "the process exists", which would warn constantly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
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

/// What the registry can still say about a session whose pane was not there to hear it die.
///
/// This is the answer to the *rehydration* question — a pane host evicted and re-created after
/// its child had already gone, so the `cide://session-state` event fired before anything was
/// listening. That path used to ask `session_has_exited`, which answers a `bool`, and so the
/// `— exited —` marker on it never carried a number: the code existed in this process the whole
/// time and was simply never on the wire.
///
/// Four variants because [`cide_pty`] genuinely has four states here and collapsing any two of
/// them loses the code again:
///
/// * `Running` — the child is alive. The pane was rehydrated over a session that outlived it,
///   which is the ordinary case and the reason the question is asked at all.
/// * `Reaping` — `has_exited()` is true but `exit_status()` is still `None`. EOF on the pty
///   master and the reaper's `wait()` are two events on two threads, and this is the window
///   between them. The code exists and is milliseconds away on `cide://session-state`, so the
///   caller must decline to mark and let the event do it.
/// * `Exited` — reaped, with the status `wait()` returned.
/// * `Unknown` — the registry has never held this id. A `SessionId` restored from
///   `workspace.json` after a restart is exactly this: the process that owned it died with the
///   app and no code survives anywhere. **This is the only answer for which a bare
///   `— exited —` is the truth**, and it is why this is a variant rather than an error —
///   `NoSuchSession` would make the one honest case indistinguishable from a failed call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SessionExit {
    Running,
    Reaping,
    Exited { code: i32 },
    Unknown,
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
///
/// **Light is the default, by explicit user instruction** — it used to be `Dark`. The
/// default is read in more places than the settings screen: `Settings` is `#[serde(default)]`
/// throughout, so it is also what a `workspace.json` with no `theme` key deserializes to,
/// and it is what `windows.rs` bakes into `?theme=` before a webview exists. Flipping it
/// here is what makes a fresh install open white.
///
/// The variant order is the mock's Appearance segmented control, and it stays Dark-first
/// even though Dark is no longer the default: ts-rs emits the union in declaration order,
/// and reordering would rewrite `generated.ts` for a cosmetic reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Theme {
    Dark,
    #[default]
    Light,
}

#[cfg(test)]
mod theme_tests {
    use super::Theme;

    /// Pins the default, because it is a user decision and nothing else in the tree fails
    /// when it changes: `Settings` is `#[serde(default)]`, so a flip back to `Dark` would
    /// silently change what a fresh install paints and what `windows.rs` writes into
    /// `?theme=`, with every existing test still green.
    #[test]
    fn the_default_theme_is_light() {
        assert_eq!(Theme::default(), Theme::Light);
    }
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

/// A file tab whose buffer holds edits that have not been written to disk.
///
/// The path travels with it for the same reason [`SessionSummary`] carries a pane title: a
/// dialog that says "3 unsaved files" and cannot say *which* gives the user no basis to
/// choose between discarding and going back. `title` is the basename the tab strip already
/// shows, so the dialog and the strip name the same thing; `path` disambiguates the two
/// `mod.rs` the user has open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UnsavedTab {
    pub tab: TabId,
    pub project: ProjectId,
    pub project_name: String,
    #[ts(type = "string")]
    pub path: std::path::PathBuf,
    /// The tab strip's label — the file's basename.
    pub title: String,
}

/// Whether closing may proceed, and what it would cost.
///
/// Both lists are empty in the common case, and that is the case where no dialog should
/// appear at all. A confirmation that fires whenever a process exists is one users learn to
/// dismiss without reading, which is worse than none.
///
/// The two lists are separate because the two risks are not the same risk, and the
/// confirmation has to word them differently:
///
/// * `unsaved` is **destruction**. Those edits exist nowhere else; closing loses them.
///   Always reported, whatever the settings say — see `Settings::
///   confirm_close_with_live_session`, which is about sessions and has no authority here.
/// * `blocking` is **interruption**. A `Busy` or `AwaitingPermission` session loses its
///   turn, not its transcript: the conversation resumes with `claude --resume`. This list
///   is governed by that setting, and is empty when the user has turned it off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QuitDecision {
    pub blocking: Vec<SessionSummary>,
    pub unsaved: Vec<UnsavedTab>,
}

impl QuitDecision {
    /// Whether anything at all is at risk. `false` means close without asking.
    pub fn is_clear(&self) -> bool {
        self.blocking.is_empty() && self.unsaved.is_empty()
    }
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

/// A text file as the editor loads it. (M9)
///
/// The text is the file's bytes verbatim, line endings included. Normalising here would be
/// the convenient thing and the wrong one: CodeMirror normalises again on `EditorState`
/// creation, so the only place that can tell a CRLF file from an LF one is the side holding
/// the original — and if that is not the editor, every save rewrites every line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileDoc {
    pub path: std::path::PathBuf,
    pub text: String,
    /// False when the file's mode bits carry no write permission at all.
    ///
    /// This is exactly `Permissions::readonly()` inverted, and that is a narrower question
    /// than "can this process write it": it reads the mode and nothing else, so a file
    /// owned by another user with mode 0644, or any file on a read-only mount, still comes
    /// back `true`. Purely advisory — the write is attempted regardless and reports its own
    /// failure — but it catches the common `chmod -w` case early rather than letting the
    /// user type for ten minutes into something that will not save.
    pub writable: bool,
}
