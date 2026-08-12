//! The inline `--settings` payload every `claude` child is spawned with.
//!
//! # Why inline rather than writing a settings file
//!
//! `~/.claude/settings.json` is the user's. Editing it to install our hooks would outlive the
//! app, apply to every `claude` they run anywhere, and silently fight any other tool that
//! manages the same file. `--settings` takes a JSON blob on the command line, which is scoped
//! to exactly the child we spawned and disappears with it.
//!
//! # The statusline is chained, never replaced
//!
//! The status bar's live token and cost figures come from the statusline — it is the only
//! supported source for them. But a user who already has a statusline configured must not
//! lose it by opening their project in cide, so `cide-hook statusline` runs their command
//! first and prints its stdout verbatim before forwarding the JSON to us. See
//! [`StatusLine::chained`].

use serde_json::{Value, json};

use crate::hook::HookEvent;

/// How the statusline should be configured for a child.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StatusLine {
    /// The user has none configured; ours is the only one.
    #[default]
    Ours,
    /// The user has one. Ours runs it and passes its output through untouched.
    Chained(String),
    /// Do not configure a statusline at all.
    ///
    /// Not a hypothetical: the CLI refuses to run a statusline when workspace trust has not
    /// been accepted, and a user may simply not want one. The status bar falls back to
    /// showing no token figures rather than showing stale ones.
    None,
}

impl StatusLine {
    /// The command string for the `statusLine` setting.
    fn command(&self, hook_bin: &str) -> Option<String> {
        match self {
            Self::Ours => Some(format!("{hook_bin} statusline")),
            // The user's command is passed as an argument rather than interpolated into a
            // shell string: it may contain quotes, spaces or a `$` that a shell would expand
            // at the wrong moment, and `cide-hook` runs it itself.
            Self::Chained(user) => Some(format!("{hook_bin} statusline {user}")),
            Self::None => None,
        }
    }
}

/// Build the `--settings` value for one child.
///
/// `hook_bin` is the absolute path to `cide-hook`; it must be absolute because the child's
/// working directory is the project root and its `PATH` is the user's, neither of which is
/// guaranteed to contain our binary.
pub fn inline_settings(hook_bin: &str, status: &StatusLine, theme: ClaudeTheme) -> Value {
    let mut hooks = serde_json::Map::new();

    for event in HookEvent::ALL {
        hooks.insert(
            event.as_str().to_owned(),
            json!([{
                // Empty matcher means "every tool". The field is only meaningful for the
                // tool-scoped events, and the CLI documents an empty string as match-all, so
                // one shape serves all ten rather than branching per event.
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": format!("{hook_bin} {event}"),
                }],
            }]),
        );
    }

    let mut settings = json!({ "hooks": Value::Object(hooks) });

    if let Some(command) = status.command(hook_bin) {
        settings["statusLine"] = json!({
            // The CLI checks `type !== "command"` and silently skips anything else.
            "type": "command",
            "command": command,
        });
    }

    // Tell the CLI which way round its own palette should be.
    //
    // **This is the real fix for "white text in claude code".** Claude Code's `theme` defaults
    // to `dark`, whose `text` is `rgb(255,255,255)` — emitted as a 24-bit `SGR 38;2;255;255;255`
    // literal, not an ANSI slot, so no terminal palette we choose can touch it. In a white pane
    // that is 1.00:1: invisible. Its diff rows survived only because the CLI paints those with
    // backgrounds of its own, which is exactly the reported screenshot — two readable coloured
    // bars and nothing else.
    //
    // `xterm`'s `minimumContrastRatio` is the safety net and it lands that white on grey at
    // about 3.6:1: legible, and grey prose is not a palette working. This is the source.
    //
    // Set at spawn, so a theme switch does not re-theme a *running* session — the CLI reads
    // this once. A pane spawned after the switch is correct; an open one keeps what it was
    // given, which is the same limitation `--settings` has for hooks.
    settings["theme"] = Value::String(theme.as_str().to_owned());

    settings
}

/// Which way round Claude Code should draw itself.
///
/// Deliberately not cide's own `Theme`: this crate must not depend on `cide-ipc`, and these are
/// the CLI's own strings — `dark`, `light`, and the `-ansi` variants that resolve through the
/// terminal palette instead of 24-bit literals. Only the two are offered, because the `-ansi`
/// pair is what a user picks when their terminal theme is authoritative and ours is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeTheme {
    Dark,
    Light,
}

impl ClaudeTheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The palette the CLI draws itself in follows cide's.
    ///
    /// The bug this closes: Claude Code's `theme` defaults to `dark`, whose `text` is
    /// `rgb(255,255,255)` emitted as a 24-bit SGR literal rather than an ANSI slot — so no
    /// terminal palette can reach it, and in a white pane it is 1.00:1. Only the CLI can fix
    /// that, and this is the one channel we have to tell it.
    ///
    /// Asserted on the wire strings, not on the enum: these are the CLI's own vocabulary
    /// (`dark`, `light`, and the `-ansi` pair), read out of the shipped 2.1.228 binary. A
    /// rename here is a rename of something we do not own.
    #[test]
    fn the_cli_is_told_which_way_round_to_draw_itself() {
        let dark = inline_settings("/opt/cide/cide-hook", &StatusLine::None, ClaudeTheme::Dark);
        let light = inline_settings("/opt/cide/cide-hook", &StatusLine::None, ClaudeTheme::Light);
        assert_eq!(dark["theme"], "dark");
        assert_eq!(light["theme"], "light");
        // And the hooks are still there: the theme rides in the same `--settings` document, so
        // getting it wrong would cost the session its hooks as well as its colours.
        assert!(light["hooks"].is_object(), "{light}");
    }

    #[test]
    fn every_hook_event_is_registered_under_its_cli_name() {
        let s = inline_settings("/opt/cide/cide-hook", &StatusLine::Ours, ClaudeTheme::Dark);
        let hooks = s["hooks"].as_object().expect("hooks is an object");

        assert_eq!(hooks.len(), HookEvent::ALL.len());
        for event in HookEvent::ALL {
            let entry = &hooks[event.as_str()];
            assert_eq!(
                entry[0]["hooks"][0]["type"], "command",
                "{event}: the CLI ignores any type but `command`"
            );
            assert_eq!(
                entry[0]["hooks"][0]["command"],
                format!("/opt/cide/cide-hook {event}")
            );
            assert_eq!(entry[0]["matcher"], "", "empty matcher means every tool");
        }
    }

    #[test]
    fn the_statusline_is_a_command_because_nothing_else_is_honoured() {
        let s = inline_settings("/opt/cide/cide-hook", &StatusLine::Ours, ClaudeTheme::Dark);
        assert_eq!(s["statusLine"]["type"], "command");
        assert_eq!(s["statusLine"]["command"], "/opt/cide/cide-hook statusline");
    }

    #[test]
    fn a_users_own_statusline_is_chained_rather_than_replaced() {
        // Opening a project in cide must not cost someone the status line they already had.
        let s = inline_settings(
            "/opt/cide/cide-hook",
            &StatusLine::Chained("~/bin/my-status --fancy".into()),
            ClaudeTheme::Dark,
        );
        assert_eq!(
            s["statusLine"]["command"],
            "/opt/cide/cide-hook statusline ~/bin/my-status --fancy"
        );
    }

    #[test]
    fn no_statusline_means_the_key_is_absent_not_empty() {
        // An empty command string would be configured-and-broken rather than unconfigured.
        let s = inline_settings("/opt/cide/cide-hook", &StatusLine::None, ClaudeTheme::Dark);
        assert!(s.get("statusLine").is_none());
        assert_eq!(
            s["hooks"].as_object().expect("still has hooks").len(),
            HookEvent::ALL.len(),
            "dropping the statusline must not drop the hooks"
        );
    }

    #[test]
    fn the_payload_is_valid_json_for_a_command_line() {
        // It is passed as one argv entry to `--settings`, so it has to survive a round trip
        // through a string with no shell quoting of its own.
        let s = inline_settings("/opt/cide/cide-hook", &StatusLine::Ours, ClaudeTheme::Dark);
        let text = serde_json::to_string(&s).expect("serialises");
        let back: Value = serde_json::from_str(&text).expect("round trips");
        assert_eq!(back, s);
    }
}
