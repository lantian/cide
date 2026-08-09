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
pub fn inline_settings(hook_bin: &str, status: &StatusLine) -> Value {
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

    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hook_event_is_registered_under_its_cli_name() {
        let s = inline_settings("/opt/cide/cide-hook", &StatusLine::Ours);
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
        let s = inline_settings("/opt/cide/cide-hook", &StatusLine::Ours);
        assert_eq!(s["statusLine"]["type"], "command");
        assert_eq!(s["statusLine"]["command"], "/opt/cide/cide-hook statusline");
    }

    #[test]
    fn a_users_own_statusline_is_chained_rather_than_replaced() {
        // Opening a project in cide must not cost someone the status line they already had.
        let s = inline_settings(
            "/opt/cide/cide-hook",
            &StatusLine::Chained("~/bin/my-status --fancy".into()),
        );
        assert_eq!(
            s["statusLine"]["command"],
            "/opt/cide/cide-hook statusline ~/bin/my-status --fancy"
        );
    }

    #[test]
    fn no_statusline_means_the_key_is_absent_not_empty() {
        // An empty command string would be configured-and-broken rather than unconfigured.
        let s = inline_settings("/opt/cide/cide-hook", &StatusLine::None);
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
        let s = inline_settings("/opt/cide/cide-hook", &StatusLine::Ours);
        let text = serde_json::to_string(&s).expect("serialises");
        let back: Value = serde_json::from_str(&text).expect("round trips");
        assert_eq!(back, s);
    }
}
