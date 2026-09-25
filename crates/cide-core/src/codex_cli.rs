//! Launching `codex`: the user's configuration, cide's own additions, and the rules over both.
//! (M93)
//!
//! [`crate::claude_cli`]'s twin for the other CLI a console or a run may be. Everything that
//! module's header argues about *where* rules live holds here: the user's launch configuration
//! is stored verbatim ([`cide_ipc::CodexCli`]) and filtered at the spawn, the screen and the spawn
//! read one [`Plan`] so they cannot disagree, and a bare binary name is resolved by the OS at each
//! spawn rather than pinned.
//!
//! # What cide adds, and why every piece is a `-c` override
//!
//! Codex has no `--settings` and no `--mcp-config`: every one of its configuration surfaces is
//! `config.toml`, and `-c key=value` is the per-invocation override of any key in it (the value
//! parsed as TOML, falling back to a literal string). cide writes into neither the user's
//! `$CODEX_HOME/config.toml` nor their project's `.codex/`, so everything it adds is an override
//! on this one command line and disappears with the child — claude's `--settings` argument in a
//! different spelling:
//!
//! * **hooks** — `-c hooks.<Event>=[{matcher="",hooks=[{type="command",command="<cide-hook>
//!   <Event>"}]}]` per event ([`hook_overrides`]). Measured on 0.155.1 in the interactive TUI:
//!   the payloads are Claude-shaped (`session_id`, `transcript_path`, `cwd`,
//!   `hook_event_name`, `turn_id`, `last_assistant_message`), the command inherits the TUI's
//!   environment (so `CIDE_SESSION` and `CIDE_HOOK_SOCK` reach `cide-hook`), and they fire
//!   **only with `--dangerously-bypass-hook-trust`** — codex otherwise runs a hook only once
//!   its hash is persisted as trusted, which is a write into the user's config. The flag's own
//!   help: *"Intended only for automation that already vets hook sources"*, which is exactly
//!   this — the only hook it trusts is one cide wrote, pointing at cide's own binary. M44 named
//!   this road as unmeasured; it is the reason codex can be a console at all.
//! * **cide's MCP server** — `-c mcp_servers.cide.{command,args,env.*}` ([`mcp_overrides`]).
//!   The environment has to be spelled into the server's own `env` table because codex starts a
//!   stdio server with a whitelisted environment, not its own — the bridge would otherwise start,
//!   find no `CIDE_SESSION`/`CIDE_RUN`, and scope itself to nothing (`harness/codex.rs`'s header
//!   measured it).
//! * **the brief** — `-c developer_instructions=<string>` ([`developer_instructions`]): the
//!   first `developer` message, ahead of codex's own. Replaces a `developer_instructions` the
//!   user keeps in their own config, for this child only.
//! * **a quiet start** — [`QUIET_START`]. Two startup modals were measured to eat the first key
//!   a cide-driven child is sent: the self-update chooser (whose first row is *Update now*, so an
//!   Enter typed to submit a prompt **updated codex** — it happened during the M93 probe) and the
//!   rate-limit "switch to a cheaper model?" nudge. Both are suppressed per invocation.
//!
//! Every string that reaches a `-c` value goes through [`toml_string`], because a value that
//! happens to parse as another TOML type is a hard error and a multi-line one is not TOML at all.
//!
//! # The argv shape
//!
//! `codex [resume|fork] <user args> <cide's options> [<thread>] [<prompt>]`. The `resume` and
//! `fork` subcommands accept every TUI option (`codex resume --help`, 0.156.1: `-c`, `-C`, `-s`,
//! `-a`, `--dangerously-bypass-hook-trust`, …), so the options are written *after* the
//! subcommand where each certainly parses them, and the thread id and prompt are the trailing
//! positionals — nothing may follow them.

use std::path::PathBuf;

use cide_ipc::CodexCli;

use crate::child_env::EnvChange;
use crate::claude_cli::{BinaryProblem, Judged, Verdict};

/// Config overrides that keep a cide-driven codex from opening a modal before its first
/// prompt. See the module header for what each one cost when it was not there.
pub const QUIET_START: &[&str] = &[
    "check_for_update_on_startup=false",
    "notice.hide_rate_limit_model_nudge=true",
];

/// The flag without which command-line hooks do not fire. See the module header.
pub const BYPASS_HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";

/// The name cide's MCP server is registered under, which is also the middle of every tool name
/// the model sees (`mcp__cide__cide_task_get`).
pub const SERVER: &str = "cide";

/// Removes the wrapper separator from user arguments so cide can put it after its own options.
/// A configured wrapper such as `o3 agent codex --` must see cide's `-C`, hooks and MCP options
/// on the Codex side of that separator rather than treating them as wrapper positionals.
pub fn remove_wrapper_separator(args: &mut Vec<String>) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == "--") {
        args.remove(index);
        true
    } else {
        false
    }
}

/// Which of cide's additions a spawn makes. Resolved from [`cide_ipc::CodexInjections`] once, in
/// [`plan`], so the verdicts on the user's own tokens and the argv agree — `claude_cli`'s rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Injected {
    pub hooks: bool,
    pub mcp_config: bool,
    pub developer_instructions: bool,
    pub resume: bool,
    pub fork: bool,
}

impl Default for Injected {
    /// Nothing: what a caller that has no configuration to read gets, and never what a spawn of
    /// a real codex gets — that always goes through [`plan`].
    fn default() -> Self {
        Self {
            hooks: false,
            mcp_config: false,
            developer_instructions: false,
            resume: false,
            fork: false,
        }
    }
}

impl From<&cide_ipc::CodexInjections> for Injected {
    fn from(i: &cide_ipc::CodexInjections) -> Self {
        Self {
            hooks: i.hooks,
            mcp_config: i.mcp_config,
            developer_instructions: i.developer_instructions,
            resume: i.resume,
            fork: i.fork,
        }
    }
}

// ==========================================================================================
// What the user may not pass.
// ==========================================================================================

/// A `-c` key prefix the user may not override, because cide writes that key itself (or, for
/// `hooks`, because a second hook table would be merged with or replace cide's own).
///
/// Matched against the key of a `-c`/`--config` value, before its `=`. No `PartialEq`: the
/// `because` gate is a function pointer, and comparing those is not meaningful.
#[derive(Debug, Clone, Copy)]
pub struct RefusedKey {
    /// A dotted prefix: `hooks` refuses `hooks` and `hooks.Stop`, not `hooksmith`.
    pub prefix: &'static str,
    pub reason: &'static str,
    /// Refused only while this addition is being made, `claude_cli::RefusedArg::because`'s
    /// rule: switching cide's addition off hands the key back to the user.
    pub because: fn(&Injected) -> bool,
}

impl RefusedKey {
    fn matches(&self, key: &str) -> bool {
        key == self.prefix
            || key
                .strip_prefix(self.prefix)
                .is_some_and(|rest| rest.starts_with('.'))
    }
}

pub const REFUSED_KEYS: &[RefusedKey] = &[
    RefusedKey {
        prefix: "hooks",
        reason: "cide installs its own hooks on this command line — they are how a codex pane \
                 reports busy, awaiting and which conversation it is on. A second table here \
                 would replace or merge with cide's.",
        because: |i| i.hooks,
    },
    RefusedKey {
        prefix: "mcp_servers.cide",
        reason: "cide attaches its own MCP server under this name, with the environment it needs \
                 to find this pane. Yours would replace it and the task tools would answer \
                 nothing.",
        because: |i| i.mcp_config,
    },
    RefusedKey {
        prefix: "developer_instructions",
        reason: "cide passes the console's roster paragraph (or a run's brief) through this key, \
                 and codex keeps only one value.",
        because: |i| i.developer_instructions,
    },
];

/// One flag the user may not pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedArg {
    pub flag: &'static str,
    pub aliases: &'static [&'static str],
    pub takes_value: bool,
    pub reason: &'static str,
}

impl RefusedArg {
    fn matches(&self, token: &str) -> bool {
        let name = token.split_once('=').map_or(token, |(name, _)| name);
        name == self.flag || self.aliases.contains(&name)
    }
}

pub const REFUSED_ARGS: &[RefusedArg] = &[
    RefusedArg {
        flag: "--cd",
        aliases: &["-C"],
        takes_value: true,
        reason: "cide starts codex in the pane's own directory and passes it as the workspace \
                 root; another one here would put the conversation somewhere Resume cannot find \
                 it.",
    },
    RefusedArg {
        flag: BYPASS_HOOK_TRUST,
        aliases: &[],
        takes_value: false,
        reason: "cide passes this itself whenever it installs its hooks, and only then.",
    },
    RefusedArg {
        flag: "--remote",
        aliases: &[],
        takes_value: true,
        reason: "A TUI attached to a remote app server runs its turns — and its hooks — on \
                 another machine, where cide's sockets do not exist.",
    },
    RefusedArg {
        flag: "--no-alt-screen",
        aliases: &[],
        takes_value: false,
        reason: "Inline mode writes the transcript into the terminal's scrollback between \
                 frames, which a pane cide restores after a restart replays as garbage.",
    },
];

/// The `-c` spellings. Both take the next token as their value unless it is inline.
const CONFIG_FLAGS: &[&str] = &["-c", "--config"];

/// Environment variables the user may not set for a codex child.
pub const REFUSED_ENV: &[(&str, &str)] = &[
    (
        "CODEX_HOME",
        "cide reads this from its own environment to find the conversation behind a pane. Set \
         for the child alone, the two disagree and Resume offers nothing, or a resume that \
         fails. Export it before launching cide instead.",
    ),
    (
        "CIDE_SESSION",
        "cide sets this per pane; it is how every hook frame finds its pane.",
    ),
    (
        "CIDE_HOOK_SOCK",
        "cide sets this; it is the socket every hook frame is sent to.",
    ),
    (
        "CIDE_AGENT_SOCK",
        "cide sets this; it is the socket the task tools reach, and another cide's would \
         answer for another cide's projects.",
    ),
    (
        "CIDE_RUN",
        "cide sets this for an agent run, and only for one.",
    ),
];

/// Variables that work and mean something serious, allowed with a sentence.
pub const WARNED_ENV: &[(&str, &str)] = &[
    (
        "OPENAI_API_KEY",
        "An API key outranks a ChatGPT login in codex's credential order and moves billing to \
         API credits.",
    ),
    (
        "CODEX_API_KEY",
        "An API key outranks a ChatGPT login in codex's credential order and moves billing to \
         API credits.",
    ),
    (
        "OPENAI_BASE_URL",
        "Sends every request to another endpoint — a legitimate gateway, and also the shape of \
         a credential redirect. cide cannot tell them apart.",
    ),
];

/// The verdict on one environment variable. `claude_cli::env_verdict`'s shape, including the
/// AppImage bundle rule (ADR 0007), which is about the value and not the CLI.
pub fn env_verdict(name: &str, value: &str, appdir: Option<&str>) -> Verdict {
    let trimmed = name.trim();
    if let Some((_, reason)) = REFUSED_ENV
        .iter()
        .find(|(refused, _)| refused.eq_ignore_ascii_case(trimmed))
    {
        return Verdict::Refused(reason);
    }
    if let Some(appdir) = appdir
        && !crate::child_env::bundle_scrub_from([(trimmed.to_string(), value.to_string())], appdir)
            .is_empty()
    {
        return Verdict::Refused(
            "This value points inside cide's own AppImage, which will not exist once cide \
             exits. See docs/adr/0007.",
        );
    }
    if let Some((_, reason)) = WARNED_ENV
        .iter()
        .find(|(warned, _)| warned.eq_ignore_ascii_case(trimmed))
    {
        return Verdict::Warned(reason);
    }
    Verdict::Accepted
}

/// Everything a spawn needs and everything the screen prints, from one pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// The user's surviving argument tokens, placed after the subcommand and before cide's.
    pub args: Vec<String>,
    /// The user's surviving environment, applied after the base environment and the proxy.
    pub env: Vec<EnvChange>,
    pub arg_notes: Vec<Judged>,
    pub env_notes: Vec<Judged>,
    /// Which of cide's additions this spawn makes.
    pub inject: Injected,
}

impl Plan {
    /// Rows the child never sees.
    pub fn refusals(&self) -> impl Iterator<Item = &Judged> {
        self.arg_notes
            .iter()
            .chain(&self.env_notes)
            .filter(|judged| judged.verdict.is_refused())
    }
}

/// The user's argument tokens that survive, and a note per row.
///
/// A refused flag takes its value with it (`claude_cli::user_args`'s argument: a positional
/// left behind is a *prompt* to codex). A `-c` is judged by its value's key, so `-c
/// model="o3"` passes and `-c hooks.Stop=[]` is refused — and so is the value token after it.
pub fn user_args(cli: &CodexCli, injected: &Injected) -> (Vec<String>, Vec<Judged>) {
    let mut kept = Vec::new();
    let mut notes = Vec::with_capacity(cli.args.len());
    // What the previous token left owing: a refused flag's value, or a `-c` whose value is
    // still to be judged.
    enum Owed {
        Nothing,
        Swallow(&'static str),
        ConfigValue,
    }
    let mut owed = Owed::Nothing;
    // A `-c` whose value is judged on the next row is held back until then, so a refused
    // override removes both tokens rather than leaving a dangling `-c`.
    let mut held: Option<(usize, String)> = None;

    for (index, token) in cli.args.iter().enumerate() {
        match std::mem::replace(&mut owed, Owed::Nothing) {
            Owed::Swallow(reason) if !token.starts_with('-') => {
                notes.push(Judged {
                    index,
                    text: token.clone(),
                    verdict: Verdict::Refused(reason),
                });
                continue;
            }
            Owed::ConfigValue => {
                let (flag_index, flag) = held.take().expect("a -c is held while its value is owed");
                let verdict = config_verdict(token, injected);
                if !verdict.is_refused() {
                    kept.push(flag.clone());
                    kept.push(token.clone());
                }
                notes.push(Judged {
                    index: flag_index,
                    text: flag,
                    verdict: verdict.clone(),
                });
                notes.push(Judged {
                    index,
                    text: token.clone(),
                    verdict,
                });
                continue;
            }
            _ => {}
        }

        // `-c key=value` and `--config=key=value`, inline.
        if let Some(value) = CONFIG_FLAGS.iter().find_map(|flag| {
            token
                .strip_prefix(flag)
                .and_then(|rest| rest.strip_prefix('='))
        }) {
            let verdict = config_verdict(value, injected);
            if !verdict.is_refused() {
                kept.push(token.clone());
            }
            notes.push(Judged {
                index,
                text: token.clone(),
                verdict,
            });
            continue;
        }
        if CONFIG_FLAGS.contains(&token.as_str()) {
            held = Some((index, token.clone()));
            owed = Owed::ConfigValue;
            continue;
        }

        let verdict = match REFUSED_ARGS.iter().find(|entry| entry.matches(token)) {
            Some(entry) => {
                if entry.takes_value && !token.contains('=') {
                    owed = Owed::Swallow(entry.reason);
                }
                Verdict::Refused(entry.reason)
            }
            None => Verdict::Accepted,
        };
        if !verdict.is_refused() {
            kept.push(token.clone());
        }
        notes.push(Judged {
            index,
            text: token.clone(),
            verdict,
        });
    }

    // A trailing `-c` with nothing after it: kept out of the argv (it would make codex's
    // parser eat whatever cide writes next as its value), and said so.
    if let Some((index, flag)) = held {
        notes.push(Judged {
            index,
            text: flag,
            verdict: Verdict::Refused(
                "A `-c` needs a `key=value` after it; as the last argument it would take the \
                 first of cide's own as its value.",
            ),
        });
    }

    (kept, notes)
}

/// The verdict on one `-c` value, by its key.
fn config_verdict(value: &str, injected: &Injected) -> Verdict {
    let key = value.split_once('=').map_or(value, |(key, _)| key).trim();
    match REFUSED_KEYS
        .iter()
        .find(|entry| (entry.because)(injected) && entry.matches(key))
    {
        Some(entry) => Verdict::Refused(entry.reason),
        None => Verdict::Accepted,
    }
}

/// The user's environment rows that survive, and a note per row. `claude_cli::user_env`'s rule:
/// every change is a set, never a removal, and duplicates keep their order.
pub fn user_env(cli: &CodexCli, appdir: Option<&str>) -> (Vec<EnvChange>, Vec<Judged>) {
    let mut kept = Vec::new();
    let mut notes = Vec::with_capacity(cli.env.len());
    for (index, var) in cli.env.iter().enumerate() {
        let name = var.name.trim();
        if name.is_empty() {
            continue;
        }
        let verdict = env_verdict(name, &var.value, appdir);
        if !verdict.is_refused() {
            kept.push((name.to_string(), Some(var.value.clone())));
        }
        notes.push(Judged {
            index,
            text: name.to_string(),
            verdict,
        });
    }
    (kept, notes)
}

/// Both halves in one pass.
pub fn plan(cli: &CodexCli, appdir: Option<&str>) -> Plan {
    let inject = Injected::from(&cli.inject);
    let (args, arg_notes) = user_args(cli, &inject);
    let (env, env_notes) = user_env(cli, appdir);
    Plan {
        args,
        env,
        arg_notes,
        env_notes,
        inject,
    }
}

/// [`plan`], reading this process's `APPDIR`. What a spawn site calls.
pub fn plan_here(cli: &CodexCli) -> Plan {
    plan(cli, std::env::var("APPDIR").ok().as_deref())
}

// ==========================================================================================
// The binary.
// ==========================================================================================

/// Can this configured `codex` be run? [`crate::claude_cli::resolve`] with codex's words.
pub fn resolve(binary: &str) -> Result<PathBuf, BinaryProblem> {
    crate::claude_cli::resolve(binary)
}

/// One sentence for a [`BinaryProblem`], naming codex's own settings row.
pub fn message(problem: &BinaryProblem) -> String {
    match problem {
        BinaryProblem::Blank => "No Codex binary is configured. Settings → Harness → Codex → \
                                 Binary; “codex” is the default and lets PATH resolve it."
            .to_string(),
        BinaryProblem::NotOnPath { name } => format!(
            "“{name}” is not on this app's PATH, nor in ~/.local/bin. A cide started from a \
             desktop launcher has a different PATH from one started in a terminal, so give an \
             absolute path in Settings → Harness → Codex → Binary if it works in your shell."
        ),
        BinaryProblem::NotExecutable { path } => format!(
            "“{}” is not an executable file. Settings → Harness → Codex → Binary.",
            path.display()
        ),
    }
}

// ==========================================================================================
// cide's own additions, spelled.
// ==========================================================================================

/// `text` as a TOML basic string: the only spelling under which a `-c` value is guaranteed to
/// reach codex as the string it was. The escapes are TOML's own — the two characters that end or
/// escape a basic string, the named controls, and every other control character and DEL as
/// `\u00XX`. Everything else, non-ASCII included, is raw. Measured (M44) to decode back byte for
/// byte through `codex debug prompt-input`.
pub fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `-c <key>=<value>`, as two tokens. `key` is a bare dotted path with no `=` in it, so the
/// CLI's split on the first `=` is unambiguous however many the value carries.
pub fn push_config(args: &mut Vec<String>, key: &str, toml_value: String) {
    args.push("-c".into());
    args.push(format!("{key}={toml_value}"));
}

/// The hook table for `events`, as `-c` overrides, followed by [`BYPASS_HOOK_TRUST`].
///
/// `events` are the CLI's own spellings — `cide_claude::HookEvent::CODEX`, passed in as strings
/// because this crate sits below `cide-claude`. One entry per event with an empty matcher (every
/// tool), each running `<hook_bin> <Event>`: `cide-claude`'s `inline_settings` shape, in TOML.
pub fn hook_overrides(hook_bin: &str, events: &[&str]) -> Vec<String> {
    let mut args = Vec::with_capacity(events.len() * 2 + 1);
    for event in events {
        let command = toml_string(&format!("{hook_bin} {event}"));
        push_config(
            &mut args,
            &format!("hooks.{event}"),
            format!(r#"[{{matcher="",hooks=[{{type="command",command={command}}}]}}]"#),
        );
    }
    args.push(BYPASS_HOOK_TRUST.to_string());
    args
}

/// cide's MCP server as `-c` overrides: `cide-hook mcp`, with `env` spelled into the server's
/// own table (the module header on why codex needs it there).
pub fn mcp_overrides(hook_bin: &str, env: &[(&str, String)]) -> Vec<String> {
    let mut args = Vec::new();
    push_config(
        &mut args,
        &format!("mcp_servers.{SERVER}.command"),
        toml_string(hook_bin),
    );
    push_config(
        &mut args,
        &format!("mcp_servers.{SERVER}.args"),
        r#"["mcp"]"#.to_string(),
    );
    for (name, value) in env {
        push_config(
            &mut args,
            &format!("mcp_servers.{SERVER}.env.{name}"),
            toml_string(value),
        );
    }
    args
}

/// `-c developer_instructions=<text>`, or nothing for blank text — an empty override would blank
/// the user's own instructions for nothing.
pub fn developer_instructions(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    if !text.trim().is_empty() {
        push_config(&mut args, "developer_instructions", toml_string(text));
    }
    args
}

/// [`QUIET_START`] as argv tokens.
pub fn quiet_start() -> Vec<String> {
    QUIET_START
        .iter()
        .flat_map(|kv| ["-c".to_string(), (*kv).to_string()])
        .collect()
}

// ==========================================================================================
// Where codex keeps a conversation.
// ==========================================================================================

/// `$CODEX_HOME`, or `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(PathBuf::from(std::env::var_os("HOME")?).join(".codex")),
    }
}

/// The rollout file codex keeps for `thread`, if it exists.
///
/// `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread>.jsonl` (measured on 0.156.1;
/// the hook payloads' own `transcript_path` names the same file). Unlike Claude's, the
/// directory is not derived from the cwd — `codex resume <id>` finds a thread from anywhere
/// (M44 measured it from another directory) — so the lookup is by id alone, newest day first,
/// and bounded: the walk is three directory levels of dates and stops at the first match.
pub fn rollout_of(home: &std::path::Path, thread: &str) -> Option<PathBuf> {
    let suffix = format!("-{thread}.jsonl");
    let sorted_desc = |dir: &std::path::Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort_unstable_by(|a, b| b.cmp(a));
        entries
    };
    for year in sorted_desc(&home.join("sessions")) {
        for month in sorted_desc(&year) {
            for day in sorted_desc(&month) {
                for file in sorted_desc(&day) {
                    if file
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&suffix))
                    {
                        return Some(file);
                    }
                }
            }
        }
    }
    None
}

/// Whether codex still holds `thread`, with the resume addition switched on.
pub fn resumable(thread: &str, resume_enabled: bool) -> bool {
    resume_enabled && codex_home().is_some_and(|home| rollout_of(&home, thread).is_some())
}

/// The names `/rename` (or codex's own auto-title) gave threads, newest last: codex's
/// `$CODEX_HOME/session_index.jsonl`, one `{"id","thread_name","updated_at"}` object per line
/// (measured). Later lines win, which is how the file records a rename.
pub fn thread_names(home: &std::path::Path) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(home.join("session_index.jsonl")) else {
        return names;
    };
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let (Some(id), Some(name)) = (
            value.get("id").and_then(|v| v.as_str()),
            value.get("thread_name").and_then(|v| v.as_str()),
        ) && !name.trim().is_empty()
        {
            names.insert(id.to_string(), name.to_string());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{ClaudeEnvVar, CodexInjections};

    fn cli(args: &[&str]) -> CodexCli {
        CodexCli {
            args: args.iter().map(|s| s.to_string()).collect(),
            ..CodexCli::default()
        }
    }

    #[test]
    fn a_workspace_that_predates_codex_injects_everything() {
        // `#[serde(default)]` on the container plus a hand-written `Default`: a stored `inject`
        // object that names none of these must still turn every addition on.
        let parsed: CodexInjections = serde_json::from_str("{}").expect("parses");
        assert_eq!(parsed, CodexInjections::default());
        assert_eq!(
            Injected::from(&parsed),
            Injected {
                hooks: true,
                mcp_config: true,
                developer_instructions: true,
                resume: true,
                fork: true,
            }
        );
    }

    #[test]
    fn a_wrapper_separator_is_removed_from_user_args() {
        let mut args = vec!["agent".into(), "codex".into(), "--".into()];
        assert!(remove_wrapper_separator(&mut args));
        assert_eq!(args, ["agent", "codex"]);
        assert!(!remove_wrapper_separator(&mut args));
    }

    #[test]
    fn an_ordinary_override_passes_and_one_cide_owns_is_refused_with_its_value() {
        let (kept, notes) = user_args(
            &cli(&["-c", "model=\"o3\"", "-c", "hooks.Stop=[]", "--search"]),
            &Injected::from(&CodexInjections::default()),
        );
        assert_eq!(kept, vec!["-c", "model=\"o3\"", "--search"]);
        let refused: Vec<usize> = notes
            .iter()
            .filter(|n| n.verdict.is_refused())
            .map(|n| n.index)
            .collect();
        assert_eq!(refused, vec![2, 3], "the -c and its value go together");
    }

    #[test]
    fn switching_an_addition_off_hands_its_key_back() {
        let mut off = Injected::from(&CodexInjections::default());
        off.hooks = false;
        let (kept, _) = user_args(&cli(&["--config=hooks.Stop=[]"]), &off);
        assert_eq!(kept, vec!["--config=hooks.Stop=[]"]);
    }

    #[test]
    fn a_prefix_is_a_dotted_path_and_not_a_substring() {
        let injected = Injected::from(&CodexInjections::default());
        assert!(config_verdict("hooks.Stop=[]", &injected).is_refused());
        assert!(config_verdict("hooks=[]", &injected).is_refused());
        assert!(!config_verdict("hooksmith=1", &injected).is_refused());
        assert!(config_verdict("mcp_servers.cide.command=\"x\"", &injected).is_refused());
        assert!(!config_verdict("mcp_servers.github.command=\"x\"", &injected).is_refused());
    }

    #[test]
    fn a_refused_flag_takes_its_value_and_a_trailing_dash_c_is_dropped() {
        let (kept, notes) = user_args(
            &cli(&["-C", "/elsewhere", "--search", "-c"]),
            &Injected::from(&CodexInjections::default()),
        );
        assert_eq!(kept, vec!["--search"]);
        assert!(notes.iter().any(|n| n.index == 3 && n.verdict.is_refused()));
    }

    #[test]
    fn cide_routing_variables_and_codex_home_are_refused() {
        for name in ["CODEX_HOME", "CIDE_SESSION", "cide_agent_sock"] {
            assert!(env_verdict(name, "x", None).is_refused(), "{name}");
        }
        assert!(matches!(
            env_verdict("OPENAI_API_KEY", "sk", None),
            Verdict::Warned(_)
        ));
        let cli = CodexCli {
            env: vec![
                ClaudeEnvVar {
                    name: "RUST_LOG".into(),
                    value: "debug".into(),
                },
                ClaudeEnvVar {
                    name: "CODEX_HOME".into(),
                    value: "/tmp/x".into(),
                },
            ],
            ..CodexCli::default()
        };
        let (kept, _) = user_env(&cli, None);
        assert_eq!(
            kept,
            vec![("RUST_LOG".to_string(), Some("debug".to_string()))]
        );
    }

    #[test]
    fn the_hook_table_is_one_override_per_event_then_the_trust_bypass() {
        let args = hook_overrides("/opt/cide/cide-hook", &["SessionStart", "Stop"]);
        assert_eq!(
            args,
            vec![
                "-c",
                r#"hooks.SessionStart=[{matcher="",hooks=[{type="command",command="/opt/cide/cide-hook SessionStart"}]}]"#,
                "-c",
                r#"hooks.Stop=[{matcher="",hooks=[{type="command",command="/opt/cide/cide-hook Stop"}]}]"#,
                BYPASS_HOOK_TRUST,
            ]
        );
    }

    #[test]
    fn a_hook_path_with_a_quote_survives_as_toml() {
        let args = hook_overrides(r#"/opt/my "cide"/cide-hook"#, &["Stop"]);
        assert!(
            args[1].contains(r#"command="/opt/my \"cide\"/cide-hook Stop""#),
            "{}",
            args[1]
        );
    }

    #[test]
    fn the_mcp_server_carries_its_environment() {
        let args = mcp_overrides(
            "/opt/cide/cide-hook",
            &[("CIDE_SESSION", "abc".to_string())],
        );
        assert_eq!(
            args,
            vec![
                "-c",
                r#"mcp_servers.cide.command="/opt/cide/cide-hook""#,
                "-c",
                r#"mcp_servers.cide.args=["mcp"]"#,
                "-c",
                r#"mcp_servers.cide.env.CIDE_SESSION="abc""#,
            ]
        );
    }

    #[test]
    fn blank_instructions_are_not_an_override() {
        assert!(developer_instructions("  \n").is_empty());
        assert_eq!(
            developer_instructions("line one\nline \"two\""),
            vec!["-c", r#"developer_instructions="line one\nline \"two\"""#]
        );
    }

    #[test]
    fn toml_strings_escape_what_toml_requires() {
        assert_eq!(toml_string("a\"b\\c\n\t\u{1}"), r#""a\"b\\c\n\t\u0001""#);
        assert_eq!(toml_string("true"), r#""true""#);
    }

    #[test]
    fn a_rollout_is_found_by_thread_id_from_any_day() {
        let home = std::env::temp_dir().join(format!("cide-codex-home-{}", std::process::id()));
        let day = home.join("sessions/2026/09/24");
        std::fs::create_dir_all(&day).expect("mkdir");
        let thread = "01a0d24a-1dfe-72d2-8b25-185cd0f80ec7";
        let file = day.join(format!("rollout-2026-09-24T10-21-07-{thread}.jsonl"));
        std::fs::write(&file, "{}\n").expect("write");
        assert_eq!(rollout_of(&home, thread), Some(file));
        assert_eq!(
            rollout_of(&home, "01a0d24a-0000-0000-0000-000000000000"),
            None
        );
        std::fs::write(
            home.join("session_index.jsonl"),
            format!(
                "{{\"id\":\"{thread}\",\"thread_name\":\"First\"}}\n{{\"id\":\"{thread}\",\"thread_name\":\"Renamed\"}}\n"
            ),
        )
        .expect("write index");
        assert_eq!(
            thread_names(&home).get(thread).map(String::as_str),
            Some("Renamed")
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
