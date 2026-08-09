//! User settings — the data behind the Settings tab.
//!
//! Field names and defaults track the design mock's Settings screen exactly, including the
//! toggles' default states, so the UI has nothing to invent.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{Theme, WindowMode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Settings {
    /// *Stack projects in the header* vs *One window per project*.
    pub window_mode: WindowMode,
    pub theme: Theme,

    /// "Each project keeps its own Claude tab — pinned, cannot be closed, only its panes
    /// can." Off would mean a project with no console tab, which the rest of the model
    /// does not currently allow; the toggle exists in the mock and is honoured as
    /// read-only-true until there is a coherent meaning for false.
    pub each_project_keeps_claude_tab: bool,

    /// "Reopen the last project on launch — restores its tab strip and pane layout."
    pub reopen_last_project: bool,

    /// "Keep sessions running when a project window is closed."
    ///
    /// Deliberately *not* worded as the mock's "keep a project running when its window is
    /// closed — live Claude sessions survive in the background". cide runs no background
    /// daemon: quitting the app quits its Claude sessions, and the workspace is restored on
    /// relaunch via `claude --resume`. Promising background survival would be a lie.
    pub keep_sessions_on_window_close: bool,

    /// "Confirm before closing a project with a live session." *Live* means a session is
    /// `Busy` or `AwaitingPermission` — not merely that a process exists, which would warn
    /// constantly.
    ///
    /// Read by `cide_app::cmd::app::app_quit_requested`, and by nothing else. Its whole
    /// authority is over the `blocking` half of `QuitDecision`: off, and a close no longer
    /// asks about an agent mid-turn.
    ///
    /// It governs **sessions only**, and specifically not unsaved file tabs, which are
    /// reported whatever it says. The two are different kinds of loss. Interrupting a turn
    /// costs the turn — the conversation is keyed by `SessionId` and resumes with
    /// `claude --resume` — so a user may reasonably decide they do not want to be asked.
    /// Discarding an unsaved buffer costs the work outright, with nothing to resume from,
    /// and a toggle whose label says "live session" must not quietly turn that guard off
    /// too. If unsaved edits ever need their own toggle, that is a second field with its own
    /// wording, not a second meaning stapled to this one.
    pub confirm_close_with_live_session: bool,

    pub editor: EditorSettings,
    pub terminal: TerminalSettings,
    pub graphics: GraphicsSettings,
    pub claude: ClaudeSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window_mode: WindowMode::default(),
            theme: Theme::default(),
            each_project_keeps_claude_tab: true,
            reopen_last_project: true,
            keep_sessions_on_window_close: true,
            confirm_close_with_live_session: false,
            editor: EditorSettings::default(),
            terminal: TerminalSettings::default(),
            graphics: GraphicsSettings::default(),
            claude: ClaudeSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct EditorSettings {
    pub font_size: u8,
    pub tab_size: u8,
    pub insert_spaces: bool,
    pub show_minimap: bool,
    pub word_wrap: bool,
    pub trim_trailing_whitespace_on_save: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_size: 13,
            tab_size: 4,
            insert_spaces: true,
            show_minimap: true,
            word_wrap: false,
            trim_trailing_whitespace_on_save: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct TerminalSettings {
    pub font_size: u8,
    pub scrollback: u32,
    /// Which xterm.js renderer to prefer.
    ///
    /// A setting rather than a probe because the healthy and unhealthy paths are
    /// indistinguishable from JavaScript: WebGL context creation succeeds even on a
    /// software rasterizer, and `WEBGL_debug_renderer_info` is masked — on Linux it reports
    /// "Apple GPU" regardless of hardware.
    pub renderer: TerminalRenderer,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_size: 12,
            scrollback: 5_000,
            renderer: TerminalRenderer::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TerminalRenderer {
    /// WebGL for the most recently focused panes, DOM for the rest. WebKitGTK caps
    /// concurrent contexts at roughly 8-16 and a pane grid can exceed that.
    #[default]
    Auto,
    Webgl,
    Dom,
}

/// Linux rendering workarounds, applied before the webview is created.
///
/// Each is off by default and each costs something; see `docs/adr/0006`. `disable_dmabuf`
/// is the exception the app cannot start without on a stock KDE Wayland desktop, so
/// `cide-app::graphics` applies it unconditionally unless the environment already has an
/// opinion — this struct is the user-facing override for the machines that heuristic gets
/// wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct GraphicsSettings {
    pub disable_dmabuf_renderer: Option<bool>,
    pub disable_compositing_mode: Option<bool>,
    pub disable_nvidia_explicit_sync: Option<bool>,
}

/// Environment toggles passed to `claude` children. All default off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ClaudeSettings {
    /// `CLAUDE_CODE_DISABLE_MOUSE=1`. Worth offering because xterm.js has no
    /// shift-to-bypass gesture for mouse capture, so a TUI that grabs the mouse takes
    /// selection with it.
    pub disable_mouse: bool,
    /// `CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT=1`.
    pub alt_screen_full_repaint: bool,
    /// `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1`.
    pub disable_alternate_screen: bool,
}
