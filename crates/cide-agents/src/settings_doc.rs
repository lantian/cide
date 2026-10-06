//! What every global setting means, for `cide_settings describe`. (M134)
//!
//! # Why a hand-written table rather than something derived
//!
//! The obvious sources both fail. The Rust `///` comments on `cide_ipc::settings` are the most
//! complete account there is, but they are not data — no macro in this workspace exports them,
//! and many of them are paragraphs about a bug rather than a sentence about the setting. The
//! Settings screen's copy (`ui/src/settings/sections.tsx`) is the sentence a person reads, but it
//! is JSX, and `ui/src/settings/settingsIndex.ts` is keyed by the visible label, not by the path a
//! write takes. Either would need a parser, and a parser of prose is a table with extra steps.
//!
//! So this is the table, and what keeps it honest is the test at the bottom: it walks
//! `Settings::default()` serialised, exactly as the tool does, and fails on a leaf nobody
//! described **and** on a description whose path no longer exists. A field added to `Settings`
//! without a line here is a red build, not a setting a model can change but cannot explain.
//!
//! Enum values are written in their wire spelling (`camelCase`), because that is what `set`
//! takes; ranges are the clamps `cmd::settings::apply_patch` applies, so a model that reads
//! `6–40` and sends `50` is told what it got rather than surprised by it.

/// One described setting.
pub struct SettingDoc {
    /// Dotted camelCase path from the root of `Settings`, as `cide_settings` takes it.
    pub path: &'static str,
    /// The value's shape: `bool`, `number 6–40`, `enum a | b`, `string`, `map`, …
    pub kind: &'static str,
    /// One line: what changes when this does.
    pub doc: &'static str,
}

/// Paths whose value is described, read and written **whole**: maps keyed by data, tagged
/// unions, and the `llm` group another pair of tools owns. The leaf walk stops here, so
/// `editor.formatters.rust` is not a path this table has to list — and cannot, since a language
/// is data rather than schema.
pub const OPAQUE: &[&str] = &[
    "accent",
    "editor.formatters",
    "inspections.sources",
    "inspections.serverBinaries",
    "remote.bind",
    "llm",
];

/// Every global setting. Grouped as `Settings` declares them.
pub const SETTING_DOCS: &[SettingDoc] = &[
    // --- top level -----------------------------------------------------------------------
    doc(
        "windowMode",
        "enum stacked | perProject",
        "Whether every project docks into one window (stacked) or each opens its own OS window.",
    ),
    doc(
        "theme",
        "enum dark | light",
        "The app's light or dark theme.",
    ),
    doc(
        "uiFontSize",
        "number 9–20",
        "Base point size of the chrome — tabs, panels, menus. Editor and terminal have their own.",
    ),
    doc(
        "accent",
        "string (hex colour) | null",
        "The accent colour. Set it with a hex such as \"#4f46e5\"; null or reset goes back to the \
         shipped red. The light and dark variants are fitted from it.",
    ),
    doc(
        "eachProjectKeepsClaudeTab",
        "bool",
        "Each project keeps its own pinned, non-closable Claude tab. Read-only true today.",
    ),
    doc(
        "awaitingHighlight",
        "enum accentRail | tintedHeader | paneOutline",
        "How panes waiting for you are highlighted in the accent colour. Applies across projects and windows.",
    ),
    doc(
        "reopenLastProject",
        "bool",
        "Reopen the last project on launch, with its tab strip and pane layout.",
    ),
    doc(
        "keepSessionsOnWindowClose",
        "bool",
        "Keep sessions running when a project window is closed (they end when cide quits).",
    ),
    doc(
        "confirmCloseWithLiveSession",
        "bool",
        "Ask before quitting while a session is busy or waiting on a permission.",
    ),
    doc(
        "consoleHarness",
        "enum claude | codex | opencode",
        "Which CLI a new project console runs.",
    ),
    doc(
        "openRunIn",
        "enum tab | split",
        "Where the Agents panel's Open shows a run: its own tab, or a split beside the console.",
    ),
    doc(
        "taskProposalRunMode",
        "enum background | tab",
        "How proposals requested during task creation are shown. Background keeps focus; tab opens the proposal run. Applies across projects.",
    ),
    // --- editor --------------------------------------------------------------------------
    doc("editor.fontSize", "number 6–40", "Editor font size."),
    doc("editor.tabSize", "number", "Columns per indentation level."),
    doc(
        "editor.insertSpaces",
        "bool",
        "Tab inserts spaces rather than a tab character.",
    ),
    doc(
        "editor.detectIndentation",
        "bool",
        "Read a file's own indentation before applying tabSize/insertSpaces.",
    ),
    doc(
        "editor.showMinimap",
        "bool",
        "Show the minimap beside the editor.",
    ),
    doc("editor.wordWrap", "bool", "Wrap long lines in the editor."),
    doc(
        "editor.trimTrailingWhitespaceOnSave",
        "bool",
        "Remove trailing whitespace when a file is saved.",
    ),
    doc(
        "editor.autosave",
        "bool",
        "Save a changed file when it loses focus, and after a minute with no edits.",
    ),
    doc(
        "editor.diffView",
        "enum unified | split",
        "How the diff surface shows a change: one column or side by side.",
    ),
    doc("editor.completion", "bool", "Code completion at all."),
    doc(
        "editor.completionOnTyping",
        "bool",
        "Open the completion popup while typing, rather than only on Ctrl+Space.",
    ),
    doc(
        "editor.colorSchemeLight",
        "string",
        "Editor colour scheme under the light theme (\"cide\" is built in; others are imported).",
    ),
    doc(
        "editor.colorSchemeDark",
        "string",
        "Editor colour scheme under the dark theme.",
    ),
    doc(
        "editor.formatters",
        "map language id → argv (list of strings)",
        "An external formatter per language; a language absent from the map is formatted by its \
         language server.",
    ),
    // --- terminal ------------------------------------------------------------------------
    doc(
        "terminal.fontSize",
        "number 6–40",
        "Terminal and Claude pane font size.",
    ),
    doc(
        "terminal.scrollback",
        "number",
        "Lines a shell pane keeps above the screen.",
    ),
    doc(
        "terminal.renderer",
        "enum auto | webgl | dom",
        "How terminals draw. auto picks WebGL and falls back to the DOM renderer.",
    ),
    doc(
        "terminal.jobNotifyAfterSecs",
        "number (seconds)",
        "How long a shell pane's foreground job runs before the pane announces it and notifies \
         when it ends.",
    ),
    doc(
        "terminal.jsonLogs",
        "bool",
        "A shell pane rewrites structured (JSON) log lines into readable ones.",
    ),
    doc(
        "terminal.showRecap",
        "bool",
        "Harness → User messages: browsable recap above Claude/Codex. Off by default.",
    ),
    doc(
        "terminal.highlightUserInput",
        "bool",
        "Harness → User messages: highlight Claude/Codex input independently of recap.",
    ),
    // --- graphics ------------------------------------------------------------------------
    doc(
        "graphics.disableDmabufRenderer",
        "bool | null",
        "WebKitGTK's DMA-BUF renderer off. null means cide decides. Takes effect after a restart.",
    ),
    doc(
        "graphics.disableCompositingMode",
        "bool | null",
        "WebKitGTK's accelerated compositing off. null means cide decides. After a restart.",
    ),
    doc(
        "graphics.disableNvidiaExplicitSync",
        "bool | null",
        "NVIDIA explicit sync off (a Wayland workaround). null means cide decides. After a restart.",
    ),
    // --- claude --------------------------------------------------------------------------
    doc(
        "claude.disableMouse",
        "bool",
        "Stop Claude's TUI capturing the mouse, so text selection works (CLAUDE_CODE_DISABLE_MOUSE).",
    ),
    doc(
        "claude.altScreenFullRepaint",
        "bool",
        "Repaint the whole alternate screen each frame (CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT).",
    ),
    doc(
        "claude.disableAlternateScreen",
        "bool",
        "Run Claude's TUI on the normal screen (CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN).",
    ),
    doc(
        "claude.resumeAllOnLaunch",
        "bool",
        "Resume every Claude pane on launch, rather than only the project's console.",
    ),
    doc(
        "claude.scrollSpeed",
        "number",
        "Transcript lines Claude's TUI moves per mouse-wheel step (CLAUDE_CODE_SCROLL_SPEED).",
    ),
    doc(
        "claude.cli.binary",
        "string",
        "The claude executable (a name on PATH or a path) — a wrapper script goes here.",
    ),
    doc(
        "claude.cli.args",
        "list of strings",
        "Extra arguments every claude child gets.",
    ),
    doc(
        "claude.cli.env",
        "list of {name, value}",
        "Extra environment every claude child gets. Values are never shown back.",
    ),
    doc(
        "claude.cli.inject.sessionId.enabled",
        "bool",
        "Pass --session-id. Off breaks resume: the CLI mints its own id.",
    ),
    doc(
        "claude.cli.inject.sessionId.flag",
        "string",
        "Spelling to use instead of --session-id; empty is the default.",
    ),
    doc(
        "claude.cli.inject.resume.enabled",
        "bool",
        "Pass --resume when resuming a pane.",
    ),
    doc(
        "claude.cli.inject.resume.flag",
        "string",
        "Spelling to use instead of --resume; empty is the default.",
    ),
    doc(
        "claude.cli.inject.forkSession.enabled",
        "bool",
        "Pass --fork-session on a split. Off makes a split a plain resume.",
    ),
    doc(
        "claude.cli.inject.forkSession.flag",
        "string",
        "Spelling to use instead of --fork-session; empty is the default.",
    ),
    doc(
        "claude.cli.inject.settings.enabled",
        "bool",
        "Pass --settings (cide's hooks). Off: panes never show busy or awaiting.",
    ),
    doc(
        "claude.cli.inject.settings.flag",
        "string",
        "Spelling to use instead of --settings; empty is the default.",
    ),
    doc(
        "claude.cli.inject.mcpConfig.enabled",
        "bool",
        "Pass --mcp-config (cide's tools, these ones included). Off: no cide tools in claude.",
    ),
    doc(
        "claude.cli.inject.mcpConfig.flag",
        "string",
        "Spelling to use instead of --mcp-config; empty is the default.",
    ),
    // --- opencode ---
    doc("opencode.cli.binary", "string", "The OpenCode executable."),
    doc(
        "opencode.cli.args",
        "list of strings",
        "Extra arguments every OpenCode child gets.",
    ),
    doc(
        "opencode.cli.env",
        "list of {name, value}",
        "Extra environment for OpenCode children; values are never shown back.",
    ),
    doc(
        "opencode.cli.inject.events",
        "bool",
        "Track native OpenCode state and conversation changes.",
    ),
    doc(
        "opencode.cli.inject.mcpConfig",
        "bool",
        "Give OpenCode cide’s MCP tools.",
    ),
    doc(
        "opencode.cli.inject.instructions",
        "bool",
        "Attach the console instructions.",
    ),
    doc(
        "opencode.cli.inject.resume",
        "bool",
        "Resume saved OpenCode conversations.",
    ),
    doc(
        "opencode.cli.inject.fork",
        "bool",
        "Fork OpenCode conversations when splitting.",
    ),
    // --- codex ---------------------------------------------------------------------------
    doc(
        "codex.cli.binary",
        "string",
        "The codex executable (a name on PATH or a path).",
    ),
    doc(
        "codex.cli.args",
        "list of strings",
        "Extra arguments every codex child gets.",
    ),
    doc(
        "codex.cli.env",
        "list of {name, value}",
        "Extra environment every codex child gets. Values are never shown back.",
    ),
    doc(
        "codex.cli.permissionMode",
        "enum useConfig | askForApproval | approveForMe | fullAccess | readOnly | customProfile",
        "Default permissions on the next Codex launch. Explicit role/review policies and CLI permission arguments take precedence; useConfig preserves existing defaults.",
    ),
    doc(
        "codex.cli.permissionProfile",
        "string",
        "Existing Codex permission-profile name, required when permissionMode is customProfile.",
    ),
    doc(
        "codex.cli.inject.hooks",
        "bool",
        "Pass cide's hooks to codex. Off: codex panes have no state and cannot resume.",
    ),
    doc(
        "codex.cli.inject.mcpConfig",
        "bool",
        "Give codex cide's MCP tools.",
    ),
    doc(
        "codex.cli.inject.developerInstructions",
        "bool",
        "Pass the console's roster paragraph or a run's brief as developer instructions.",
    ),
    doc(
        "codex.cli.inject.resume",
        "bool",
        "Resume a codex pane's thread on reopen.",
    ),
    doc(
        "codex.cli.inject.fork",
        "bool",
        "Fork a codex thread on split.",
    ),
    doc(
        "codex.cli.inject.permissions",
        "bool",
        "Pass sandbox and approval flags to runs and tabs cide opens by itself.",
    ),
    doc(
        "codex.cli.inject.gitPermissions",
        "bool",
        "Grant scoped Git writes to eligible editable Codex consoles and worker worktrees on their next launch. Custom and legacy permissions are preserved.",
    ),
    doc(
        "codex.cli.inject.reviewPermissions",
        "bool",
        "Suppress approval prompts for MR reviews.",
    ),
    // --- proxy ---------------------------------------------------------------------------
    doc(
        "proxy.mode",
        "enum inherit | manual | direct",
        "inherit: whatever the environment says. manual: the URLs below. direct: no proxy for \
         anything cide spawns.",
    ),
    doc(
        "proxy.scope.claude",
        "enum configured | untouched | direct",
        "Whether the proxy settings reach claude/codex children.",
    ),
    doc(
        "proxy.scope.shells",
        "enum configured | untouched | direct",
        "Whether the proxy settings reach shell panes.",
    ),
    doc(
        "proxy.scope.git",
        "enum configured | untouched | direct",
        "Whether the proxy settings reach git.",
    ),
    doc(
        "proxy.http",
        "string (URL)",
        "HTTP_PROXY under manual mode. Credentials in it are never shown back.",
    ),
    doc(
        "proxy.https",
        "string (URL)",
        "HTTPS_PROXY under manual mode. Credentials in it are never shown back.",
    ),
    doc(
        "proxy.all",
        "string (URL)",
        "ALL_PROXY under manual mode. Credentials in it are never shown back.",
    ),
    doc(
        "proxy.noProxy",
        "string (comma separated)",
        "Extra NO_PROXY entries; loopback is always exempt.",
    ),
    // --- sidebar -------------------------------------------------------------------------
    doc(
        "sidebar.filesWidth",
        "number 180–640",
        "Files panel width, in pixels.",
    ),
    doc(
        "sidebar.gitWidth",
        "number 180–640",
        "Git panel width, in pixels.",
    ),
    doc(
        "sidebar.agentsWidth",
        "number 180–640",
        "Agents panel width, in pixels.",
    ),
    doc(
        "sidebar.extWidth",
        "number 180–640",
        "Extensions panel width, in pixels.",
    ),
    // --- explorer ------------------------------------------------------------------------
    doc(
        "explorer.showHiddenFiles",
        "bool",
        "The file tree shows dotfiles. Changing it re-indexes open projects.",
    ),
    doc(
        "explorer.showIgnoredFiles",
        "bool",
        "The file tree shows gitignored files. Changing it re-indexes open projects.",
    ),
    // --- inspections ---------------------------------------------------------------------
    doc(
        "inspections.severities.error",
        "bool",
        "Show errors in the editor and Problems.",
    ),
    doc("inspections.severities.warning", "bool", "Show warnings."),
    doc(
        "inspections.severities.weakWarning",
        "bool",
        "Show weak warnings.",
    ),
    doc("inspections.severities.hint", "bool", "Show hints."),
    doc(
        "inspections.sources",
        "map analyser name → bool",
        "Which analysers' problems are shown (e.g. {\"clippy\": false}); absent means shown.",
    ),
    doc(
        "inspections.defaultHighlightLevel",
        "enum none | syntaxOnly | allProblems",
        "What an editor's highlighting level starts at.",
    ),
    doc(
        "inspections.pushToClaude",
        "bool",
        "Type a note into the project's Claude pane when new problems appear.",
    ),
    doc(
        "inspections.pushDebounceMs",
        "number 500–30000",
        "How long problems must settle before that note is typed.",
    ),
    doc(
        "inspections.serverMemoryLimitMb",
        "number (0 = off, else ≥ 256)",
        "Restart a language server that grows past this many megabytes.",
    ),
    doc(
        "inspections.serverIndexWorkingSetPct",
        "number (0 = default, else 25–400)",
        "Scale the bundled rust-analyzer's in-memory working set.",
    ),
    doc(
        "inspections.serverBinaries",
        "map server binary → builtin | system",
        "Which build of a language server runs; absent means builtin. Changing it restarts it.",
    ),
    // --- git -----------------------------------------------------------------------------
    doc(
        "git.pullStrategy",
        "enum ask | fastForward | merge | rebase",
        "What a pull does when the branches have diverged.",
    ),
    doc(
        "git.autoApplyNonConflicting",
        "bool",
        "The merge resolver applies one-sided changes without being asked.",
    ),
    // --- llm (owned elsewhere) -----------------------------------------------------------
    doc(
        "llm",
        "providers and pools",
        "Not changed here: use cide_llm_provider and cide_llm_pool, which never print a key.",
    ),
    // --- remote --------------------------------------------------------------------------
    doc(
        "remote.enabled",
        "bool",
        "Let a paired phone reach this cide over the network.",
    ),
    doc(
        "remote.bind",
        "{kind: loopback} | {kind: network} | {kind: address, address}",
        "Which address the remote listener binds.",
    ),
    doc(
        "remote.port",
        "number (0 = derived from the profile)",
        "The remote listener's port.",
    ),
    doc(
        "remote.allowNonPrivate",
        "bool",
        "Permit binding an address routable from the internet.",
    ),
    // --- update --------------------------------------------------------------------------
    doc(
        "update.checkOnStart",
        "bool",
        "Check for a new release a few seconds after start-up.",
    ),
    doc(
        "update.skippedVersion",
        "string | null",
        "The one release the user chose to skip; null for none.",
    ),
];

const fn doc(path: &'static str, kind: &'static str, doc: &'static str) -> SettingDoc {
    SettingDoc { path, kind, doc }
}

/// The description for exactly this path, if there is one.
#[must_use]
pub fn find(path: &str) -> Option<&'static SettingDoc> {
    SETTING_DOCS.iter().find(|entry| entry.path == path)
}

/// Whether the walk stops at `path` and treats its value as one leaf.
#[must_use]
pub fn is_opaque(path: &str) -> bool {
    OPAQUE.contains(&path)
}

/// Every leaf under `value`, as `(path, value)`, in declaration order.
///
/// An object is walked unless it is empty or [`is_opaque`]; everything else — scalars, `null`,
/// arrays — is a leaf. The same walk the tool renders with, which is why the coverage test
/// below can use it: a leaf this finds is a line `get` prints, and so a line that needs a doc.
pub fn leaves<'a>(
    value: &'a serde_json::Value,
    prefix: &str,
    out: &mut Vec<(String, &'a serde_json::Value)>,
) {
    match value {
        serde_json::Value::Object(map) if !map.is_empty() && !is_opaque(prefix) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                leaves(child, &path, out);
            }
        }
        _ => out.push((prefix.to_string(), value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drift guard: every leaf of a default `Settings` has a line, and every line names a
    /// leaf that exists. Fails naming the path, which is the one fact a person fixing it needs.
    #[test]
    fn every_setting_is_described_and_every_description_is_a_setting() {
        let value = serde_json::to_value(cide_ipc::settings::Settings::default()).unwrap();
        let mut found = Vec::new();
        leaves(&value, "", &mut found);
        let paths: Vec<&str> = found.iter().map(|(path, _)| path.as_str()).collect();
        for path in &paths {
            assert!(
                find(path).is_some(),
                "{path} is a setting with no line in SETTING_DOCS"
            );
        }
        for entry in SETTING_DOCS {
            assert!(
                paths.contains(&entry.path),
                "SETTING_DOCS describes {}, which Settings no longer has",
                entry.path
            );
        }
    }

    #[test]
    fn no_path_is_described_twice() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in SETTING_DOCS {
            assert!(seen.insert(entry.path), "{} is described twice", entry.path);
        }
    }
}
