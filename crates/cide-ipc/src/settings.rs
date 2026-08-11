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
    pub sidebar: SidebarSettings,
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
            sidebar: SidebarSettings::default(),
        }
    }
}

/// The narrowest a sidebar panel may be left, in CSS pixels.
///
/// 180 rather than something smaller because both panels stop being *usable* below it
/// rather than merely tight: the explorer indents 12px per depth and draws 21px mono rows,
/// so a file three levels down has about 12 characters left at 180 and none at 140; the git
/// panel's header carries a segmented Commit/Shelf control plus its counts on one 30px line.
/// A panel that can be dragged to a sliver is a panel a user can lose by accident, and there
/// is no "reset width" gesture to get it back with.
pub const SIDEBAR_MIN_WIDTH: u16 = 180;

/// The widest, in CSS pixels. Roughly 1.5x the mock's 420px git panel.
///
/// This is the *stored* ceiling and it is deliberately generous — it exists to keep a
/// corrupt or hand-edited `workspace.json` from producing a panel wider than any monitor,
/// not to decide what fits. What fits depends on the window, which Rust cannot see, so the
/// frontend applies a second ceiling against the live viewport (`clampSidebarWidth` in
/// `ui/src/chrome/sidebarWidth.ts`) before it paints anything. Restoring a 640px panel into
/// a 720px window is therefore safe: it is clamped on the way to the DOM, and the stored
/// value survives for the next time the window is wide enough to honour it.
pub const SIDEBAR_MAX_WIDTH: u16 = 640;

/// How wide the user left each sidebar panel.
///
/// **Two widths, not one.** The mock gives the explorer 252px and the git panel 420px, and
/// that difference is a property of the content, not a stylistic accident: the explorer
/// draws one truncatable name per row, while the git panel draws a path *and* an
/// added/removed figure *and* a stage checkbox on the same line. Sharing a single number
/// would mean every switch between the two views resized the workspace under the user, and
/// whichever panel they had not tuned would be the wrong width — so the resize would feel
/// like it had been forgotten rather than remembered.
///
/// Only these two, because only these two are tokens: `--w-sidebar-files` also sizes the
/// search and problems panels (they are the explorer's column with different rows in it),
/// which is a decision `tokens.css` already made and this type follows rather than reopens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct SidebarSettings {
    /// `--w-sidebar-files`: the explorer, and with it search and problems.
    pub files_width: u16,
    /// `--w-sidebar-git`.
    pub git_width: u16,
}

impl Default for SidebarSettings {
    /// The mock's two widths, which are also the two literals `tokens.css` ships as the
    /// token values — a fresh workspace and a workspace whose settings failed to load look
    /// identical, which is the point.
    fn default() -> Self {
        Self {
            files_width: 252,
            git_width: 420,
        }
    }
}

impl SidebarSettings {
    /// Both widths brought inside [`SIDEBAR_MIN_WIDTH`]..=[`SIDEBAR_MAX_WIDTH`].
    ///
    /// Applied where a patch lands rather than where the workspace is read, so the stored
    /// file converges on a legal value instead of being re-clamped forever on every load.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            files_width: self.files_width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
            git_width: self.git_width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The round trip a resize has to survive to still be there after a restart: struct →
    /// the JSON `workspace.json` actually stores → struct.
    ///
    /// Asserted on the *camelCase wire names* rather than only on equality, because equality
    /// would pass just as well if both directions agreed on `files_width` — and the
    /// frontend, which reads this type through `generated.ts`, would then see `undefined`
    /// for both widths and fall back to the mock's defaults on every launch. That is exactly
    /// the bug this feature exists to fix, arriving through the serializer instead.
    #[test]
    fn a_sidebar_width_survives_the_json_round_trip_under_its_wire_name() {
        let settings = Settings {
            sidebar: SidebarSettings {
                files_width: 300,
                git_width: 500,
            },
            ..Settings::default()
        };

        let json = serde_json::to_string(&settings).expect("serialize");
        assert!(
            json.contains("\"filesWidth\":300"),
            "wire name changed: {json}"
        );
        assert!(
            json.contains("\"gitWidth\":500"),
            "wire name changed: {json}"
        );

        let back: Settings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.sidebar, settings.sidebar);
    }

    /// A `workspace.json` written before this field existed — i.e. every install that
    /// upgrades into this build.
    ///
    /// It must load, and it must load as the mock's widths rather than as zero. That is what
    /// the container's `#[serde(default)]` buys; an `Option<SidebarSettings>` would have
    /// pushed the same decision onto the frontend, in a place with no default to reach for.
    #[test]
    fn settings_saved_before_the_sidebar_field_existed_still_load() {
        let legacy = r#"{"theme":"dark","reopenLastProject":false}"#;
        let settings: Settings = serde_json::from_str(legacy).expect("legacy settings");
        assert_eq!(settings.sidebar, SidebarSettings::default());
        assert_eq!(settings.sidebar.files_width, 252);
        assert_eq!(settings.sidebar.git_width, 420);
        // And the rest of the file still parsed: the new field did not become required.
        assert!(!settings.reopen_last_project);
    }

    /// Half a `sidebar` object, which is what a hand-edited file tends to look like.
    #[test]
    fn one_named_width_leaves_the_other_at_its_default() {
        let partial = r#"{"sidebar":{"gitWidth":500}}"#;
        let settings: Settings = serde_json::from_str(partial).expect("partial sidebar");
        assert_eq!(settings.sidebar.git_width, 500);
        assert_eq!(settings.sidebar.files_width, 252);
    }

    #[test]
    fn clamping_pulls_both_widths_inside_the_band_and_leaves_legal_ones_alone() {
        let clamped = SidebarSettings {
            files_width: 4,
            git_width: 9_000,
        }
        .clamped();
        assert_eq!(clamped.files_width, SIDEBAR_MIN_WIDTH);
        assert_eq!(clamped.git_width, SIDEBAR_MAX_WIDTH);

        let legal = SidebarSettings {
            files_width: 300,
            git_width: 500,
        };
        assert_eq!(legal.clamped(), legal);
        // The defaults are inside the band, or a first launch would move the panel itself.
        assert_eq!(
            SidebarSettings::default().clamped(),
            SidebarSettings::default()
        );
    }
}
