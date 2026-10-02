//! The tools that answer "what is the hotkey for …", "turn on word wrap", "open the command
//! palette" — every setting cide has, the keymap, and the command registry. (M134)
//!
//! # Why these exist
//!
//! The M71 four (`cide_agents_config`, `cide_agent_override`, `cide_llm_provider`,
//! `cide_llm_pool`) let the console adjust what a role runs on. Everything else a person can
//! change in Settings was out of reach: a user who asked the console how to open the command
//! palette got an answer from the model's guess at a VS Code default, and one who asked it to
//! rebind a key was told to open Settings. The harness is the one surface in cide a person talks
//! to in sentences, and a sentence is the natural way to ask for most of these.
//!
//! # Every write goes through the road the Settings screen takes
//!
//! The sink's setters call the same `cmd::` functions the screen's `invoke`s land on —
//! `settings_set`, `keymap_edit`, `ext_set_setting`, `spec_settings_set`, the GitLab worker's
//! `Preferences` request — so every clamp, every side effect (a theme repaint, an LSP restart, a
//! remote listener reconcile, a file-tree re-walk) and every broadcast happens exactly as it does
//! for a click. A tool that wrote `workspace.json` itself would be a second writer that agreed
//! with the first until a side effect was added to one of them.
//!
//! # A setting is named by a dotted path, and written by the group it is in
//!
//! `cide_ipc::SettingsPatch` is per top-level group: to change `editor.wordWrap` the screen sends
//! the whole `EditorSettings`. The tool takes one leaf and composes that group from the settings
//! as they stand, so a model changing one field **cannot** change its siblings — the M71 rule that
//! a tool names one row and the read-modify-write happens on this side. The composed group is
//! deserialised through serde before anything is written, so a wrong type or an unknown enum
//! value is refused with serde's own sentence (which lists the variants) and nothing is stored.
//!
//! # Writes are the attended console's
//!
//! Reads are open to any pane served the orchestration vocabulary. A **write** is refused unless
//! the caller is [`crate::tools::CallerKind::Console`] — a pane the user opened or typed into. A tab cide spun
//! up by itself (the planner, a reviewer) has nobody behind it asking, and "the planner rebound
//! my save key" is not a thing anyone should ever have to diagnose. Roles never see these tools
//! at all: they are in [`crate::tools::tool::ORCHESTRATION`], which a run is not served.
//!
//! # A credential is written and never rendered
//!
//! The module header of [`crate::tools`] states the rule for API keys; it applies here to the
//! two places a secret lives in `Settings`: userinfo in a proxy URL, and the values of
//! `claude.cli.env` / `codex.cli.env` (which is where people put tokens). Both render as
//! [`HIDDEN`]. And because a model that read `<hidden>` will happily send it back, a `set` whose
//! value contains the placeholder is refused — the Models screen's masked-key argument
//! (`cide_ipc::SettingsPatch::llm`) arriving at the other door.
//!
//! # `cide_command_run` asks; it does not report
//!
//! The command runs in a webview, through the same `runCommand` the palette calls. The tool
//! emits the request and answers at once: there is no reply channel from `App.tsx`'s dispatcher,
//! and building one for this would make a model's turn wait on a window that may be minimised,
//! detached or closing. The answer says *requested*, which is true, rather than *done*, which
//! would not always be.

use serde_json::{Map, Value, json};

use cide_ipc::KeymapEdit;
use cide_ipc::settings::Settings;
use cide_ipc::settings_ops::{KeymapReport, SettingsPatch};

use crate::settings_doc;
use crate::tools::{AgentSink, ToolResult, tool};

/// What a hidden value renders as. Refused as input anywhere in a `set`.
pub const HIDDEN: &str = "<hidden>";

/// How many hits `cide_keymap find` prints.
const FIND_LIMIT: usize = 12;

// --- what `tools/list` says --------------------------------------------------------------------

pub(crate) fn description(name: &str) -> Option<&'static str> {
    Some(match name {
        tool::SETTINGS => {
            "Read or change any of cide's own settings — theme, fonts, editor, terminal, Claude/codex \
             CLI, proxy, inspections, git, remote, updates: everything on the Settings screen except \
             models (cide_llm_provider / cide_llm_pool), keybindings (cide_keymap), extension \
             settings (cide_extension_settings) and per-project settings (cide_project_settings, \
             cide_agents_config). A setting is a dotted camelCase `path` such as `editor.wordWrap`, \
             `terminal.fontSize` or `theme`. `action: describe` lists paths with their type, allowed \
             values and meaning — call it first when you do not know the path; `get` shows current \
             values beside their defaults; `set` takes `path` and `value` (JSON of the setting's \
             type) and changes that one setting, leaving its siblings alone; `reset` puts one back \
             to its default. Machine-wide, applied at once exactly as if the user had changed it in \
             Settings. Only change a setting when the user asked you to."
        }
        tool::KEYMAP => {
            "Keyboard shortcuts and commands. `action: find` with `query` (words like \"command \
             palette\", \"split right\", or a command id) lists matching commands with their current \
             keys — the way to answer \"how do I …\" and \"what is the shortcut for …\". `lookup` \
             with `key` (\"ctrl+shift+p\", sequences as \"ctrl+k ctrl+s\") says what a keystroke \
             does. `rebind` gives `command` the key `key` (replacing its keys in that context), \
             `unbind` removes a command's keys, `reset` restores its defaults, `resetAll` (with \
             `confirm: true`) discards every customisation. A rebind onto a key another command \
             holds is refused, naming it, until you pass `onConflict`: \"keep\" (both stay bound; \
             one will not fire) or \"unbindOther\" (the other command loses its keys in that \
             context). Only change the keymap when the user asked you to."
        }
        tool::EXTENSION_SETTINGS => {
            "Settings that installed cide extensions declare. `action: get` (optionally one \
             `extension`, as \"marketplace.extension\") lists each setting with its kind, allowed \
             values and current value; `set` with `extension`, `key` and `value` changes one. \
             Machine-wide."
        }
        tool::PROJECT_SETTINGS => {
            "Settings outside the machine-wide ones: `section: \"openspec\"` (this project's \
             OpenSpec options in .cide/config.json) and `section: \"gitlab\"` (GitLab review \
             preferences: excluded files and review prompts; accounts are connected from the GitLab \
             panel, never here). `action: get` shows the section; `set` with `field` and `value` \
             changes one field. `section: \"harness\"` reads local project overrides: consoleHarness, \
             claude, codex or opencode launch configuration; set overrides one field, reset removes \
             that field to inherit global settings. How roles run is cide_agents_config and cide_agent_override."
        }
        tool::COMMAND_RUN => {
            "Run one of cide's commands in the window showing this project, exactly as if the user \
             had picked it from the command palette — open the palette itself, a settings page, a \
             panel, a new terminal. Find the id with cide_keymap find. Answers once the request is \
             sent, not when the command has finished; a command that acts on the focused editor or \
             pane acts on whatever the user has focused. Only run a command when the user asked for \
             that to happen."
        }
        _ => return None,
    })
}

pub(crate) fn input_schema(name: &str) -> Option<Value> {
    Some(match name {
        tool::SETTINGS => json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["describe", "get", "set", "reset"],
                    "description": "describe: what the settings are. get: their values. set: change one. reset: back to its default.",
                },
                "path": {
                    "type": "string",
                    "description":
                        "Dotted camelCase path — a setting (`editor.wordWrap`) or a group \
                         (`editor`) for describe/get/reset. Omitted on describe/get: everything.",
                },
                "value": {
                    "description":
                        "For set: the new value, in the setting's own JSON type — true, 14, \
                         \"dark\", [\"--flag\"], {\"kind\": \"loopback\"}.",
                },
            },
            "required": ["action"],
        }),
        tool::KEYMAP => json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["find", "lookup", "rebind", "unbind", "reset", "resetAll"],
                },
                "query": {
                    "type": "string",
                    "description": "For find: words from the command's name, or its id.",
                },
                "key": {
                    "type": "string",
                    "description":
                        "For lookup and rebind: a keystroke such as \"ctrl+shift+p\", \
                         \"alt+enter\", \"f4\"; a sequence as \"ctrl+k ctrl+s\".",
                },
                "command": {
                    "type": "string",
                    "description": "For rebind/unbind/reset: the command id, as find prints it.",
                },
                "when": {
                    "type": ["string", "null"],
                    "description":
                        "The context the binding applies in, copied from what find prints. Omit \
                         it when the command has one context; null means everywhere.",
                },
                "onConflict": {
                    "type": "string",
                    "enum": ["keep", "unbindOther"],
                    "description": "For rebind onto a key another command holds.",
                },
                "allowBareKey": {
                    "type": "boolean",
                    "description":
                        "For rebind to a key with no ctrl/alt/meta that types a character: that \
                         character is then taken from every text field and terminal. Only when \
                         the user insists.",
                },
                "confirm": {
                    "type": "boolean",
                    "description": "Required true for resetAll.",
                },
            },
            "required": ["action"],
        }),
        tool::EXTENSION_SETTINGS => json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["get", "set"] },
                "extension": {
                    "type": "string",
                    "description": "\"marketplace.extension\", as get prints it.",
                },
                "key": { "type": "string", "description": "For set: the setting's id." },
                "value": { "description": "For set: the new value." },
            },
            "required": ["action"],
        }),
        tool::PROJECT_SETTINGS => json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["get", "set", "reset"] },
                "section": { "type": "string", "enum": ["openspec", "gitlab", "harness"] },
                "field": {
                    "type": "string",
                    "description": "For set: the camelCase field name, as get prints it.",
                },
                "value": { "description": "For set: the new value." },
            },
            "required": ["action", "section"],
        }),
        tool::COMMAND_RUN => json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command id, as cide_keymap find prints it.",
                },
                "args": {
                    "description": "Arguments, for the few commands that take them. Usually omitted.",
                },
            },
            "required": ["command"],
        }),
        _ => return None,
    })
}

/// Run one of these tools, or `None` when the name is not one of them.
pub(crate) fn dispatch(name: &str, arguments: &Value, sink: &dyn AgentSink) -> Option<ToolResult> {
    Some(match name {
        tool::SETTINGS => settings(arguments, sink),
        tool::KEYMAP => keymap(arguments, sink),
        tool::EXTENSION_SETTINGS => extension_settings(arguments, sink),
        tool::PROJECT_SETTINGS => project_settings(arguments, sink),
        tool::COMMAND_RUN => command_run(arguments, sink),
        _ => return None,
    })
}

// --- small readers -----------------------------------------------------------------------------

fn fail(tool: &str, why: impl std::fmt::Display) -> ToolResult {
    ToolResult::error(format!("{tool}: {why}"))
}

fn string_arg(arguments: &Value, key: &str) -> Result<Option<String>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.trim().to_string())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn required(arguments: &Value, key: &str) -> Result<String, String> {
    match string_arg(arguments, key)? {
        Some(text) if !text.is_empty() => Ok(text),
        _ => Err(format!("`{key}` is required")),
    }
}

fn flag(arguments: &Value, key: &str) -> bool {
    arguments.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The refusal every write shares. `None` when the caller may write.
fn write_refusal(tool: &str, sink: &dyn AgentSink) -> Option<ToolResult> {
    (!sink.caller().attended()).then(|| {
        fail(
            tool,
            "changes are made from the user's own console, when they ask for them — this tab was \
             opened by cide, so it may read settings but not change them",
        )
    })
}

fn contains_hidden(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains(HIDDEN),
        Value::Array(items) => items.iter().any(contains_hidden),
        Value::Object(map) => map.values().any(contains_hidden),
        _ => false,
    }
}

fn render_value(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "?".into())
}

// --- cide_settings -----------------------------------------------------------------------------

/// Split and check a dotted path. Empty segments are refused rather than skipped, so `editor.`
/// is a typo the model is told about rather than the whole group.
fn segments(path: &str) -> Result<Vec<&str>, String> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.iter().any(|part| part.trim().is_empty()) {
        return Err(format!(
            "`{path}` is not a setting path (a dotted name like editor.wordWrap)"
        ));
    }
    Ok(parts)
}

fn at<'a>(value: &'a Value, parts: &[&str]) -> Option<&'a Value> {
    parts.iter().try_fold(value, |node, part| node.get(*part))
}

/// The closest paths to one that does not exist — the parent's children, which is almost always
/// what a model that guessed `editor.wrap` needed to see.
fn nearby(root: &Value, parts: &[&str]) -> String {
    let mut depth = parts.len();
    while depth > 0 {
        depth -= 1;
        if let Some(Value::Object(map)) = at(root, &parts[..depth]) {
            let prefix = parts[..depth].join(".");
            let names: Vec<String> = map
                .keys()
                .map(|key| {
                    if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    }
                })
                .collect();
            return names.join(", ");
        }
    }
    String::new()
}

/// Settings as JSON, with every credential replaced by [`HIDDEN`].
fn redacted(settings: &Settings) -> Value {
    let mut value = serde_json::to_value(settings).unwrap_or(Value::Null);
    if let Some(proxy) = value.get_mut("proxy") {
        for field in ["http", "https", "all"] {
            if let Some(Value::String(url)) = proxy.get_mut(field) {
                *url = hide_userinfo(url);
            }
        }
    }
    for harness in ["claude", "codex", "opencode"] {
        if let Some(Value::Array(env)) = value
            .get_mut(harness)
            .and_then(|group| group.get_mut("cli"))
            .and_then(|cli| cli.get_mut("env"))
        {
            for entry in env {
                if let Some(Value::String(text)) = entry.get_mut("value")
                    && !text.is_empty()
                {
                    *text = HIDDEN.into();
                }
            }
        }
    }
    // Not rendered at all: `llm` is its own tools' table, and `cide_llm_provider`'s roster is
    // the one place that has already decided how a provider is shown without its key.
    if let Some(llm) = value.get_mut("llm") {
        *llm = Value::String("see cide_llm_provider / cide_llm_pool".into());
    }
    value
}

/// `http://user:pass@host:3128` → `http://<hidden>@host:3128`. Anything without an `@` in its
/// authority is returned as it came.
fn hide_userinfo(url: &str) -> String {
    let (scheme, rest) = match url.find("://") {
        Some(at) => (&url[..at + 3], &url[at + 3..]),
        None => ("", url),
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}{HIDDEN}{}", &rest[at..]),
        None => url.to_string(),
    }
}

fn settings(arguments: &Value, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::SETTINGS;
    let action = match required(arguments, "action") {
        Ok(action) => action,
        Err(why) => return fail(name, why),
    };
    let path = match string_arg(arguments, "path") {
        Ok(path) => path.filter(|path| !path.is_empty()),
        Err(why) => return fail(name, why),
    };
    match action.as_str() {
        "describe" => settings_describe(path.as_deref()),
        "get" => settings_get(path.as_deref(), sink),
        "set" | "reset" => {
            if let Some(refused) = write_refusal(name, sink) {
                return refused;
            }
            let Some(path) = path else {
                return fail(name, "`path` is required: name the one setting to change");
            };
            let parts = match segments(&path) {
                Ok(parts) => parts,
                Err(why) => return fail(name, why),
            };
            let parent = parts[..parts.len() - 1].join(".");
            let value = if action == "reset" && settings_doc::is_opaque(&parent) {
                // A key under a data map has no default of its own: absent *is* the default, and
                // `place` removes a key it is handed `null` for.
                Value::Null
            } else if action == "reset" {
                let defaults = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
                match at(&defaults, &parts) {
                    Some(value) => value.clone(),
                    None => {
                        return fail(
                            name,
                            format!("there is no setting `{path}` — call describe for the paths"),
                        );
                    }
                }
            } else {
                match arguments.get("value") {
                    Some(value) => value.clone(),
                    None => return fail(name, "`value` is required for set"),
                }
            };
            // A group is reset whole, never set whole: its struct deserialises with
            // `#[serde(default)]`, so an object naming two of fourteen fields would put the
            // other twelve back to their defaults — the very sibling damage a path exists to
            // prevent, arriving through a value instead of a patch.
            if action == "set" && !settings_doc::is_opaque(&path) {
                let groups = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
                if matches!(at(&groups, &parts), Some(Value::Object(map)) if !map.is_empty()) {
                    return fail(
                        name,
                        format!(
                            "`{path}` is a group — set its settings one at a time (describe \
                             lists them), or reset the whole group"
                        ),
                    );
                }
            }
            settings_set(&path, value, sink)
        }
        other => fail(
            name,
            format!("unknown action `{other}` — describe, get, set or reset"),
        ),
    }
}

fn settings_describe(path: Option<&str>) -> ToolResult {
    let lines: Vec<String> = settings_doc::SETTING_DOCS
        .iter()
        .filter(|entry| match path {
            None => true,
            Some(path) => entry.path == path || entry.path.starts_with(&format!("{path}.")),
        })
        .map(|entry| format!("{} — {} — {}", entry.path, entry.kind, entry.doc))
        .collect();
    if lines.is_empty() {
        return fail(
            tool::SETTINGS,
            format!(
                "no setting under `{}` — call describe with no path for the whole list",
                path.unwrap_or_default()
            ),
        );
    }
    ToolResult::text(lines.join("\n"))
}

fn settings_get(path: Option<&str>, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::SETTINGS;
    let current = match sink.settings() {
        Ok(settings) => settings,
        Err(why) => return fail(name, why),
    };
    let now = redacted(&current);
    let defaults = redacted(&Settings::default());
    let (root, prefix) = match path {
        None => (&now, String::new()),
        Some(path) => {
            let parts = match segments(path) {
                Ok(parts) => parts,
                Err(why) => return fail(name, why),
            };
            match at(&now, &parts) {
                Some(node) => (node, path.to_string()),
                None => {
                    return fail(
                        name,
                        format!(
                            "there is no setting `{path}`. Nearby: {}",
                            nearby(&now, &parts)
                        ),
                    );
                }
            }
        }
    };
    let mut found = Vec::new();
    settings_doc::leaves(root, &prefix, &mut found);
    let lines: Vec<String> = found
        .iter()
        .map(|(path, value)| {
            let default = segments(path)
                .ok()
                .and_then(|parts| at(&defaults, &parts).cloned());
            match default {
                Some(default) if default != **value => format!(
                    "{path} = {}   (default {})",
                    render_value(value),
                    render_value(&default)
                ),
                _ => format!("{path} = {}", render_value(value)),
            }
        })
        .collect();
    ToolResult::text(lines.join("\n"))
}

fn settings_set(path: &str, value: Value, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::SETTINGS;
    let parts = match segments(path) {
        Ok(parts) => parts,
        Err(why) => return fail(name, why),
    };
    if contains_hidden(&value) {
        return fail(
            name,
            format!(
                "`{HIDDEN}` is how a secret is shown, not a value — sending it back would replace \
                 the real one. Leave that entry out, or ask the user for the real value"
            ),
        );
    }
    let top = parts[0];
    match top {
        "llm" => {
            return fail(
                name,
                "models are changed with cide_llm_provider and cide_llm_pool, which edit one \
                 provider or pool at a time and never print a key",
            );
        }
        "windowMode" => {
            if parts.len() > 1 {
                return fail(name, "`windowMode` has no fields");
            }
            let mode: cide_ipc::WindowMode = match serde_json::from_value(value) {
                Ok(mode) => mode,
                Err(why) => return fail(name, format!("windowMode: {why}")),
            };
            return match sink.set_window_mode(mode) {
                Ok(()) => ToolResult::text(format!(
                    "windowMode = {}",
                    render_value(&serde_json::to_value(mode).unwrap_or(Value::Null))
                )),
                Err(why) => fail(name, why),
            };
        }
        "accent" => {
            if parts.len() > 1 {
                return fail(
                    name,
                    "set `accent` itself to a hex colour; its light and dark variants are fitted \
                     from it",
                );
            }
            let patch = match &value {
                Value::Null => cide_ipc::settings_ops::AccentPatch::Reset,
                Value::String(base) => {
                    cide_ipc::settings_ops::AccentPatch::Set { base: base.clone() }
                }
                // Reset hands the default's `null` here, and a stored accent object is what a
                // model that read `get` might send back — take its base rather than refuse.
                Value::Object(map) => match map.get("base").and_then(Value::as_str) {
                    Some(base) => cide_ipc::settings_ops::AccentPatch::Set { base: base.into() },
                    None => return fail(name, "accent: a hex colour such as \"#4f46e5\", or null"),
                },
                _ => return fail(name, "accent: a hex colour such as \"#4f46e5\", or null"),
            };
            let patch = SettingsPatch {
                accent: Some(patch),
                ..SettingsPatch::default()
            };
            return write_patch(path, patch, sink);
        }
        _ => {}
    }

    let current = match sink.settings() {
        Ok(settings) => settings,
        Err(why) => return fail(name, why),
    };
    let mut whole = match serde_json::to_value(&current) {
        Ok(whole) => whole,
        Err(why) => return fail(name, why),
    };
    if let Err(why) = place(&mut whole, &parts, value) {
        return fail(name, why);
    }
    let Some(group) = whole.get(top).cloned() else {
        return fail(name, format!("there is no setting `{path}`"));
    };
    let mut body = Map::new();
    body.insert(top.to_string(), group);
    let patch: SettingsPatch = match serde_json::from_value(Value::Object(body)) {
        Ok(patch) => patch,
        Err(why) => return fail(name, format!("{path}: {why}")),
    };
    write_patch(path, patch, sink)
}

/// Put `value` at `parts` inside `root`, refusing a path that does not already exist — except
/// one key below an [opaque](settings_doc::is_opaque) map, which is data (a language's formatter,
/// an analyser's switch) and so may be new. `null` there removes the key.
fn place(root: &mut Value, parts: &[&str], value: Value) -> Result<(), String> {
    let path = parts.join(".");
    let (last, parents) = parts.split_last().expect("segments is never empty");
    let parent_path = parents.join(".");
    let snapshot = root.clone();
    let mut node = &mut *root;
    for part in parents {
        node = match node.get_mut(*part) {
            Some(child) => child,
            None => {
                return Err(format!(
                    "there is no setting `{path}`. Nearby: {}",
                    nearby(&snapshot, parts)
                ));
            }
        };
    }
    let Value::Object(map) = node else {
        return Err(format!(
            "`{parent_path}` is a single value; set it directly"
        ));
    };
    let is_data_map = settings_doc::is_opaque(&parent_path) && !parent_path.is_empty();
    if !map.contains_key(*last) && !is_data_map {
        return Err(format!(
            "there is no setting `{path}`. Nearby: {}",
            nearby(&snapshot, parts)
        ));
    }
    if is_data_map && value.is_null() {
        map.remove(*last);
    } else {
        map.insert((*last).to_string(), value);
    }
    Ok(())
}

fn write_patch(path: &str, patch: SettingsPatch, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::SETTINGS;
    let after = match sink.set_settings(patch) {
        Ok(after) => after,
        Err(why) => return fail(name, why),
    };
    let now = redacted(&after);
    let parts = segments(path).unwrap_or_default();
    let mut found = Vec::new();
    match at(&now, &parts) {
        Some(node) => settings_doc::leaves(node, path, &mut found),
        // A key removed from a data map: say so rather than print nothing.
        None => return ToolResult::text(format!("{path} removed — the default applies")),
    }
    let mut lines: Vec<String> = found
        .iter()
        .map(|(path, value)| format!("{path} = {}", render_value(value)))
        .collect();
    if path.starts_with("graphics") {
        lines.push("Graphics settings take effect the next time cide starts.".into());
    }
    ToolResult::text(lines.join("\n"))
}

// --- cide_keymap -------------------------------------------------------------------------------

fn keymap(arguments: &Value, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::KEYMAP;
    let action = match required(arguments, "action") {
        Ok(action) => action,
        Err(why) => return fail(name, why),
    };
    if matches!(action.as_str(), "rebind" | "unbind" | "reset" | "resetAll")
        && let Some(refused) = write_refusal(name, sink)
    {
        return refused;
    }
    let report = match sink.keymap() {
        Ok(report) => report,
        Err(why) => return fail(name, why),
    };
    match action.as_str() {
        "find" => match required(arguments, "query") {
            Ok(query) => keymap_find(&query, &report),
            Err(why) => fail(name, why),
        },
        "lookup" => match required(arguments, "key") {
            Ok(key) => keymap_lookup(&key, &report),
            Err(why) => fail(name, why),
        },
        "rebind" => keymap_rebind(arguments, &report, sink),
        "unbind" | "reset" => keymap_remove(arguments, &action, &report, sink),
        "resetAll" => {
            if !flag(arguments, "confirm") {
                return fail(
                    name,
                    format!(
                        "resetAll discards all {} of the user's keybinding customisations; pass \
                         `confirm: true` once the user has said that is what they want",
                        report.overrides.len()
                    ),
                );
            }
            match sink.keymap_edit(vec![KeymapEdit::ResetAll]) {
                Ok(_) => ToolResult::text("Every keybinding is back to its default."),
                Err(why) => fail(name, why),
            }
        }
        other => fail(
            name,
            format!("unknown action `{other}` — find, lookup, rebind, unbind, reset or resetAll"),
        ),
    }
}

fn describe_when(when: Option<&str>) -> String {
    match when {
        Some(when) => format!(" (when {when})"),
        None => String::new(),
    }
}

fn layer_name(layer: cide_ipc::KeymapLayer) -> &'static str {
    match layer {
        cide_ipc::KeymapLayer::Default | cide_ipc::KeymapLayer::Platform => "default",
        cide_ipc::KeymapLayer::User => "customised",
    }
}

fn keys_of(command: &str, report: &KeymapReport) -> Vec<String> {
    report
        .bindings
        .iter()
        .filter(|binding| binding.command == command)
        .map(|binding| {
            format!(
                "{}{} [{}]",
                binding.key,
                describe_when(binding.when.as_deref()),
                layer_name(binding.layer)
            )
        })
        .collect()
}

fn render_command(command: &cide_ipc::Command, report: &KeymapReport) -> String {
    let keys = keys_of(&command.id, report);
    let keys = if keys.is_empty() {
        "no key".to_string()
    } else {
        keys.join(", ")
    };
    let mut line = format!(
        "{} — \"{}\" [{}]: {keys}",
        command.id, command.title, command.group
    );
    if let Some(why) = &command.unavailable {
        line.push_str(&format!(" — unavailable: {why}"));
    }
    line
}

fn keymap_find(query: &str, report: &KeymapReport) -> ToolResult {
    let mut hits: Vec<&cide_ipc::Command> = Vec::new();
    if let Some(exact) = cide_core::commands::by_id(query) {
        hits.push(exact);
    }
    for command in cide_core::commands::search(query) {
        if !hits.iter().any(|hit| hit.id == command.id) {
            hits.push(command);
        }
    }
    if hits.is_empty() {
        return ToolResult::text(format!(
            "No command matches \"{query}\". Try fewer or different words — the palette's own \
             search is what this runs."
        ));
    }
    let more = hits.len().saturating_sub(FIND_LIMIT);
    let mut lines: Vec<String> = hits
        .iter()
        .take(FIND_LIMIT)
        .map(|command| render_command(command, report))
        .collect();
    if more > 0 {
        lines.push(format!("… and {more} more; narrow the query"));
    }
    lines.push(
        "The command palette itself is `palette.commands`; run any of these with cide_command_run."
            .into(),
    );
    ToolResult::text(lines.join("\n"))
}

/// Every stroke-prefix of a normalised sequence, shortest first, the whole excluded.
fn prefixes(sequence: &str) -> Vec<String> {
    let strokes: Vec<&str> = sequence.split(' ').collect();
    (1..strokes.len()).map(|n| strokes[..n].join(" ")).collect()
}

fn title_of(command: &str) -> String {
    cide_core::commands::by_id(command)
        .map(|found| format!("{command} (\"{}\")", found.title))
        .unwrap_or_else(|| command.to_string())
}

fn keymap_lookup(key: &str, report: &KeymapReport) -> ToolResult {
    if let Err(why) = cide_core::keymap::parse_chord(key) {
        return fail(tool::KEYMAP, format!("`{key}` is not a key: {why}"));
    }
    let wanted = cide_core::keymap::normalize_key(key);
    let mut lines = Vec::new();
    for binding in &report.bindings {
        let existing = cide_core::keymap::normalize_key(&binding.key);
        let relation = if existing == wanted {
            ""
        } else if prefixes(&existing).contains(&wanted) {
            " — a sequence starting with it"
        } else {
            continue;
        };
        lines.push(format!(
            "{} → {}{}{} [{}]",
            binding.key,
            title_of(&binding.command),
            describe_when(binding.when.as_deref()),
            relation,
            layer_name(binding.layer)
        ));
    }
    if lines.is_empty() {
        return ToolResult::text(format!("{wanted} is not bound to anything."));
    }
    for conflict in &report.conflicts {
        if cide_core::keymap::normalize_key(&conflict.key) == wanted {
            lines.push(format!(
                "Conflict{}: {} all answer to it, and only one fires.",
                describe_when(conflict.when.as_deref()),
                conflict.commands.join(", ")
            ));
        }
    }
    ToolResult::text(lines.join("\n"))
}

/// The command a key edit names: in the registry, and runnable.
fn known_command(arguments: &Value) -> Result<&'static cide_ipc::Command, String> {
    let id = required(arguments, "command")?;
    let command = cide_core::commands::by_id(&id)
        .ok_or_else(|| format!("there is no command `{id}` — find its id with action find"))?;
    Ok(command)
}

/// Which context an edit names: the explicit `when` (null meaning everywhere), or — when the
/// argument is absent — the one context the command is bound in now.
///
/// Copied verbatim from the resolved row, because `keymap::apply_edit` matches a `when`
/// textually and an edit aimed at a context the command is not bound in removes nothing. Several
/// contexts and no `when` is a question for the model, not a guess.
fn context_for(
    arguments: &Value,
    command: &str,
    report: &KeymapReport,
) -> Result<Option<String>, String> {
    match arguments.get("when") {
        Some(Value::Null) => return Ok(None),
        Some(Value::String(when)) => {
            let when = when.trim();
            return Ok((!when.is_empty()).then(|| when.to_string()));
        }
        Some(_) => return Err("`when` must be a string or null".into()),
        None => {}
    }
    let mut contexts: Vec<Option<String>> = Vec::new();
    for binding in report.bindings.iter().filter(|b| b.command == command) {
        if !contexts.contains(&binding.when) {
            contexts.push(binding.when.clone());
        }
    }
    match contexts.len() {
        0 => Ok(None),
        1 => Ok(contexts.remove(0)),
        _ => Err(format!(
            "`{command}` is bound in {} contexts — pass `when` as one of: {}",
            contexts.len(),
            contexts
                .iter()
                .map(|when| when
                    .as_deref()
                    .map_or("null".into(), |w| format!("\"{w}\"")))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// What binding `key` in `when` would run into, as `(command, its when, kind)`.
///
/// The Rust twin of `ui/src/settings/keymapModel.ts::clashesFor`: the same key in an overlapping
/// context, or one being a stroke-prefix of the other (a prefix arms the sequence machine, so
/// the shorter binding can never fire). Overlapping means equal, or either side unscoped — an
/// unscoped binding applies everywhere. It lacks the UI's key-alias table (`ctrl+backquote` vs
/// `` ctrl+` ``), so it can miss a clash spelled two ways; never invent one.
fn clashes(
    report: &KeymapReport,
    key: &str,
    when: Option<&str>,
    command: &str,
) -> Vec<(String, Option<String>, &'static str)> {
    let candidate = cide_core::keymap::normalize_key(key);
    let mut out = Vec::new();
    for binding in &report.bindings {
        if binding.command == command || binding.command.starts_with('-') {
            continue;
        }
        let overlaps = binding.when.as_deref() == when || binding.when.is_none() || when.is_none();
        if !overlaps {
            continue;
        }
        let existing = cide_core::keymap::normalize_key(&binding.key);
        let kind = if existing == candidate {
            "holds that key"
        } else if prefixes(&existing).contains(&candidate) {
            "has a sequence starting with it, which would stop firing"
        } else if prefixes(&candidate).contains(&existing) {
            "holds its first stroke, which would stop firing"
        } else {
            continue;
        };
        out.push((binding.command.clone(), binding.when.clone(), kind));
    }
    out
}

/// Whether a key's first stroke types a character: no ctrl, alt or meta, and a key that is one
/// printable character (or space). Such a binding takes the character from every text field.
fn types_a_character(key: &str) -> bool {
    let Ok(chords) = cide_core::keymap::parse_chord(key) else {
        return false;
    };
    let Some(first) = chords.first() else {
        return false;
    };
    !first.ctrl
        && !first.alt
        && !first.meta
        && (first.key.chars().count() == 1 || first.key == "space")
}

fn keymap_rebind(arguments: &Value, report: &KeymapReport, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::KEYMAP;
    let command = match known_command(arguments) {
        Ok(command) => command,
        Err(why) => return fail(name, why),
    };
    if let Some(why) = &command.unavailable {
        return fail(
            name,
            format!(
                "`{}` cannot run in this build ({why}), so nothing may be bound to it",
                command.id
            ),
        );
    }
    let key = match required(arguments, "key") {
        Ok(key) => key,
        Err(why) => return fail(name, why),
    };
    if let Err(why) = cide_core::keymap::parse_chord(&key) {
        return fail(name, format!("`{key}` is not a key: {why}"));
    }
    if types_a_character(&key) && !flag(arguments, "allowBareKey") {
        return fail(
            name,
            format!(
                "`{key}` has no ctrl/alt/meta, so binding it takes that character from every text \
                 field and terminal in cide. Suggest a chord with a modifier; pass \
                 `allowBareKey: true` only if the user insists"
            ),
        );
    }
    let when = match context_for(arguments, &command.id, report) {
        Ok(when) => when,
        Err(why) => return fail(name, why),
    };
    let clashes = clashes(report, &key, when.as_deref(), &command.id);
    let on_conflict = match string_arg(arguments, "onConflict") {
        Ok(choice) => choice,
        Err(why) => return fail(name, why),
    };
    let mut edits = vec![KeymapEdit::Rebind {
        command: command.id.clone(),
        when: when.clone(),
        key: key.clone(),
    }];
    let mut notes = Vec::new();
    if !clashes.is_empty() {
        match on_conflict.as_deref() {
            None => {
                let list: Vec<String> = clashes
                    .iter()
                    .map(|(other, other_when, kind)| {
                        format!(
                            "{}{} {kind}",
                            title_of(other),
                            describe_when(other_when.as_deref())
                        )
                    })
                    .collect();
                return fail(
                    name,
                    format!(
                        "not rebound — {}. Ask the user, then call again with onConflict: \
                         \"unbindOther\" (the other command loses its keys in that context) or \
                         \"keep\" (both stay bound)",
                        list.join("; ")
                    ),
                );
            }
            Some("keep") => notes.push(format!(
                "Kept the other binding{} on it — check with lookup which one fires.",
                if clashes.len() == 1 { "" } else { "s" }
            )),
            Some("unbindOther") => {
                for (other, other_when, _) in &clashes {
                    let edit = KeymapEdit::Unbind {
                        command: other.clone(),
                        when: other_when.clone(),
                    };
                    if !edits.contains(&edit) {
                        edits.push(edit);
                        notes.push(format!(
                            "Unbound {}{}.",
                            title_of(other),
                            describe_when(other_when.as_deref())
                        ));
                    }
                }
            }
            Some(other) => {
                return fail(name, format!("onConflict `{other}` — keep or unbindOther"));
            }
        }
    }
    let after = match sink.keymap_edit(edits) {
        Ok(after) => after,
        Err(why) => return fail(name, why),
    };
    let keys = keys_of(&command.id, &after);
    let mut text = format!(
        "{} (\"{}\") is now on: {}.",
        command.id,
        command.title,
        if keys.is_empty() {
            "no key".into()
        } else {
            keys.join(", ")
        }
    );
    for note in notes {
        text.push(' ');
        text.push_str(&note);
    }
    ToolResult::text(text)
}

fn keymap_remove(
    arguments: &Value,
    action: &str,
    report: &KeymapReport,
    sink: &dyn AgentSink,
) -> ToolResult {
    let name = tool::KEYMAP;
    let command = match known_command(arguments) {
        Ok(command) => command,
        Err(why) => return fail(name, why),
    };
    let when = match context_for(arguments, &command.id, report) {
        Ok(when) => when,
        Err(why) => return fail(name, why),
    };
    // Reset is aimed at the user's own entries, so its context comes from those when the
    // command's live bindings do not name one — a command the user unbound has no live row.
    let when = if action == "reset" && arguments.get("when").is_none() && when.is_none() {
        report
            .overrides
            .iter()
            .find(|binding| binding.target_command() == command.id)
            .and_then(|binding| binding.when.clone())
    } else {
        when
    };
    let edit = if action == "unbind" {
        KeymapEdit::Unbind {
            command: command.id.clone(),
            when,
        }
    } else {
        KeymapEdit::Reset {
            command: command.id.clone(),
            when,
        }
    };
    let after = match sink.keymap_edit(vec![edit]) {
        Ok(after) => after,
        Err(why) => return fail(name, why),
    };
    let keys = keys_of(&command.id, &after);
    ToolResult::text(format!(
        "{} (\"{}\") is now on: {}.",
        command.id,
        command.title,
        if keys.is_empty() {
            "no key".into()
        } else {
            keys.join(", ")
        }
    ))
}

// --- cide_extension_settings -------------------------------------------------------------------

fn extension_settings(arguments: &Value, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::EXTENSION_SETTINGS;
    let action = match required(arguments, "action") {
        Ok(action) => action,
        Err(why) => return fail(name, why),
    };
    let wanted = match string_arg(arguments, "extension") {
        Ok(wanted) => wanted.filter(|w| !w.is_empty()),
        Err(why) => return fail(name, why),
    };
    let installed = match sink.extensions() {
        Ok(installed) => installed,
        Err(why) => return fail(name, why),
    };
    let chosen: Vec<&cide_ipc::ext::InstalledExtension> = installed
        .iter()
        .filter(|ext| wanted.as_deref().is_none_or(|w| ext.id.to_string() == w))
        .collect();
    if let Some(wanted) = &wanted
        && chosen.is_empty()
    {
        let names: Vec<String> = installed.iter().map(|ext| ext.id.to_string()).collect();
        return fail(
            name,
            format!(
                "no installed extension `{wanted}`. Installed: {}",
                if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                }
            ),
        );
    }
    match action.as_str() {
        "get" => {
            let mut lines = Vec::new();
            for ext in &chosen {
                if ext.contributes.settings.is_empty() {
                    continue;
                }
                lines.push(format!("{} — {}", ext.id, ext.name));
                for def in &ext.contributes.settings {
                    let value = def.coerce(ext.settings.get(&def.id));
                    lines.push(format!(
                        "  {} = {} — {} ({}){}",
                        def.id,
                        render_value(&value),
                        def.label,
                        setting_kind(&def.kind),
                        def.description
                            .as_deref()
                            .map(|d| format!(": {d}"))
                            .unwrap_or_default()
                    ));
                }
            }
            if lines.is_empty() {
                return ToolResult::text("No installed extension declares any settings.");
            }
            ToolResult::text(lines.join("\n"))
        }
        "set" => {
            if let Some(refused) = write_refusal(name, sink) {
                return refused;
            }
            if wanted.is_none() {
                return fail(name, "`extension` is required for set");
            }
            let ext = chosen[0];
            let key = match required(arguments, "key") {
                Ok(key) => key,
                Err(why) => return fail(name, why),
            };
            let Some(def) = ext.contributes.settings.iter().find(|def| def.id == key) else {
                let ids: Vec<&str> = ext
                    .contributes
                    .settings
                    .iter()
                    .map(|d| d.id.as_str())
                    .collect();
                return fail(
                    name,
                    format!(
                        "{} has no setting `{key}`. Its settings: {}",
                        ext.id,
                        ids.join(", ")
                    ),
                );
            };
            let Some(value) = arguments.get("value").cloned() else {
                return fail(name, "`value` is required for set");
            };
            // Coerced here as well as in `ExtStore::set_setting`, so a value the store would
            // quietly replace with the default is refused with the reason instead.
            let coerced = def.coerce(Some(&value));
            if coerced != value && !(value.is_number() && coerced.is_number()) {
                return fail(
                    name,
                    format!(
                        "{key}: {} is not a {} value",
                        render_value(&value),
                        setting_kind(&def.kind)
                    ),
                );
            }
            match sink.set_extension_setting(&ext.id, &key, coerced.clone()) {
                Ok(()) => {
                    ToolResult::text(format!("{}: {key} = {}", ext.id, render_value(&coerced)))
                }
                Err(why) => fail(name, why),
            }
        }
        other => fail(name, format!("unknown action `{other}` — get or set")),
    }
}

fn setting_kind(kind: &cide_ipc::ext::SettingKind) -> String {
    use cide_ipc::ext::SettingKind;
    match kind {
        SettingKind::Toggle { .. } => "bool".into(),
        SettingKind::Text { .. } => "text".into(),
        SettingKind::Number { min, max, .. } => match (min, max) {
            (Some(min), Some(max)) => format!("number {min}–{max}"),
            (Some(min), None) => format!("number ≥ {min}"),
            (None, Some(max)) => format!("number ≤ {max}"),
            (None, None) => "number".into(),
        },
        SettingKind::Choice { choices, .. } => format!(
            "one of {}",
            choices
                .iter()
                .map(|choice| choice.value.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        ),
    }
}

// --- cide_project_settings ---------------------------------------------------------------------

fn project_settings(arguments: &Value, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::PROJECT_SETTINGS;
    let (action, section) = match (
        required(arguments, "action"),
        required(arguments, "section"),
    ) {
        (Ok(action), Ok(section)) => (action, section),
        (Err(why), _) | (_, Err(why)) => return fail(name, why),
    };
    if section == "harness" {
        let result = if action == "get" {
            sink.project_harness()
        } else if matches!(action.as_str(), "set" | "reset") {
            if let Some(refused) = write_refusal(name, sink) {
                return refused;
            }
            let field = match required(arguments, "field") {
                Ok(field) => field,
                Err(why) => return fail(name, why),
            };
            let value = if action == "reset" {
                Value::Null
            } else {
                match arguments.get("value") {
                    Some(value) => value.clone(),
                    None => return fail(name, "value is required for set"),
                }
            };
            if contains_hidden(&value) {
                return fail(
                    name,
                    format!("`{HIDDEN}` is a secret placeholder, not a launch setting value"),
                );
            }
            let edit = match serde_json::from_value::<cide_ipc::ProjectHarnessEdit>(
                json!({ "field": field, "value": value }),
            ) {
                Ok(edit) => edit,
                Err(error) => return fail(name, error.to_string()),
            };
            sink.set_project_harness(edit)
        } else {
            return fail(name, "action must be get, set or reset");
        };
        return match result {
            Ok(settings) => {
                let mut value = serde_json::to_value(settings).unwrap_or_default();
                for harness in ["claude", "codex", "opencode"] {
                    if let Some(env) = value
                        .get_mut(harness)
                        .and_then(|v| v.get_mut("env"))
                        .and_then(Value::as_array_mut)
                    {
                        for variable in env {
                            variable["value"] = json!(HIDDEN);
                        }
                    }
                }
                ToolResult::text(value.to_string())
            }
            Err(error) => fail(name, error),
        };
    }
    let current = match section.as_str() {
        "openspec" => sink
            .spec_settings()
            .and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string())),
        "gitlab" => sink
            .gitlab_preferences()
            .and_then(|p| serde_json::to_value(p).map_err(|e| e.to_string())),
        other => {
            return fail(
                name,
                format!("unknown section `{other}` — openspec or gitlab"),
            );
        }
    };
    let current = match current {
        Ok(current) => current,
        Err(why) => return fail(name, why),
    };
    match action.as_str() {
        "get" => {
            let lines: Vec<String> = current
                .as_object()
                .map(|map| {
                    map.iter()
                        .map(|(field, value)| format!("{field} = {}", render_value(value)))
                        .collect()
                })
                .unwrap_or_default();
            ToolResult::text(lines.join("\n"))
        }
        "set" => {
            if let Some(refused) = write_refusal(name, sink) {
                return refused;
            }
            let field = match required(arguments, "field") {
                Ok(field) => field,
                Err(why) => return fail(name, why),
            };
            let Some(value) = arguments.get("value").cloned() else {
                return fail(name, "`value` is required for set");
            };
            let mut whole = current;
            let Some(map) = whole.as_object_mut() else {
                return fail(name, "that section has no fields");
            };
            if !map.contains_key(&field) {
                let fields: Vec<&str> = map.keys().map(String::as_str).collect();
                return fail(
                    name,
                    format!(
                        "{section} has no field `{field}`. Its fields: {}",
                        fields.join(", ")
                    ),
                );
            }
            map.insert(field.clone(), value);
            let written = match section.as_str() {
                "openspec" => serde_json::from_value(whole)
                    .map_err(|e| format!("{field}: {e}"))
                    .and_then(|s| sink.set_spec_settings(s)),
                _ => serde_json::from_value(whole)
                    .map_err(|e| format!("{field}: {e}"))
                    .and_then(|p| sink.set_gitlab_preferences(p)),
            };
            match written {
                Ok(()) => ToolResult::text(format!("{section}.{field} changed.")),
                Err(why) => fail(name, why),
            }
        }
        other => fail(name, format!("unknown action `{other}` — get or set")),
    }
}

// --- cide_command_run --------------------------------------------------------------------------

fn command_run(arguments: &Value, sink: &dyn AgentSink) -> ToolResult {
    let name = tool::COMMAND_RUN;
    if let Some(refused) = write_refusal(name, sink) {
        return refused;
    }
    let command = match known_command(arguments) {
        Ok(command) => command,
        Err(why) => return fail(name, why),
    };
    if let Some(why) = &command.unavailable {
        return fail(
            name,
            format!("`{}` cannot run in this build: {why}", command.id),
        );
    }
    let args = arguments
        .get("args")
        .cloned()
        .filter(|args| !args.is_null());
    match sink.run_command(&command.id, args) {
        Ok(()) => ToolResult::text(format!(
            "Asked the window to run {} (\"{}\"). This says it was requested, not that it \
             finished.",
            command.id, command.title
        )),
        Err(why) => fail(name, why),
    }
}

#[cfg(test)]
mod tests;
