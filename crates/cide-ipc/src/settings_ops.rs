//! What the Settings screen sends and reads back.
//!
//! [`crate::settings`] holds the *stored* shape — the struct that lives in the workspace and
//! is persisted. This module holds the shape of the operations on it: the patch a screen
//! sends when a toggle flips, and the two reports it renders that are not settings at all
//! (the keymap's conflicts, and what the graphics ladder actually did to this process).
//!
//! The split is deliberate. A patch is not a `Settings` with holes in it — it is an
//! instruction, and "the user did not touch this" has to be distinguishable from "the user
//! set this to false" or every window that saves would overwrite the others' fields.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Theme;
use crate::settings::{
    ClaudeSettings, EditorSettings, GraphicsSettings, SidebarSettings, TerminalSettings,
};

/// A partial update to [`crate::Settings`]. `None` means "leave this alone".
///
/// Patching is per *top-level* field: a caller changing one editor option sends the whole
/// [`EditorSettings`] it currently holds. Per-leaf patching would need a parallel type for
/// every group and buy very little — two windows editing two different editor options in the
/// same 500 ms debounce is not a case worth a type hierarchy, and the loser sees the winner's
/// value arrive on the next snapshot rather than being told nothing happened.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
///
/// `window_mode` is deliberately absent. It is stored in `Settings`, but changing it is not
/// a stored-preference edit — it redistributes every open project across OS windows, which
/// means opening and closing real windows. That work already exists as `window.set_mode`,
/// and having two commands that can move the same field would leave the desktop agreeing
/// with the workspace only when the user happened to use the right one. The radio cards call
/// `window.set_mode`.
#[ts(export)]
pub struct SettingsPatch {
    // `#[ts(optional)]` throughout, so TypeScript sees `theme?: Theme` rather than a required
    // `theme: Theme | null`. Without it every caller would have to spell out eight nulls to
    // change one toggle, and the type would stop distinguishing "not mentioned" from
    // "explicitly cleared" at the only layer where a human writes it.
    #[ts(optional)]
    pub theme: Option<Theme>,
    #[ts(optional)]
    pub each_project_keeps_claude_tab: Option<bool>,
    #[ts(optional)]
    pub reopen_last_project: Option<bool>,
    #[ts(optional)]
    pub keep_sessions_on_window_close: Option<bool>,
    #[ts(optional)]
    pub confirm_close_with_live_session: Option<bool>,
    #[ts(optional)]
    pub editor: Option<EditorSettings>,
    #[ts(optional)]
    pub terminal: Option<TerminalSettings>,
    #[ts(optional)]
    pub graphics: Option<GraphicsSettings>,
    #[ts(optional)]
    pub claude: Option<ClaudeSettings>,
    /// Both sidebar widths, sent together.
    ///
    /// Per the rule above: a patch is per top-level field, so the splitter that just moved
    /// the explorer sends the git width it currently holds alongside it. That is not a
    /// theoretical loss of the other panel's value — the splitter reads both out of the same
    /// settings snapshot it renders from, so "currently holds" is the mirror's value and not
    /// a stale local copy.
    #[ts(optional)]
    pub sidebar: Option<SidebarSettings>,
}

/// Two or more commands competing for one keystroke in one context.
///
/// A wire mirror of `cide_core::keymap::Conflict` rather than that type itself, because
/// `ts-rs` exports per crate and `cargo xtask codegen` only reads `cide-ipc`'s bindings —
/// a `#[derive(TS)]` in `cide-core` lands in `crates/cide-core/bindings` and never reaches
/// `ui/src/ipc/generated.ts`. The domain type stays the domain's; this one is the wire's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeymapConflict {
    /// The normalised keystroke, e.g. `ctrl+shift+p`.
    pub key: String,
    /// The competing command ids in resolution order. The **last** one wins.
    pub commands: Vec<String>,
    /// The shared `when` clause, `None` when all of them are unconditional. Bindings whose
    /// contexts differ are not in conflict — that is how one key means two things in a
    /// terminal and in an editor.
    pub when: Option<String>,
}

/// Something in `keymap.json` that resolution recovered from rather than rejecting.
///
/// Flattened to one message from `cide_core::keymap::KeymapDiagnostic`'s variants on
/// purpose: the screen renders a line of prose either way, and exporting an internal enum to
/// the wire would freeze its variant names as a contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeymapProblem {
    pub key: String,
    pub command: String,
    pub message: String,
}

/// Everything Settings → Keymap draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeymapReport {
    /// Every binding after layering defaults → platform → user, each remembering its layer.
    pub bindings: Vec<crate::ResolvedBinding>,
    /// Keystrokes more than one command answers to. Reported, never resolved: which should
    /// win is a judgement only the user can make.
    pub conflicts: Vec<KeymapConflict>,
    /// Lines in the user's file that did not do what they look like they do.
    pub problems: Vec<KeymapProblem>,
    /// Where the overrides file lives, so the screen can name it. It need not exist — a
    /// fresh install has no overrides, which is not an error.
    pub path: String,
}

/// One rung of the Linux graphics ladder.
///
/// The whole point of surfacing these is that the failures they work around are invisible:
/// WebGL context creation succeeds on a software rasteriser, `WEBGL_debug_renderer_info` is
/// masked on Linux, and a DMABUF failure presents as a window that never paints. No probe
/// can pick the right rung, so the user gets the switch and the cost of each one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GraphicsRung {
    /// The environment variable, e.g. `WEBKIT_DISABLE_DMABUF_RENDERER`.
    pub variable: String,
    pub label: String,
    /// What turning it on costs. Shown next to the switch, because every rung costs
    /// something and a switch with no stated price gets flipped for no reason.
    pub cost: String,
    /// What the user has asked for. `None` is "leave it to the launcher's heuristic".
    pub setting: Option<bool>,
    /// Whether this process actually has the variable set — read from our own environment,
    /// not inferred from the setting.
    pub active: bool,
}

impl GraphicsRung {
    /// Whether the user's choice can only take effect after a restart.
    ///
    /// Always true when they differ: these variables are read while the webview is being
    /// created, and setting one afterwards does nothing at all. Saying so on screen is the
    /// difference between a switch that looks broken and one that is honest.
    pub fn restart_required(&self) -> bool {
        self.setting.is_some_and(|want| want != self.active)
    }
}

/// The ladder, plus whether the automatic heuristic is still in charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GraphicsStatus {
    pub rungs: Vec<GraphicsRung>,
    /// False once the user has set any rung explicitly.
    ///
    /// Taking an explicit choice as "you are driving now" is what lets a rung be turned
    /// *off* at all: the launcher's defaults are applied with set-if-unset semantics, so
    /// there is no value this side could write that means "and do not apply the default
    /// either". Overriding one rung therefore stands the whole heuristic down, and the
    /// screen says so rather than leaving the user to discover it.
    pub automatic: bool,
    /// True when `CIDE_NO_GRAPHICS_WORKAROUNDS` was set in the launching environment, which
    /// disables the lot regardless of what is stored here.
    pub suppressed_by_env: bool,
}
