//! User settings — the data behind the Settings tab.
//!
//! Field names and defaults track the design mock's Settings screen exactly, including the
//! toggles' default states, so the UI has nothing to invent.

use std::fmt;

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
    pub proxy: ProxySettings,
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
            proxy: ProxySettings::default(),
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

/// What cide does about the proxy variables in a child's environment.
///
/// Three states rather than a bool, because "do not proxy" and "do not interfere" are
/// different answers and a corporate laptop needs both. A single on/off switch would have to
/// pick one of them to be its off position: off-as-inherit leaves a user whose profile
/// exports `HTTP_PROXY` with no way to run a pane without it, and off-as-direct silently
/// breaks every pane the moment the app ships to somebody who does export one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProxyMode {
    /// Whatever the environment already says. cide sets nothing and scrubs nothing.
    ///
    /// The default, and the only mode in which a child's proxy environment is not entirely
    /// cide's doing. One exception, and it is not a proxy setting: if the inherited
    /// environment *does* name a proxy, the loopback exemption is still merged into
    /// `NO_PROXY` — see [`ProxySettings::no_proxy`].
    #[default]
    Inherit,
    /// The URLs below, overriding anything inherited. A blank field means the variable is
    /// **removed**, not left alone: a mode that says "this is the proxy" must not let a
    /// forgotten profile export supply the half the user left empty.
    Manual,
    /// No proxy for anything cide spawns. Every spelling is scrubbed from the child.
    Direct,
}

/// Proxy configuration, applied to every child cide spawns — `$SHELL` panes and `claude`
/// alike.
///
/// # This is where a password can end up
///
/// `http://user:hunter2@proxy.corp:3128` is a legal and common value, and this struct is
/// serialized into `workspace.json` in plain text, which is the only way a proxy that needs
/// credentials can work at all without a keyring cide does not have. What is *not* acceptable
/// is that value leaking sideways into a log the user then pastes into a bug report, so
/// [`Debug`] is implemented by hand and redacts the userinfo. That is deliberately at the
/// type level rather than at each call site: `tracing::debug!(?settings)` anywhere in the app
/// would otherwise print the password, and there is no way to review every future call site.
///
/// The alternative that lost was storing the credentials separately in the OS keyring. It is
/// the right answer and it is a milestone of its own — Secret Service over zbus, a fallback
/// for machines with no keyring daemon, and a migration for the value already on disk.
/// Redacted `Debug` is what makes the plain-text version defensible in the meantime.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ProxySettings {
    pub mode: ProxyMode,
    /// `HTTP_PROXY`/`http_proxy`. Empty means the variable is not set.
    pub http: String,
    /// `HTTPS_PROXY`/`https_proxy`. Empty **falls back to [`Self::http`]**, because one
    /// CONNECT proxy for both schemes is the overwhelmingly common shape and asking for it
    /// twice is how one of the two ends up stale.
    pub https: String,
    /// `ALL_PROXY`/`all_proxy`, usually a SOCKS URL.
    ///
    /// No fallback from [`Self::http`], unlike `https`: `ALL_PROXY` covers protocols beyond
    /// HTTP, and quietly pointing them at an HTTP CONNECT proxy the user only meant for web
    /// traffic changes behaviour they did not ask for.
    pub all: String,
    /// Extra `NO_PROXY` entries, comma separated — the corporate intranet, a registry mirror.
    ///
    /// `localhost`, `127.0.0.1` and `::1` are always prepended and cannot be removed. cide's
    /// IDE integration is an MCP server on loopback (`CLAUDE_CODE_SSE_PORT`), and a proxy
    /// that swallows loopback turns the headline feature — inline diffs, @-mentions, the
    /// editor selection — off with no error anywhere. Users do not think to exempt a port
    /// they were never told about.
    pub no_proxy: String,
}

impl ProxySettings {
    /// The `HTTPS_PROXY` value, after the fallback described on [`Self::https`].
    pub fn https_url(&self) -> Option<String> {
        normalize_proxy_url(&self.https).or_else(|| normalize_proxy_url(&self.http))
    }
}

impl fmt::Debug for ProxySettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxySettings")
            .field("mode", &self.mode)
            .field("http", &redact_proxy_url(&self.http))
            .field("https", &redact_proxy_url(&self.https))
            .field("all", &redact_proxy_url(&self.all))
            // Not a URL and cannot carry userinfo, so it is printed as it stands.
            .field("no_proxy", &self.no_proxy)
            .finish()
    }
}

/// A proxy URL as a child process should see it, or `None` for "not set".
///
/// Blank-is-unset rather than `Option<String>` on the struct: the UI binds text inputs to
/// these fields, and a control that has to distinguish "" from `None` grows a second piece of
/// state that disagrees with the first one.
///
/// A scheme is added when there is none. `proxy.corp:3128` is what people type and what curl
/// accepts (defaulting to HTTP), but `new URL()` in Node throws on it and the Claude CLI is
/// Node — so the same string would proxy a `curl` in a shell pane and fail in the pane next to
/// it. Normalising here means every consumer is handed the same unambiguous answer.
pub fn normalize_proxy_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") {
        Some(trimmed.to_string())
    } else {
        Some(format!("http://{trimmed}"))
    }
}

/// A proxy URL with any credentials removed, safe to put in a log line or an error message.
///
/// The host survives, because a redaction that hides the host too makes the log line useless
/// for the one question it is there to answer — which proxy did this child get. Only the
/// userinfo before `@` is a secret.
///
/// A string walk rather than a URL crate: this must never fail, and a parser that rejects a
/// malformed value would leave the caller holding the raw string with the password in it.
/// Anything this cannot make sense of is redacted whole.
pub fn redact_proxy_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        // Nothing to anchor on, so nothing can be shown to be safe.
        return if url.contains('@') {
            "***".to_string()
        } else {
            url.to_string()
        };
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    // `rsplit_once`, not `split_once`: a password may itself contain an `@`, and splitting on
    // the first one would print the tail of it.
    match rest[..authority_end].rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://***@{host}{}", &rest[authority_end..]),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The password must not survive `{:?}`, however the settings are printed.
    ///
    /// The whole struct, not the field: this is the leak that a future
    /// `tracing::debug!(?settings)` would cause, and the point of the manual `Debug` is that
    /// such a call site does not have to know about proxies to be safe.
    #[test]
    fn a_password_does_not_survive_debug() {
        let proxy = ProxySettings {
            mode: ProxyMode::Manual,
            http: "http://alice:hunter2@proxy.corp:3128".into(),
            https: "http://alice:hunter2@proxy.corp:3128".into(),
            all: "socks5://alice:hunter2@socks.corp:1080".into(),
            no_proxy: "corp.internal".into(),
        };
        let settings = Settings {
            proxy: proxy.clone(),
            ..Settings::default()
        };

        for printed in [format!("{proxy:?}"), format!("{settings:?}")] {
            assert!(!printed.contains("hunter2"), "password leaked: {printed}");
            assert!(!printed.contains("alice"), "username leaked: {printed}");
            // The host is what makes the line worth logging at all.
            assert!(printed.contains("proxy.corp:3128"), "host lost: {printed}");
        }
    }

    #[test]
    fn redaction_keeps_the_host_and_drops_the_userinfo() {
        assert_eq!(
            redact_proxy_url("http://u:p@proxy.corp:3128"),
            "http://***@proxy.corp:3128"
        );
        // An `@` inside the password must not put part of it on screen.
        assert_eq!(
            redact_proxy_url("http://u:p@ss@proxy.corp:3128"),
            "http://***@proxy.corp:3128"
        );
        // Nothing to hide: printed as it stands.
        assert_eq!(
            redact_proxy_url("http://proxy.corp:3128"),
            "http://proxy.corp:3128"
        );
        assert_eq!(redact_proxy_url(""), "");
        // Unparseable *and* carrying an `@`: redacted whole rather than guessed at.
        assert_eq!(redact_proxy_url("u:p@proxy.corp:3128"), "***");
    }

    #[test]
    fn a_bare_host_and_port_gains_the_scheme_node_insists_on() {
        assert_eq!(
            normalize_proxy_url(" proxy.corp:3128 ").as_deref(),
            Some("http://proxy.corp:3128")
        );
        assert_eq!(
            normalize_proxy_url("socks5h://localhost:9050").as_deref(),
            Some("socks5h://localhost:9050")
        );
        assert_eq!(normalize_proxy_url("   "), None);
    }

    /// The one-proxy-for-both case, which is what most corporate setups are.
    #[test]
    fn https_falls_back_to_http_but_never_the_other_way() {
        let proxy = ProxySettings {
            mode: ProxyMode::Manual,
            http: "http://proxy.corp:3128".into(),
            ..ProxySettings::default()
        };
        assert_eq!(proxy.https_url().as_deref(), Some("http://proxy.corp:3128"));

        let https_only = ProxySettings {
            mode: ProxyMode::Manual,
            https: "http://tls.corp:3129".into(),
            ..ProxySettings::default()
        };
        assert_eq!(
            https_only.https_url().as_deref(),
            Some("http://tls.corp:3129")
        );
        assert_eq!(
            normalize_proxy_url(&https_only.http),
            None,
            "http stays unset"
        );
    }

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
