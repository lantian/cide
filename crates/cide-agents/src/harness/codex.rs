//! Running a role on OpenAI's Codex CLI (`codex`). (M44)
//!
//! Measured against **codex-cli 0.153.2**, the standalone-installer build at `~/.local/bin`,
//! not remembered. Where a paragraph below says *measured*, a command was run — `--help`
//! probes for which flags each subcommand accepts, `codex mcp list`/`mcp get` for what an
//! override reaches, `codex debug prompt-input` for what the model is actually shown — none of
//! which spends a token. Where it says *unmeasured*, the fact needs a real turn and is pinned
//! by an `#[ignore]`d test in `tests/real_codex.rs` rather than by a probe.
//!
//! # Hosted like `opencode`, not like `qwen`
//!
//! Qwen Code is Claude-shaped enough to host the way `claude.rs` does: an interactive TUI in
//! the pane, with the run's state read off a JSON event file the CLI writes beside it. Codex
//! has no such dual output — `--json` exists on `codex exec` only — and its hooks, though
//! Claude-shaped, live in `$CODEX_HOME/config.toml`, `.codex/config.toml` or `.codex/hooks.json`
//! behind a persisted trust hash. cide writes into neither a user's home configuration nor their
//! project, and there is no `--settings` twin to hand a document over on the command line. So a
//! run is **`codex exec --json`**: one process per turn, one JSON event per line on stdout, the
//! process gone when the turn is. That is `opencode.rs`'s shape exactly, and every piece of
//! machinery it needs already exists — [`SessionBinding::Harness`] for an identity that arrives
//! on stdout, [`Delivery::Respawn`] for a follow-up that must be a new child, the rendering
//! upstream of the mirror, the fixed-width mirror, the `#handle` ring. Codex improves on
//! opencode in one respect: `codex exec resume <thread_id> <prompt>` is a real follow-up into
//! the same conversation, and `codex resume <thread_id>` is the real TUI a person re-opens a
//! finished run in ([`Harness::continue_spec`]).
//!
//! `-c hooks.…` overrides plus `--dangerously-bypass-hook-trust` may one day make the
//! interactive road viable; it is unmeasured and named here as the upgrade path, the way
//! opencode's `serve` plus `attach` is named in that file.
//!
//! # One argv for both children
//!
//! `codex exec [OPTIONS] [PROMPT]` and `codex exec [OPTIONS] resume [SESSION_ID] [PROMPT]`. The
//! `resume` subcommand **rejects** `-s`, `-C`, `--approve-for-me` and `--add-dir` — measured —
//! and every one of them is **accepted at the exec root ahead of `resume`**, also measured, so
//! [`assemble`] writes the shared options once and puts either the prompt or `resume <id>
//! <prompt>` after them. `--json` is declared on both `exec` and `resume`; whether the root one
//! propagates into the subcommand is unmeasured, so it is written where each subcommand
//! certainly parses it. Numbered steps at the call sites; the order is load-bearing only at
//! the end, where the prompt is positional and a token after it is another word of the prompt.
//!
//! # The brief is a config override, and it must be a TOML string
//!
//! There is no `--append-system-prompt`. What there is: `-c developer_instructions="…"`, and
//! measured with `codex debug prompt-input` it lands as the **first `developer` message** the
//! model sees, ahead of codex's own. A `-c` value is *parsed as TOML* and only falls back to a
//! literal when it fails to parse, which cuts both ways: a brief that happens to parse as
//! another type — `true`, `42`, `[x]`, a date — is a hard error (`invalid type: integer,
//! expected a string`, measured), and a multi-line literal is not valid TOML at all. So
//! [`toml_string`] encodes every string cide passes through `-c` as a basic string with the
//! escapes TOML defines, measured to decode back byte for byte. One documented cost: the
//! override *replaces* any `developer_instructions` the user keeps in their own config, for this
//! child only.
//!
//! The paragraphs and their order are every harness's: the role's own words, then — only when
//! the tracker is attached — [`super::TRACKER_PREAMBLE`], the OpenSpec paragraph, the ad-hoc
//! one. `harness.rs`'s parity tests hold [`developer_brief`] to claude's fold.
//!
//! # The tracker, and the one place codex differs from every other harness
//!
//! `-c mcp_servers.cide.command=… -c mcp_servers.cide.args=["mcp"]` reaches codex's MCP table
//! (measured: `codex mcp list` prints the server), so cide attaches `cide-hook mcp` per
//! invocation and writes nothing anywhere. But codex starts a stdio MCP server with a
//! **whitelisted environment** — `DEFAULT_ENV_VARS` plus the server's own `env` map in
//! `codex-rs/rmcp-client/src/utils.rs` — and not with its own. `claude` and `qwen` and
//! `opencode` all let the bridge inherit `CIDE_RUN` and `CIDE_AGENT_SOCK` from the child; here
//! they would be stripped, the bridge would start, find no run to scope itself to, and every
//! tracker tool would answer nothing with no error anywhere. So both variables are spelled
//! into `mcp_servers.cide.env.*` as well (measured accepted, `codex mcp get cide` prints
//! them), and set on the child for the same reason every harness sets them.
//!
//! Tools reach the model as `mcp__<server>__<tool>` — `MCP_TOOL_NAME_PREFIX`, the `__`
//! delimiter, the server, the tool, in `codex-rs/codex-mcp/src/tools.rs` — which is claude's
//! spelling, so [`CodexHarness::tool_name`] answers `mcp__cide__cide_task_get`. A feature flag
//! `non_prefixed_mcp_tool_names` (under development, off) is the thing that would change it.
//!
//! # Permissions are sandboxes here
//!
//! `codex exec` has no `-a/--ask-for-approval` and never asks: an action the sandbox refuses
//! is returned to the model as a failure. So the definition's `permission-mode` — claude's
//! vocabulary, [`crate::defs::PERMISSION_MODES`] — maps onto sandbox flags in
//! [`permission_policy`], and there is no [`RunState::AwaitingPermission`] on this harness.
//! The one rung that matters most is the project default. Under `workspace-write`, codex
//! re-binds `.git`, `.codex` and `.agents` **read-only even inside the writable root**, and a
//! cide worktree's `.git` is a file pointing into the parent's `.git/worktrees/<name>` outside
//! the root in any case — so a sandboxed run cannot `git commit`, which the tracker paragraph
//! asks every task run to do. `RunPlan::unattended` therefore means
//! `--dangerously-bypass-approvals-and-sandbox` for both `auto` and `bypassPermissions`, and the argument is `opencode.rs`'s `--auto`
//! paragraph word for word: a refusal that ends the work is not a safety property in a headless
//! child, and the containment for an unattended run is what it always actually was, the
//! worktree. A role that names a sandboxed mode gets it, and its task runs will not satisfy the
//! commit rule; that is the author's choice and is recorded rather than papered over with
//! `--add-dir`, which cannot unprotect `.git`.
//!
//! `tools:` is claude's allow-list vocabulary and has no codex counterpart. It is **dropped**,
//! as `opencode.rs` drops it and for its reason: `mcp_servers.cide.enabled_tools` could carry
//! the `mcp__cide__*` half of such a list and nothing could carry the rest, and a restriction
//! applied by halves is the failure that looks like success. Refusing instead would make every
//! claude-vocabulary role undispatchable here over a key that cannot mean anything on this CLI.
//!
//! # What ends a turn
//!
//! The exit. `turn.completed` is the last event and the process leaves within milliseconds of
//! it, so no line maps to [`RunState::Idle`] — opencode's run-06202dd6 rule, argued at length
//! in that file's `observe`: being late to call a turn over costs nothing, being early cost a
//! working agent its life. Every `thread.started`/`turn.started`/`item.*` line says `Running`,
//! because for this harness they are the only evidence the child is doing anything.
//!
//! # The environment
//!
//! [`cide_core::child_env::terminal_child_env`] with an **empty** `extra`, opencode's rule; the
//! proxy; `CIDE_RUN` and `CIDE_AGENT_SOCK`. Nothing removed. **`CODEX_API_KEY` is neither set
//! nor removed**: it outranks the ChatGPT login in codex's auth ladder and moves billing to API
//! credits, which is `child_env`'s `ANTHROPIC_API_KEY` rule with the variable renamed — the
//! child inherits whatever the user has and cide adds nothing.
//!
//! # Finding the binary
//!
//! `codex` is a static binary the standalone installer symlinks into `~/.local/bin`, or an npm
//! package (`@openai/codex`) under a Node version manager. Both directories are on a child's
//! `PATH` through `toolchain::discovered_dirs`, and neither needs a `node` beside it.

use std::path::Path;

use cide_ipc::{HarnessSession, RunState};
use cide_pty::{Geometry as PtyGeometry, Rendered, SpawnSpec};
use serde::Deserialize;
use serde_json::Value;

use super::render::{
    BOLD, CYAN, DIM, RED, RESET, RUN_COLS, RUN_ROWS, TITLE_BUDGET, absorbs, clip, compact_input,
    compose, measured_ms, one_line_of, prose, tail, thought_line, thousands,
};
use super::{
    ADHOC_PREAMBLE, ContinueSpec, Delivery, Harness, HarnessError, HarnessSpawn, Observation,
    RenderState, RunPlan, SERVER, SessionBinding, spec_preamble, tracker_preamble,
};
use crate::config::Unattended;

/// The Codex CLI as a harness. A unit struct: it holds nothing, and must not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodexHarness;

impl Harness for CodexHarness {
    fn kind(&self) -> cide_ipc::Harness {
        cide_ipc::Harness::Codex
    }

    /// `mcp__<server>__<tool>` — claude's namespacing; see the module header for where codex
    /// builds it and the flag that would change it.
    fn tool_name(&self, tool: &str) -> String {
        format!("mcp__{SERVER}__{tool}")
    }

    fn spawn_spec(&self, plan: &RunPlan<'_>) -> Result<HarnessSpawn, HarnessError> {
        assemble(plan, None)
    }

    /// The continuing child: the same options, `resume <thread_id>` where the fresh child had
    /// nothing, the follow-up as the prompt. `session` is the id [`capture`] read off the first
    /// child's `thread.started`, never cide's [`cide_ipc::SessionId`].
    fn respawn_spec(
        &self,
        plan: &RunPlan<'_>,
        session: &str,
    ) -> Result<HarnessSpawn, HarnessError> {
        assemble(plan, Some(session))
    }

    /// Always [`Delivery::Respawn`]: `exec` is one turn per process, and the child that
    /// answered the last one has already exited.
    fn deliver(&self, _text: &str) -> Delivery {
        Delivery::Respawn
    }

    /// The codex **TUI** on the conversation: `codex resume <uuid> --include-non-interactive`,
    /// from the run's directory.
    ///
    /// Not `exec`. A run is `exec --json`, rendered by [`render_event`] into the digest a pane
    /// shows while a turn is in flight, and that digest is what the person who opened a
    /// finished run did *not* want to read. `--include-non-interactive` because the picker and
    /// `--last` exclude `exec` sessions by default (measured in `--help`); whether an id alone
    /// would have found one is unmeasured, and the flag is harmless either way. The id must be
    /// a uuid — codex mints uuidv7s, which parse as uuids — or it is not a conversation.
    fn continue_spec(&self, conversation: &HarnessSession) -> Result<ContinueSpec, HarnessError> {
        if conversation.harness != cide_ipc::Harness::Codex {
            return Err(HarnessError::WrongHarness {
                plan: conversation.harness,
                harness: cide_ipc::Harness::Codex,
            });
        }
        let id = conversation.id.trim();
        if id.parse::<cide_ipc::SessionId>().is_err() {
            return Err(HarnessError::NotAConversation {
                harness: cide_ipc::Harness::Codex,
                id: conversation.id.clone(),
            });
        }
        Ok(ContinueSpec {
            program: crate::defs::harness_binary(cide_ipc::Harness::Codex).to_string(),
            args: vec![
                "resume".into(),
                id.to_string(),
                "--include-non-interactive".into(),
            ],
            resume: None,
        })
    }

    /// The whole state machine for this harness, because the output stream is the whole channel.
    ///
    /// | observation | answer |
    /// | --- | --- |
    /// | `Exit(code)` | `Finished { code }` from any state — the one ground truth about the child |
    /// | `Line` — `thread.started`, `turn.started`, `item.started`, `item.updated`, `item.completed` | `Running` |
    /// | `Line` — `turn.completed`, `turn.failed`, `thread.failed`, `error`, anything else, not JSON | `None` |
    /// | `Hook` | `None` — nothing installs a hook into this CLI |
    ///
    /// No line answers `Idle` (the module header says why), and no line moves a `Paused`,
    /// `Finished` or `Failed` run — [`absorbs`], opencode's two refusals.
    fn observe(&self, current: RunState, ob: Observation<'_>) -> Option<RunState> {
        match ob {
            Observation::Exit(code) => {
                let next = RunState::Finished { code };
                (next != current).then_some(next)
            }
            Observation::Hook(_) => None,
            Observation::Line(line) => {
                if absorbs(&current) {
                    return None;
                }
                let event = Event::parse(line)?;
                let next = match event.kind.as_str() {
                    "thread.started" | "turn.started" | "item.started" | "item.updated"
                    | "item.completed" => RunState::Running,
                    _ => return None,
                };
                (next != current).then_some(next)
            }
        }
    }

    fn usage(&self, line: &str) -> Option<cide_ipc::TokenUsage> {
        usage(line)
    }

    /// `codex debug models`, run for real.
    ///
    /// A probe and not a table, for `opencode.rs`'s reason: the catalog is a property of this
    /// machine — which models the account can reach, under what names — and it refreshes
    /// under the CLI's own etag. Measured at 0.8 s with a warm cache. The `visibility` filter
    /// is the catalog's own: a `hide` entry is one the CLI's picker would not offer either.
    fn models(
        &self,
        cwd: Option<&Path>,
        _llm: &cide_ipc::LlmSettings,
    ) -> Result<Vec<String>, String> {
        let binary =
            cide_core::toolchain::which(crate::defs::harness_binary(cide_ipc::Harness::Codex))
                // The *same* search `defs::installed` uses, and the same sentence when it misses.
                .ok_or_else(|| {
                    crate::defs::installed(cide_ipc::Harness::Codex)
                        .unwrap_or_else(|| "`codex` could not be found".to_string())
                })?;

        let mut command = std::process::Command::new(&binary);
        command.args(["debug", "models"]);
        command.env("NO_COLOR", "1");
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        // `run_filter_with`, the workspace's spawn chokepoint, so `prepare_command` and `arm`
        // are applied. The binary's own directory rides on the child's PATH for the
        // version-manager-shim case, as opencode's probe does.
        let bin_dir: Vec<std::path::PathBuf> = binary
            .parent()
            .map(|d| vec![d.to_path_buf()])
            .unwrap_or_default();
        let filtered =
            cide_core::child_env::run_filter_with(command, None, MODELS_DEADLINE, &bin_dir)
                .map_err(|error| describe(&error))?;

        if !filtered.ok {
            let detail = filtered
                .stderr
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty());
            return Err(match detail {
                Some(line) => format!("`codex debug models` failed: {line}"),
                None => "`codex debug models` failed and said nothing".to_string(),
            });
        }

        model_slugs(&String::from_utf8_lossy(&filtered.stdout))
    }
}

/// What a `turn.completed` line says the turn spent. (M80)
///
/// `usage: {input_tokens, cached_input_tokens, output_tokens}` — and the one thing to know
/// about it is that **`cached_input_tokens` is a part of `input_tokens`, not a figure beside
/// it**: the measured line carries `5696` input of which `4096` were cached, which is a prompt
/// of 5,696 tokens and not one of 9,792. opencode counts the other way round (see its `usage`),
/// so the subtraction happens here, once, and both harnesses hand
/// [`cide_ipc::TokenUsage`] the same shape. Carried raw, an identical conversation would read
/// 72% larger under codex and nothing on screen would say why.
///
/// A saturating subtraction, not a bare one: the figures come off another program's wire, and a
/// release that ever reported more cached than input would otherwise panic the coalescer
/// thread — the thread every byte of every session flows through.
///
/// codex reports no reasoning count and no cache *writes* at all, so both are zero: a number
/// derived from the output would be cide inventing a measurement the CLI never took.
pub fn usage(line: &str) -> Option<cide_ipc::TokenUsage> {
    let line = line.trim();
    // Cheap first — `opencode::failover`'s rule, on the coalescer thread for its reason.
    if !line.starts_with('{') || !line.contains("\"turn.completed\"") {
        return None;
    }
    let event: Value = serde_json::from_str(line).ok()?;
    if event.get("type").and_then(Value::as_str) != Some("turn.completed") {
        return None;
    }
    let usage = event.get("usage")?;
    let at = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = at("cached_input_tokens");
    Some(cide_ipc::TokenUsage {
        input: at("input_tokens").saturating_sub(cached),
        output: at("output_tokens"),
        reasoning: 0,
        cache_read: cached,
        cache_write: 0,
    })
}

/// One sentence for a [`cide_core::child_env::FilterError`], read in a settings dialog.
fn describe(error: &cide_core::child_env::FilterError) -> String {
    use cide_core::child_env::FilterError;
    match error {
        FilterError::Spawn(io) => format!("`codex debug models` could not be started: {io}"),
        FilterError::Timeout => "`codex debug models` did not answer in time and was stopped. \
                                 The catalog may be refreshing from the network — the model can \
                                 still be typed in below."
            .to_string(),
        FilterError::Unreadable => {
            "`codex debug models` ran and its output could not be read".to_string()
        }
        FilterError::Wait(io) => format!("`codex debug models` ran and could not be reaped: {io}"),
    }
}

/// How long `codex debug models` is given before it is killed. Measured well under a second
/// with a warm cache, and an order of magnitude plus a network round trip on top, because the
/// catalog refreshes on a stale etag — the cost of the ceiling is a hint under a typable box.
const MODELS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// The shape of `codex debug models`'s document, in the two fields the probe reads.
#[derive(Debug, Deserialize)]
struct Catalog {
    #[serde(default)]
    models: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct CatalogEntry {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    visibility: Option<String>,
}

/// The listed slugs in the catalog's order, without repeats.
///
/// Skips to the first `{` first: a warning the CLI prints above its document — an update
/// notice, a deprecation — would otherwise be a parse failure, and the whole point of a probe
/// is that "today it prints nothing else" is not a guarantee.
fn model_slugs(stdout: &str) -> Result<Vec<String>, String> {
    let start = stdout.find('{').ok_or_else(|| {
        "`codex debug models` printed nothing that looks like its catalog".to_string()
    })?;
    let catalog: Catalog = serde_json::from_str(&stdout[start..]).map_err(|_| {
        "`codex debug models` printed something that is not its JSON catalog".to_string()
    })?;
    let mut seen = std::collections::HashSet::new();
    Ok(catalog
        .models
        .into_iter()
        .filter(|entry| entry.visibility.as_deref() == Some("list"))
        .map(|entry| entry.slug)
        .filter(|slug| !slug.is_empty() && !slug.chars().any(char::is_whitespace))
        .filter(|slug| seen.insert(slug.clone()))
        .collect())
}

// ==========================================================================================
// The stream.
// ==========================================================================================

/// One event line, in the two fields anything here reads by type.
///
/// Measured shape: `{"type":"thread.started","thread_id":"…"}` first, then `turn.started`,
/// `item.started`/`item.updated`/`item.completed` carrying an `item`, `turn.completed` with
/// `usage`, and `turn.failed`/`thread.failed`/`error` on the way out. The names are exhaustive
/// from the binary's own strings.
#[derive(Debug, Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    thread_id: Option<String>,
}

impl Event {
    /// One line, or `None` when it is not one of ours.
    ///
    /// The child runs in a PTY, so its stderr — a startup banner, a warning about the sandbox,
    /// possibly styled — lands in the same stream as its stdout. Those fall through rather than
    /// raising anything, and [`render_event`] keeps them verbatim. `trim` for the line
    /// discipline's `\r`.
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if !line.starts_with('{') {
            return None;
        }
        serde_json::from_str(line).ok()
    }
}

/// The thread id, for [`SessionBinding::Harness`]. Answers from `thread.started` and from
/// nothing else — explicitly, so a later event that happens to carry a `thread_id` (a
/// sub-agent's, one day) can never be the first thing captured.
fn capture(line: &str) -> Option<String> {
    let event = Event::parse(line)?;
    if event.kind != "thread.started" {
        return None;
    }
    event.thread_id.filter(|id| !id.is_empty())
}

/// Whether a person may later ask for this line's whole event — [`SessionBinding::Harness`]'s
/// `keep`. Every completed item, and the two failure shapes.
///
/// Reasoning was the one exclusion until M62, when the rendering stopped drawing any of it: the
/// row is now a duration and a handle, so this ring is the only copy of what was thought. See
/// `opencode::keep_event` for the pairing this has to hold with the row's handle.
///
/// `item.started`/`item.updated` stay out, and must: they arrive per streamed chunk, and keeping
/// them would spend a ring slot each on a line the renderer deliberately draws nothing for.
pub fn keep_event(line: &str) -> bool {
    let Some(event) = object_of(line) else {
        return false;
    };
    matches!(
        event.get("type").and_then(Value::as_str),
        Some("item.completed" | "error" | "turn.failed")
    )
}

/// The line as a JSON object, or `None` for anything else — [`Event::parse`]'s rule, for the
/// readers that want the whole document.
fn object_of(line: &str) -> Option<serde_json::Map<String, Value>> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Value>(line).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

// ==========================================================================================
// Rendering the event stream for a person.
// ==========================================================================================

/// One event line, as a person should read it — [`SessionBinding::Harness`]'s `render`.
///
/// The same reading `opencode.rs`'s `render_event` gives its stream, over codex's shapes:
///
/// * a completed item is **one line** — `● shell  cargo test  #7`, `● cide_task_get  t-14  #8`,
///   `● edit  ~src/a.rs +src/b.rs` — with the whole event behind the handle for the card, and a
///   failed one red with the CLI's own reason on the next line: a command's last output line,
///   an MCP call's error, a sandbox's *declined*;
/// * the live marker sits under the last line from `turn.started` to `turn.completed`;
///   `item.started` and `item.updated` draw nothing **and leave the marker alone** — they are
///   the streamed progress of one item, and redrawing the marker on every chunk would scroll
///   a pane doing nothing;
/// * the model's own words pass whole, a block of reasoning collapses to one dim row naming how
///   long it took (M62 — the whole block is behind the handle), `turn.completed` is one dim
///   token count;
/// * an event or item type this build has never heard of becomes a dim one-word marker, and a
///   line that is not JSON is kept verbatim — the CLI's own prose, which hiding would hide.
///
/// The `#handle` tail is spelled as opencode spells it — by the same function, since M62 — at
/// the end of the first line, because `ui/src/terminal/runLinks.ts` reads it off the end of a
/// line beginning `●`, `✗` or `∴`.
pub fn render_event(state: &mut RenderState, line: &str, handle: Option<u64>) -> Rendered {
    let Some(event) = object_of(line) else {
        return prose(state, line);
    };
    let Some(kind) = event.get("type").and_then(Value::as_str) else {
        return prose(state, line);
    };

    let text = match kind {
        // The id is captured, not shown.
        "thread.started" => None,
        "turn.started" => {
            state.in_step = true;
            None
        }
        "turn.completed" => {
            state.in_step = false;
            let usage = event.get("usage").unwrap_or(&Value::Null);
            let total = usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            Some(format!("{DIM}· done · {} tok{RESET}", thousands(total)))
        }
        "turn.failed" | "thread.failed" => {
            state.in_step = false;
            let what = if kind == "turn.failed" {
                "turn"
            } else {
                "thread"
            };
            Some(format!(
                "{RED}✗ {what} failed: {}{RESET}",
                one_line_of(&message_of(event.get("error")))
            ))
        }
        // `is_critical` absent reads as critical: the CLI's own reconnect notices say `false`
        // in as many words, and a shape that says nothing is not one to play down.
        "error" => {
            let message = one_line_of(&message_of(event.get("message")));
            let critical = event
                .get("is_critical")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Some(if critical {
                format!("{RED}✗ {message}{RESET}")
            } else {
                format!("{DIM}· {message}{RESET}")
            })
        }
        // Progress of an item still running. Nothing drawn, and — deliberately not through
        // `compose` — the marker left where it is.
        "item.started" | "item.updated" => return Rendered::Drop,
        // The thinking duration is resolved *here*, before `compose` moves the clock on, and
        // handed down: `render_item` stays a function of the item alone, which is what its
        // table of arms is readable as.
        "item.completed" => {
            let item = event.get("item").unwrap_or(&Value::Null);
            match render_item(item, handle, measured_ms(state)) {
                Some(text) => Some(text),
                None => return Rendered::Drop,
            }
        }
        other => Some(format!("{DIM}· {other}{RESET}")),
    };
    compose(state, text)
}

/// One completed item on one line (two for a failure), or `None` for one that draws nothing.
///
/// `thought_ms` is the gap since the previous rendered line, read by the reasoning arm alone —
/// codex records no clock on any item, so it is the only duration there is.
fn render_item(item: &Value, handle: Option<u64>, thought_ms: Option<u64>) -> Option<String> {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("item");
    let tail = tail(None, handle);
    Some(match kind {
        "agent_message" => {
            let text = text_of(item, "text");
            let text = text.trim_end();
            if text.is_empty() {
                return None;
            }
            text.to_string()
        }
        // One row, no prose — `opencode.rs`'s reasoning arm, over this shape. The duration is
        // measured rather than stated because **no** codex item carries a clock (see
        // `render_command`), so the only interval available is the silence this item ended.
        "reasoning" => {
            if one_line_of(text_of(item, "text")).is_empty() {
                return None;
            }
            thought_line(thought_ms, handle)
        }
        "command_execution" => render_command(item, &tail),
        "file_change" => render_file_change(item, &tail),
        "mcp_tool_call" => render_mcp_call(item, &tail),
        "web_search" => format!(
            "{CYAN}{BOLD}● web_search{RESET}  {}{tail}",
            clip(&one_line_of(text_of(item, "query")), TITLE_BUDGET)
        ),
        // The source calls it `todo_list`; the documentation calls it `plan_update`. Both.
        "todo_list" | "plan_update" => render_plan(item),
        "error" => format!(
            "{RED}✗ {}{RESET}",
            one_line_of(&message_of(item.get("message")))
        ),
        other => format!("{DIM}· {other}{RESET}"),
    })
}

/// `● shell  <command>`, with the exit code when it is not zero; a failure or a sandbox's
/// refusal is red, with the reason on the next line. Codex records no timing on an item, so
/// the tail is the handle alone.
fn render_command(item: &Value, tail: &str) -> String {
    let command = clip(&one_line_of(text_of(item, "command")), TITLE_BUDGET);
    let status = text_of(item, "status");
    let exit = item.get("exit_code").and_then(Value::as_i64);
    match status {
        "failed" => {
            let exit = exit
                .map(|code| format!("  {RED}exit {code}{RESET}"))
                .unwrap_or_default();
            // The last thing the command said is the one line a reader wants without opening
            // the card — a compiler's `error:`, a shell's `not found`.
            let why = text_of(item, "aggregated_output")
                .lines()
                .map(str::trim)
                .rfind(|line| !line.is_empty())
                .unwrap_or("failed");
            format!(
                "{RED}✗ shell{RESET}  {command}{exit}{tail}\n{RED}  {}{RESET}",
                clip(&one_line_of(why), TITLE_BUDGET)
            )
        }
        "declined" => format!(
            "{RED}✗ shell{RESET}  {command}{tail}\n{RED}  declined by the sandbox or the approval policy{RESET}"
        ),
        _ => {
            let exit = exit
                .filter(|code| *code != 0)
                .map(|code| format!("  {RED}exit {code}{RESET}"))
                .unwrap_or_default();
            format!("{CYAN}{BOLD}● shell{RESET}  {command}{exit}{tail}")
        }
    }
}

/// `● edit  ~changed.rs +added.rs -deleted.rs`.
fn render_file_change(item: &Value, tail: &str) -> String {
    let changes: Vec<String> = item
        .get("changes")
        .and_then(Value::as_array)
        .map(|changes| {
            changes
                .iter()
                .map(|change| {
                    let glyph = match text_of(change, "kind") {
                        "add" => '+',
                        "delete" => '-',
                        _ => '~',
                    };
                    format!("{glyph}{}", text_of(change, "path"))
                })
                .collect()
        })
        .unwrap_or_default();
    let title = clip(&one_line_of(&changes.join(" ")), TITLE_BUDGET);
    if text_of(item, "status") == "failed" {
        format!("{RED}✗ edit{RESET}  {title}{tail}")
    } else {
        format!("{CYAN}{BOLD}● edit{RESET}  {title}{tail}")
    }
}

/// `● cide_task_get  t-14` for cide's own server — the tool's name is what a person knows —
/// and `● server:tool` for any other, spelled without whitespace so the handle regex still
/// reads the first word as the tool.
fn render_mcp_call(item: &Value, tail: &str) -> String {
    let server = text_of(item, "server");
    let tool = text_of(item, "tool");
    let tool = if tool.is_empty() { "tool" } else { tool };
    let name = if server.is_empty() || server == SERVER {
        tool.to_string()
    } else {
        format!("{server}:{tool}")
    };
    let title = clip(
        &one_line_of(&compact_input(
            item.get("arguments").unwrap_or(&Value::Null),
        )),
        TITLE_BUDGET,
    );
    if text_of(item, "status") == "failed" {
        format!(
            "{RED}✗ {name}{RESET}  {title}{tail}\n{RED}  {}{RESET}",
            one_line_of(&message_of(item.get("error")))
        )
    } else {
        format!("{CYAN}{BOLD}● {name}{RESET}  {title}{tail}")
    }
}

/// `· plan 1/3: the next open step`, dim — a plan is texture, not record.
fn render_plan(item: &Value) -> String {
    let items = item
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let done = items
        .iter()
        .filter(|entry| entry.get("completed").and_then(Value::as_bool) == Some(true))
        .count();
    let next = items
        .iter()
        .find(|entry| entry.get("completed").and_then(Value::as_bool) != Some(true))
        .map(|entry| {
            format!(
                ": {}",
                clip(&one_line_of(text_of(entry, "text")), TITLE_BUDGET)
            )
        })
        .unwrap_or_default();
    format!("{DIM}· plan {done}/{}{next}{RESET}", items.len())
}

/// A string field, or the empty string.
fn text_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The sentence inside an error value, whichever shape it took: a string, an object with a
/// `message`, or anything else printed as it is.
fn message_of(error: Option<&Value>) -> String {
    match error {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(map)) => match map.get("message") {
            Some(Value::String(text)) => text.clone(),
            _ => Value::Object(map.clone()).to_string(),
        },
        Some(Value::Null) | None => "failed".to_string(),
        Some(other) => other.to_string(),
    }
}

// ==========================================================================================
// The child.
// ==========================================================================================

/// Build the child, fresh (`resume: None`) or continuing the thread the first child named.
/// One function so the two differ in exactly the identity tokens — `claude.rs::assemble`'s
/// shape, `opencode.rs::child`'s argument.
fn assemble(plan: &RunPlan<'_>, resume: Option<&str>) -> Result<HarnessSpawn, HarnessError> {
    // `plan.harness`, the *resolved* one, not the definition's: a role a local override moved onto
    // this CLI must not be refused by it. See `RunPlan::harness`.
    if plan.harness != cide_ipc::Harness::Codex {
        return Err(HarnessError::WrongHarness {
            plan: plan.harness,
            harness: cide_ipc::Harness::Codex,
        });
    }
    // Refused before anything is built: `exec` with an empty prompt reads its instructions
    // from stdin, which is a PTY nobody types into — a child that starts, waits and holds a
    // concurrency slot and a worktree.
    if plan.prompt.trim().is_empty() {
        return Err(HarnessError::NoPrompt);
    }
    // Refused before anything is built too, so a role naming a mode this harness cannot honour
    // is refused at dispatch with the mode in the sentence, not started under some other one.
    let policy = permission_policy(plan.agent.permission_mode.as_deref(), plan.unattended)?;

    // A bare name, resolved by the OS at this spawn: codex updates itself underneath a running
    // app (`codex update`), and `defs::installed` probed for the same name.
    let program = crate::defs::harness_binary(cide_ipc::Harness::Codex);

    let mut args: Vec<String> = vec!["exec".into()];

    // ---- 1. where the work is ----
    //
    // Both `-C` and the spawn's cwd, to the same directory — opencode's `--dir` rule: the cwd is
    // what the child and everything it starts inherit, `-C` is what codex resolves the
    // workspace root, the sandbox's writable root and `AGENTS.md` from. A task-less run lands
    // in the project root (M40), which need not be a repository; `exec` refuses a non-git cwd
    // without the check switched off.
    args.push("-C".into());
    args.push(plan.cwd.to_string_lossy().to_string());
    args.push("--skip-git-repo-check".into());

    // ---- 2. the sandbox, from the definition or the project default ----
    args.extend(policy.iter().map(|token| token.to_string()));

    // ---- 3. what the definition asked for ----
    if let Some(model) = &plan.agent.def.model {
        args.push("-m".into());
        args.push(model.clone());
    }
    // Verbatim and unvalidated, for `LoadedAgent::effort`'s stated reason: the set is per
    // model and per release, and a bad value is the CLI's refusal to make, loudly.
    if let Some(effort) = &plan.agent.effort {
        push_config(&mut args, "model_reasoning_effort", toml_string(effort));
    }

    // ---- 4. the brief, one override ----
    //
    // Skipped entirely when there is nothing to say, so a role with an empty prompt and no
    // bridge does not blank the user's own `developer_instructions` for nothing.
    let brief = developer_brief(plan);
    if !brief.trim().is_empty() {
        push_config(&mut args, "developer_instructions", toml_string(&brief));
    }

    // ---- 5. the tracker, with its own environment ----
    //
    // Gated on `hook_bin` like every harness, and all four lines or none: a server named
    // without its environment starts and scopes itself to nothing (the module header).
    if let Some(hook) = &plan.hook_bin {
        push_config(
            &mut args,
            &format!("mcp_servers.{SERVER}.command"),
            toml_string(&hook.to_string_lossy()),
        );
        push_config(
            &mut args,
            &format!("mcp_servers.{SERVER}.args"),
            r#"["mcp"]"#.to_string(),
        );
        push_config(
            &mut args,
            &format!("mcp_servers.{SERVER}.env.CIDE_RUN"),
            toml_string(&plan.run.to_string()),
        );
        if let Some(sock) = &plan.agent_sock {
            push_config(
                &mut args,
                &format!("mcp_servers.{SERVER}.env.CIDE_AGENT_SOCK"),
                toml_string(&sock.to_string_lossy()),
            );
        }
    }

    // The banner and any warning are the only human output left once `--json` is on, and the
    // pane keeps them verbatim; plain is easier to read back from the ring than styled.
    args.push("--color".into());
    args.push("never".into());

    // ---- 6. the channel, and the identity ----
    //
    // `--json` where each subcommand certainly declares it — the module header on why the root
    // one is not relied on to reach `resume`.
    match resume {
        None => args.push("--json".into()),
        Some(thread) => {
            args.push("resume".into());
            args.push("--json".into());
            args.push(thread.to_string());
        }
    }

    // ---- 7. last, and nothing may be added after it ----
    args.push(plan.prompt.clone());

    // Not `plan.geometry`, and never resized afterwards — see `render::RUN_COLS`. The child
    // prints lines and never reads the size, and the pane's later measurement is exactly the
    // resize that would damage the mirror.
    let mut spec = SpawnSpec::new(program, plan.cwd.clone())
        .geometry(PtyGeometry::new(
            RUN_COLS,
            RUN_ROWS,
            plan.geometry.cell_width,
            plan.geometry.cell_height,
        ))
        .fixed_size();
    for arg in args {
        spec = spec.arg(arg);
    }
    spec = spec.apply(cide_core::child_env::terminal_child_env(
        &plan.claude,
        env!("CARGO_PKG_VERSION"),
        Vec::new(),
    ));
    spec = spec.apply(plan.proxy.changes().to_vec());
    spec = spec.apply(plan.env.clone());
    spec = spec.env("CIDE_RUN", plan.run.to_string());
    if let Some(sock) = &plan.agent_sock {
        spec = spec.env("CIDE_AGENT_SOCK", sock.to_string_lossy().to_string());
    }

    Ok(HarnessSpawn {
        spec,
        // The prompt went in the argv; see step 7.
        opening: None,
        binding: SessionBinding::Harness {
            capture,
            keep: keep_event,
            render: render_event,
        },
        // The stream *is* this harness's stdout; nothing is written beside it.
        events: None,
    })
}

/// The paragraphs a role is told, joined the way every harness joins them.
///
/// The role's own words; then, only when the tracker is attached, the tracker paragraph, the
/// OpenSpec paragraph for a run working a change, and the ad-hoc correction for a run with no
/// task — `claude.rs`'s fold order and gate, `opencode.rs`'s `config_json` order and gate. A
/// blank paragraph contributes nothing, as `fold_append_system_prompt` treats one.
///
/// `pub(crate)` for `harness.rs`'s parity tests, which hold this to what claude's argv carries
/// so a role is told one thing whichever binary runs it.
pub(crate) fn developer_brief(plan: &RunPlan<'_>) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut say = |text: String| {
        if !text.trim().is_empty() {
            paragraphs.push(text);
        }
    };
    say(plan.agent.def.system_prompt.clone());
    // `&& tracker_paragraphs`: an MR review keeps the bridge and hears none of this (M85).
    if plan.hook_bin.is_some() && plan.tracker_paragraphs {
        say(tracker_preamble(&CodexHarness));
        if let Some(change) = &plan.change {
            say(spec_preamble(
                change,
                plan.spec_cli.as_deref(),
                plan.spec_apply.as_deref(),
                &CodexHarness,
            ));
        }
        if plan.task.is_none() {
            say(ADHOC_PREAMBLE.to_string());
        }
    }
    paragraphs.join("\n\n")
}

/// The sandbox tokens for a definition's claude-vocabulary permission mode, or for the
/// project default when the definition names none. The module header carries the argument for
/// every row; the short form is that `exec` cannot ask, so a mode is what the sandbox lets
/// through without asking.
fn permission_policy(
    mode: Option<&str>,
    unattended: Unattended,
) -> Result<&'static [&'static str], HarnessError> {
    const BYPASS: &[&str] = &["--dangerously-bypass-approvals-and-sandbox"];
    const WORKSPACE: &[&str] = &["-s", "workspace-write"];
    const READ_ONLY: &[&str] = &["-s", "read-only"];
    const AUTO_REVIEW: &[&str] = &["--approve-for-me"];
    const DEFAULT: &[&str] = &[];
    Ok(match mode {
        Some("bypassPermissions") => BYPASS,
        // Edits inside the checkout without asking, everything beyond refused and returned to
        // the model — which is also what `dontAsk` means, on a CLI that never asks anyway.
        Some("acceptEdits") | Some("dontAsk") => WORKSPACE,
        Some("plan") => READ_ONLY,
        // Claude's `auto` is a classifier approving on the user's behalf; codex's flag routes
        // approvals through its own reviewer.
        Some("auto") => AUTO_REVIEW,
        // "Ask a person": there is nobody at a headless `exec`, and every mapping would drop the
        // one restriction the author wrote.
        Some(other) => {
            return Err(HarnessError::NoEquivalent {
                harness: cide_ipc::Harness::Codex,
                what: format!("`permission-mode: {other}`"),
            });
        }
        // The project default. `auto` here is the promptless flag and not `--approve-for-me`,
        // and the choice is deliberate. That flag keeps the `workspace-write` sandbox, which
        // re-binds `.git` read-only, so a task run could never make the commit its brief asks
        // for (the module header). The project chose "an unattended run keeps working", and
        // on this CLI the only way to keep that is the bypass it always had. A *role* that
        // writes `auto` gets the literal mapping above.
        None => match unattended {
            Unattended::Auto | Unattended::Bypass => BYPASS,
            Unattended::Ask => DEFAULT,
        },
    })
}

/// `-c <key>=<value>`, as two tokens. `key` is a bare dotted path with no `=` in it, so the
/// CLI's split on the first `=` is unambiguous however many the value carries.
fn push_config(args: &mut Vec<String>, key: &str, toml_value: String) {
    args.push("-c".into());
    args.push(format!("{key}={toml_value}"));
}

/// `text` as a TOML basic string, which is the only spelling under which a `-c` value is
/// guaranteed to reach codex as the string it was (the module header). The escapes are TOML's
/// own: the two characters that end or escape a basic string, the named controls, and every
/// other control character and DEL as `\u00XX`. Everything else, non-ASCII included, is raw.
fn toml_string(text: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, TaskId, Theme};

    use crate::LoadedAgent;
    use crate::harness::render::{ERASE_MARKER, MARKER};

    const THREAD: &str = "0199a8f2-6d1e-7c3a-9b2e-4f5d6a7b8c9d";

    fn role() -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId("developer".into()),
                label: "Developer".into(),
                scope: cide_ipc::agents::AgentScope::Project,
                harness: cide_ipc::Harness::Codex,
                description: "Implements one task end to end.".into(),
                system_prompt: "You are the developer agent. Finish the task.".into(),
                model: None,
                color: None,
                unavailable: None,
                max_concurrent: 1,
                worktree: true,
            },
            origin: PathBuf::from("/repo/.cide/agents/developer.md"),
            shadows: None,
            tools: Vec::new(),
            permission_mode: None,
            effort: None,
            extras: Vec::new(),
        }
    }

    fn plan_for<'a>(agent: &'a LoadedAgent, session: SessionId) -> RunPlan<'a> {
        RunPlan {
            run: RunId::new(),
            session,
            agent,
            cwd: PathBuf::from("/repo/.cide/worktrees/developer-t-14"),
            project: ProjectId::new(),
            task: Some(TaskId("t-14".into())),
            task_title: Some("Teach the parser about tabs".into()),
            change: None,
            spec_cli: None,
            spec_apply: None,
            prompt: "Work on task t-14 (Teach the parser about tabs).".into(),
            hook_bin: Some(PathBuf::from("/opt/cide/cide-hook")),
            hook_sock: Some(PathBuf::from("/run/user/1000/cide-hooks-42.sock")),
            agent_sock: Some(PathBuf::from("/run/user/1000/cide-agents-42.sock")),
            events_path: Some(PathBuf::from("/run/user/1000/cide-run-7.events")),
            theme: Theme::Dark,
            proxy: cide_core::proxy::ProxyEnv::default(),
            env: Vec::new(),
            geometry: Geometry::default(),
            claude: cide_ipc::ClaudeSettings::default(),
            llm: cide_ipc::LlmSettings::default(),
            choice: None,
            harness: agent.def.harness,
            unattended: Unattended::Ask,
            tracker_paragraphs: true,
        }
    }

    fn at(args: &[String], token: &str) -> usize {
        args.iter()
            .position(|a| a == token)
            .unwrap_or_else(|| panic!("no {token} in {args:?}"))
    }

    /// The value of the one `-c <key>=…` override for `key`, panicking on none or two.
    fn config_value(args: &[String], key: &str) -> String {
        let prefix = format!("{key}=");
        let values: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *i > 0 && args[i - 1] == "-c" && a.starts_with(&prefix))
            .map(|(_, a)| a)
            .collect();
        assert_eq!(values.len(), 1, "one `{key}` override: {args:?}");
        values[0][prefix.len()..].to_string()
    }

    fn has_config(args: &[String], key: &str) -> bool {
        let prefix = format!("{key}=");
        args.iter()
            .enumerate()
            .any(|(i, a)| i > 0 && args[i - 1] == "-c" && a.starts_with(&prefix))
    }

    fn env_value<'a>(spec: &'a SpawnSpec, name: &str) -> Option<&'a str> {
        spec.env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The SGR-stripped text, which is what `runLinks.ts` reads off the buffer.
    fn plain(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
            out.push(c);
        }
        out
    }

    fn replaced(rendered: Rendered) -> String {
        match rendered {
            Rendered::Replace(text) => text,
            other => panic!("expected a rendering, got {other:?}"),
        }
    }

    /// The argv a run gets: `exec`, the directory and the git check first, the brief as one
    /// TOML-encoded override, `--json` before the prompt, the prompt last and once.
    #[test]
    fn a_run_is_one_codex_exec_turn_with_the_prompt_last() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let spawn = CodexHarness.spawn_spec(&plan).expect("spawnable");
        let args = &spawn.spec.args;

        assert_eq!(spawn.spec.program, "codex");
        assert_eq!(args[0], "exec");
        assert_eq!(args[at(args, "-C") + 1], plan.cwd.to_string_lossy());
        assert_eq!(
            spawn.spec.cwd, plan.cwd,
            "`-C` and the cwd name one directory"
        );
        assert!(args.iter().any(|a| a == "--skip-git-repo-check"));
        assert_eq!(args[at(args, "--color") + 1], "never");
        assert!(!args.iter().any(|a| a == "resume"));

        assert_eq!(
            args.iter().filter(|a| **a == "--json").count(),
            1,
            "one channel flag: {args:?}"
        );
        assert!(at(args, "--json") < args.len() - 1);
        assert_eq!(args.last(), Some(&plan.prompt), "the prompt is last");
        assert_eq!(
            args.iter().filter(|a| **a == plan.prompt).count(),
            1,
            "and appears once: {args:?}"
        );

        // No sandbox token when neither the role nor the project asked for one: the CLI's own
        // default is the safe end.
        assert!(!args.iter().any(|a| a == "-s"), "{args:?}");
        assert!(
            !args
                .iter()
                .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"),
            "{args:?}"
        );

        assert_eq!(
            env_value(&spawn.spec, "CIDE_RUN"),
            Some(plan.run.to_string()).as_deref()
        );
        assert_eq!(
            env_value(&spawn.spec, "CIDE_AGENT_SOCK"),
            Some("/run/user/1000/cide-agents-42.sock")
        );
        assert!(env_value(&spawn.spec, "TERM").is_some(), "a terminal child");
        assert!(env_value(&spawn.spec, "CIDE_SESSION").is_none());
        assert!(env_value(&spawn.spec, "CIDE_HOOK_SOCK").is_none());
        assert!(
            env_value(&spawn.spec, "CODEX_API_KEY").is_none(),
            "cide never injects a key that outranks the user's login"
        );
        // The terminal list is `terminal_child_env`'s (TMUX, COLUMNS…); nothing of codex's own
        // is switched off, because nothing of codex's blocks a headless turn.
        assert!(
            !spawn
                .spec
                .env_remove
                .iter()
                .any(|name| name.starts_with("CODEX")),
            "{:?}",
            spawn.spec.env_remove
        );

        assert!(spawn.opening.is_none(), "the prompt went in the argv");
        assert_eq!(
            spawn.binding,
            SessionBinding::Harness {
                capture,
                keep: keep_event,
                render: render_event
            }
        );
        assert!(spawn.events.is_none(), "stdout is the channel");
        assert_eq!(spawn.spec.geometry.cols, RUN_COLS);
        assert_eq!(spawn.spec.geometry.rows, RUN_ROWS);
    }

    /// The brief: one `developer_instructions` override carrying the role's words and the
    /// tracker paragraph in this harness's spelling, in every harness's order, gated on the
    /// bridge, and encoded so that what codex decodes is what cide composed.
    #[test]
    fn the_brief_rides_in_one_toml_encoded_developer_instructions_override() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;

        let brief = developer_brief(&plan);
        assert_eq!(
            brief,
            format!(
                "You are the developer agent. Finish the task.\n\n{}",
                tracker_preamble(&CodexHarness)
            )
        );
        assert!(brief.contains("mcp__cide__cide_task_get"), "{brief}");
        assert_eq!(
            config_value(&args, "developer_instructions"),
            toml_string(&brief)
        );

        // A task-less run is told the correction, last (M40); a run on a change is told the
        // spec paragraph before it.
        let mut adhoc = plan_for(&agent, session);
        adhoc.task = None;
        adhoc.change = Some("m28-openspec".into());
        let brief = developer_brief(&adhoc);
        assert!(brief.ends_with(ADHOC_PREAMBLE), "{brief}");
        let spec = brief
            .find("openspec/changes/m28-openspec/")
            .expect("the spec paragraph");
        let tracker = brief
            .find(".cide/tasks.json")
            .expect("the tracker paragraph");
        let adhoc_at = brief
            .find("no task on the board")
            .expect("the ad-hoc paragraph");
        assert!(tracker < spec && spec < adhoc_at, "{brief}");

        // No bridge: the role's own words alone, and no server named anywhere.
        let mut bare = plan_for(&agent, session);
        bare.hook_bin = None;
        let args = CodexHarness.spawn_spec(&bare).expect("spawnable").spec.args;
        assert_eq!(
            config_value(&args, "developer_instructions"),
            toml_string("You are the developer agent. Finish the task.")
        );
        assert!(
            !args.iter().any(|a| a.contains("mcp_servers.")),
            "no tracker, no server: {args:?}"
        );

        // A hostile brief survives as one argv token with TOML's escapes and no raw newline.
        let mut hostile = role();
        hostile.def.system_prompt = "Say \"hello\".\nUse C:\\tools\\bin".into();
        let args = CodexHarness
            .spawn_spec(&plan_for(&hostile, session))
            .expect("spawnable")
            .spec
            .args;
        let value = config_value(&args, "developer_instructions");
        assert!(
            value.starts_with("\"Say \\\"hello\\\".\\nUse C:\\\\tools\\\\bin"),
            "{value}"
        );
        assert!(!value.contains('\n'), "{value:?}");

        // Nothing to say, nothing overridden — and nothing of the user's blanked.
        let mut silent = role();
        silent.def.system_prompt = "   ".into();
        let mut plan = plan_for(&silent, session);
        plan.hook_bin = None;
        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        assert!(!has_config(&args, "developer_instructions"), "{args:?}");
    }

    /// The encoder against the cases TOML would otherwise misread — measured: a bare `42` is
    /// refused as an integer, a quoted one is the string.
    #[test]
    fn toml_basic_strings_escape_what_toml_would_misread() {
        assert_eq!(toml_string("abc"), "\"abc\"");
        assert_eq!(
            toml_string("a \"quoted\" word"),
            "\"a \\\"quoted\\\" word\""
        );
        assert_eq!(toml_string("back\\slash"), "\"back\\\\slash\"");
        assert_eq!(toml_string("one\ntwo"), "\"one\\ntwo\"");
        assert_eq!(toml_string("cr\rlf"), "\"cr\\rlf\"");
        assert_eq!(toml_string("tab\there"), "\"tab\\there\"");
        assert_eq!(toml_string("\u{8}"), "\"\\b\"");
        assert_eq!(toml_string("\u{c}"), "\"\\f\"");
        assert_eq!(toml_string("\u{1}"), "\"\\u0001\"");
        assert_eq!(toml_string("\u{7f}"), "\"\\u007F\"");
        assert_eq!(toml_string("true"), "\"true\"");
        assert_eq!(toml_string("42"), "\"42\"");
        assert_eq!(toml_string("[x]"), "\"[x]\"");
        assert_eq!(toml_string("1979-05-27"), "\"1979-05-27\"");
        assert_eq!(toml_string("é ∴ 日本"), "\"é ∴ 日本\"");
        assert_eq!(toml_string(""), "\"\"");
    }

    /// cide's server as four overrides: the absolute hook, `mcp`, and the two variables the
    /// bridge would otherwise never see — all or none, on the bridge's presence.
    #[test]
    fn the_tracker_is_a_config_override_with_its_own_environment() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;

        assert_eq!(
            config_value(&args, "mcp_servers.cide.command"),
            "\"/opt/cide/cide-hook\""
        );
        assert_eq!(config_value(&args, "mcp_servers.cide.args"), r#"["mcp"]"#);
        assert_eq!(
            config_value(&args, "mcp_servers.cide.env.CIDE_RUN"),
            toml_string(&plan.run.to_string())
        );
        assert_eq!(
            config_value(&args, "mcp_servers.cide.env.CIDE_AGENT_SOCK"),
            "\"/run/user/1000/cide-agents-42.sock\""
        );

        // A hook path with a quote in it is still one well-formed string.
        let mut odd = plan_for(&agent, session);
        odd.hook_bin = Some(PathBuf::from("/opt/ci\"de/cide-hook"));
        odd.agent_sock = None;
        let args = CodexHarness.spawn_spec(&odd).expect("spawnable").spec.args;
        assert_eq!(
            config_value(&args, "mcp_servers.cide.command"),
            "\"/opt/ci\\\"de/cide-hook\""
        );
        assert!(!has_config(&args, "mcp_servers.cide.env.CIDE_AGENT_SOCK"));
        assert!(has_config(&args, "mcp_servers.cide.env.CIDE_RUN"));

        let mut bare = plan_for(&agent, session);
        bare.hook_bin = None;
        let args = CodexHarness.spawn_spec(&bare).expect("spawnable").spec.args;
        assert!(!args.iter().any(|a| a.contains("mcp_servers.")), "{args:?}");
    }

    /// Every row of the mapping, the project default, the model and effort knobs, the dropped
    /// allow-list, and the two structural refusals.
    #[test]
    fn the_definitions_vocabulary_maps_to_sandbox_flags_or_refuses() {
        let session = SessionId::new();
        let sandbox_of = |mode: Option<&str>, unattended: Unattended| -> Vec<String> {
            let mut agent = role();
            agent.permission_mode = mode.map(str::to_string);
            let mut plan = plan_for(&agent, session);
            plan.unattended = unattended;
            let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;
            let start = at(&args, "--skip-git-repo-check") + 1;
            let end = args
                .iter()
                .position(|a| a == "-m" || a == "-c" || a == "--color");
            args[start..end.expect("something follows")].to_vec()
        };

        assert_eq!(
            sandbox_of(Some("bypassPermissions"), Unattended::Ask),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert_eq!(
            sandbox_of(Some("acceptEdits"), Unattended::Ask),
            ["-s", "workspace-write"]
        );
        assert_eq!(
            sandbox_of(Some("dontAsk"), Unattended::Ask),
            ["-s", "workspace-write"]
        );
        assert_eq!(
            sandbox_of(Some("plan"), Unattended::Bypass),
            ["-s", "read-only"]
        );
        assert_eq!(
            sandbox_of(Some("auto"), Unattended::Ask),
            ["--approve-for-me"]
        );
        assert_eq!(
            sandbox_of(None, Unattended::Bypass),
            ["--dangerously-bypass-approvals-and-sandbox"],
            "the project default for unattended children"
        );
        assert_eq!(
            sandbox_of(None, Unattended::Auto),
            ["--dangerously-bypass-approvals-and-sandbox"],
            "a project default of `auto` keeps a run able to commit, which `--approve-for-me` \
             cannot"
        );
        assert!(
            sandbox_of(None, Unattended::Ask).is_empty(),
            "the CLI's own default"
        );

        let mut manual = role();
        manual.permission_mode = Some("manual".into());
        let refused = CodexHarness
            .spawn_spec(&plan_for(&manual, session))
            .expect_err("nobody to ask");
        assert!(
            matches!(refused, HarnessError::NoEquivalent { .. }),
            "{refused}"
        );
        assert!(refused.to_string().contains("manual"), "{refused}");

        // Model and effort.
        let mut tuned = role();
        tuned.def.model = Some("gpt-5.5".into());
        tuned.effort = Some("high".into());
        tuned.tools = vec!["Read".into(), "mcp__cide__cide_task_get".into()];
        let args = CodexHarness
            .spawn_spec(&plan_for(&tuned, session))
            .expect("spawnable")
            .spec
            .args;
        assert_eq!(args[at(&args, "-m") + 1], "gpt-5.5");
        assert_eq!(config_value(&args, "model_reasoning_effort"), "\"high\"");
        // The allow-list is dropped: no token names a tool or an allow flag.
        assert!(
            !args
                .iter()
                .any(|a| a == "Read" || a.contains("allowed") || a.contains("enabled_tools")),
            "{args:?}"
        );

        let mut claude = role();
        claude.def.harness = cide_ipc::Harness::Claude;
        assert!(matches!(
            CodexHarness.spawn_spec(&plan_for(&claude, session)),
            Err(HarnessError::WrongHarness { .. })
        ));

        let mut empty = plan_for(&manual, session);
        empty.agent = &tuned;
        empty.prompt = "  \n".into();
        assert!(matches!(
            CodexHarness.spawn_spec(&empty),
            Err(HarnessError::NoPrompt)
        ));
    }

    /// A follow-up is a new child on `resume <thread>`: every shared token ahead of the
    /// subcommand and identical to the fresh child's, the follow-up prompt last.
    #[test]
    fn a_follow_up_respawns_into_the_thread_the_child_named() {
        let mut agent = role();
        agent.permission_mode = Some("acceptEdits".into());
        let session = SessionId::new();
        let mut plan = plan_for(&agent, session);
        let fresh = CodexHarness.spawn_spec(&plan).expect("spawnable").spec;

        assert_eq!(CodexHarness.deliver("go on"), Delivery::Respawn);

        plan.prompt = "Now also handle vertical tabs.".into();
        let spawn = CodexHarness
            .respawn_spec(&plan, THREAD)
            .expect("continuable");
        let args = &spawn.spec.args;
        let resume = at(args, "resume");
        assert_eq!(args[resume + 1], "--json");
        assert_eq!(args[resume + 2], THREAD);
        assert_eq!(args.last().map(String::as_str), Some(plan.prompt.as_str()));
        assert_eq!(args.iter().filter(|a| **a == "--json").count(), 1);

        // Everything shared sits before `resume`, exactly as the fresh child had it.
        assert_eq!(&args[..resume], &fresh.args[..fresh.args.len() - 2]);
        assert!(at(args, "-C") < resume);
        assert!(at(args, "-s") < resume);
        assert_eq!(
            config_value(args, "developer_instructions"),
            config_value(&fresh.args, "developer_instructions")
        );
        assert_eq!(spawn.spec.cwd, fresh.cwd);
        assert_eq!(spawn.binding, fresh_binding());
    }

    fn fresh_binding() -> SessionBinding {
        SessionBinding::Harness {
            capture,
            keep: keep_event,
            render: render_event,
        }
    }

    // Lines hand-written to the measured 0.153.2 schema.
    const THREAD_STARTED: &str =
        r#"{"type":"thread.started","thread_id":"0199a8f2-6d1e-7c3a-9b2e-4f5d6a7b8c9d"}"#;
    const TURN_STARTED: &str = r#"{"type":"turn.started"}"#;
    const ITEM_REASONING: &str = r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"**Planning**\n\nRead the task first."}}"#;
    const ITEM_CMD_STARTED: &str = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"cargo test --workspace","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
    const ITEM_CMD_UPDATED: &str = r#"{"type":"item.updated","item":{"id":"item_1","type":"command_execution","command":"cargo test --workspace","aggregated_output":"running 3 tests\n","exit_code":null,"status":"in_progress"}}"#;
    const ITEM_CMD_DONE: &str = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"cargo test --workspace","aggregated_output":"running 3 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\n\ntest result: ok. 3 passed\n","exit_code":0,"status":"completed"}}"#;
    const ITEM_CMD_NONZERO: &str = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"grep -r TODO src","aggregated_output":"","exit_code":1,"status":"completed"}}"#;
    const ITEM_CMD_FAILED: &str = r#"{"type":"item.completed","item":{"id":"item_2","type":"command_execution","command":"cargo build","aggregated_output":"   Compiling x\nerror: could not compile `x`\n","exit_code":101,"status":"failed"}}"#;
    const ITEM_CMD_DECLINED: &str = r#"{"type":"item.completed","item":{"id":"item_3","type":"command_execution","command":"cat ~/.cargo/config.toml","aggregated_output":"","exit_code":null,"status":"declined"}}"#;
    const ITEM_FILE_CHANGE: &str = r#"{"type":"item.completed","item":{"id":"item_4","type":"file_change","changes":[{"path":"src/parser.rs","kind":"update"},{"path":"src/tabs.rs","kind":"add"},{"path":"src/old.rs","kind":"delete"}],"status":"completed"}}"#;
    const ITEM_MCP: &str = r#"{"type":"item.completed","item":{"id":"item_5","type":"mcp_tool_call","server":"cide","tool":"cide_task_get","status":"completed","arguments":{"id":"t-14"},"result":{"content":[{"type":"text","text":"{\"id\":\"t-14\"}"}]}}}"#;
    const ITEM_MCP_FAILED: &str = r#"{"type":"item.completed","item":{"id":"item_6","type":"mcp_tool_call","server":"cide","tool":"cide_task_update","status":"failed","arguments":{"id":"t-14","status":"done"},"error":{"message":"done is the reviewer's call"}}}"#;
    const ITEM_MCP_OTHER: &str = r#"{"type":"item.completed","item":{"id":"item_6","type":"mcp_tool_call","server":"github","tool":"list_prs","status":"completed","arguments":{"repo":"cide"}}}"#;
    const ITEM_WEB_SEARCH: &str = r#"{"type":"item.completed","item":{"id":"item_7","type":"web_search","query":"rust tab parsing","results":[]}}"#;
    const ITEM_TODO: &str = r#"{"type":"item.completed","item":{"id":"item_8","type":"todo_list","items":[{"text":"Read the parser","completed":true},{"text":"Add tabs","completed":false}]}}"#;
    const ITEM_MESSAGE: &str = r#"{"type":"item.completed","item":{"id":"item_9","type":"agent_message","text":"The task is already in doing.\n\nChecking the build.\n"}}"#;
    const ITEM_ERROR: &str = r#"{"type":"item.completed","item":{"id":"item_10","type":"error","message":"stream disconnected"}}"#;
    const ITEM_UNKNOWN: &str = r#"{"type":"item.completed","item":{"id":"item_11","type":"collab_agent_tool_call","agent":"reviewer"}}"#;
    const TURN_COMPLETED: &str = r#"{"type":"turn.completed","usage":{"input_tokens":5696,"cached_input_tokens":4096,"output_tokens":412}}"#;
    const TURN_FAILED: &str = r#"{"type":"turn.failed","error":{"message":"quota exceeded"}}"#;
    const ERROR_SOFT: &str =
        r#"{"type":"error","message":"Reconnecting... 1/5","is_critical":false}"#;
    const ERROR_HARD: &str =
        r#"{"type":"error","message":"stream disconnected before completion"}"#;
    const BANNER: &str = "\x1b[1mOpenAI Codex\x1b[0m v0.153.2 (research preview)";

    /// The identity is the first event's, and only that event's.
    #[test]
    fn the_thread_id_is_read_off_thread_started_and_nothing_else() {
        assert_eq!(capture(THREAD_STARTED).as_deref(), Some(THREAD));
        assert_eq!(
            capture(&format!("{THREAD_STARTED}\r")).as_deref(),
            Some(THREAD),
            "a PTY line ends in a carriage return"
        );
        assert_eq!(capture(TURN_STARTED), None);
        assert_eq!(capture(ITEM_CMD_DONE), None);
        assert_eq!(capture(BANNER), None);
        assert_eq!(capture(""), None);
        assert_eq!(capture("{not json"), None);
        assert_eq!(capture(r#"{"type":"thread.started","thread_id":""}"#), None);
        assert_eq!(
            capture(r#"{"type":"item.completed","thread_id":"abc","item":{}}"#),
            None,
            "an id on any other event is not the run's"
        );
    }

    /// The state machine: every event says the child is working, nothing but the exit ends
    /// it, and nothing moves a paused or ended run.
    #[test]
    fn the_stream_moves_the_run_and_only_the_exit_ends_it() {
        let h = CodexHarness;
        let step = |from: RunState, line: &str| h.observe(from, Observation::Line(line));

        assert_eq!(
            step(RunState::Starting, THREAD_STARTED),
            Some(RunState::Running)
        );
        assert_eq!(
            step(RunState::Running, TURN_STARTED),
            None,
            "already running"
        );
        assert_eq!(step(RunState::Running, ITEM_CMD_STARTED), None);
        assert_eq!(step(RunState::Running, ITEM_CMD_UPDATED), None);
        assert_eq!(step(RunState::Running, ITEM_CMD_DONE), None);
        assert_eq!(
            step(RunState::Starting, ITEM_CMD_DONE),
            Some(RunState::Running),
            "a run whose first event was missed still leaves Starting"
        );
        assert_eq!(
            step(RunState::Running, TURN_COMPLETED),
            None,
            "no line ends a turn — the exit does"
        );
        assert_eq!(step(RunState::Running, TURN_FAILED), None);
        assert_eq!(step(RunState::Running, ERROR_HARD), None);
        assert_eq!(step(RunState::Running, BANNER), None);
        assert_eq!(
            step(RunState::Paused { since_unix_ms: 1 }, ITEM_MESSAGE),
            None,
            "a frame in flight must not thaw a frozen run"
        );
        assert_eq!(
            step(RunState::Finished { code: 0 }, ITEM_MESSAGE),
            None,
            "a late line never resurrects a finished run"
        );
        assert_eq!(
            h.observe(RunState::Running, Observation::Exit(0)),
            Some(RunState::Finished { code: 0 })
        );
        assert_eq!(
            h.observe(
                RunState::Paused { since_unix_ms: 1 },
                Observation::Exit(143)
            ),
            Some(RunState::Finished { code: 143 }),
            "the exit is ground truth from any state"
        );
        assert_eq!(
            h.observe(RunState::Finished { code: 0 }, Observation::Exit(0)),
            None
        );
    }

    /// A turn's figures, and the subtraction that is the whole of this function. (M80)
    ///
    /// The measured line says `input_tokens: 5696` of which `cached_input_tokens: 4096` — one
    /// prompt of 5,696 tokens, reported two ways. Read as opencode reports its own (`input`
    /// beside the cache rather than over it) the same turn would come back as 9,792, and the
    /// card would show a conversation 72% larger on one harness than the other with nothing
    /// anywhere saying why.
    #[test]
    fn a_completed_turn_reports_what_it_spent() {
        let spent = usage(TURN_COMPLETED).expect("a turn.completed carries its usage");
        assert_eq!(spent.input, 5_696 - 4_096, "the cached half comes out");
        assert_eq!(spent.cache_read, 4_096);
        assert_eq!(spent.output, 412);
        assert_eq!(
            spent.context(),
            5_696 + 412,
            "the prompt as codex counted it, plus what the model wrote"
        );
        // Nothing is invented for what the CLI does not measure.
        assert_eq!(spent.reasoning, 0);
        assert_eq!(spent.cache_write, 0);

        // Another program's wire, so more cached than input must not panic the coalescer
        // thread — it is zero uncached input, which is the only reading that is not a lie.
        let impossible = r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":99,"output_tokens":1}}"#;
        assert_eq!(usage(impossible).expect("still read").input, 0);

        // A turn that reports no usage object at all says nothing, rather than zero: "this run
        // has never reported" and "this step spent nothing" are different claims.
        assert_eq!(usage(r#"{"type":"turn.completed"}"#), None);
        for quiet in [TURN_STARTED, ITEM_MESSAGE, ITEM_CMD_DONE, BANNER, "{}"] {
            assert_eq!(usage(quiet), None, "{quiet}");
        }
    }

    /// The rendering: one line per completed item with the handle at its end, the marker
    /// discipline around a turn, failures red and on screen, the CLI's own prose verbatim.
    #[test]
    fn the_rendering_reads_as_a_transcript_and_hides_no_failure() {
        let mut state = RenderState::default();

        assert_eq!(
            render_event(&mut state, THREAD_STARTED, None),
            Rendered::Drop
        );
        assert!(!state.in_step && !state.marker);

        let start = replaced(render_event(&mut state, TURN_STARTED, None));
        assert_eq!(start, MARKER);
        assert!(state.in_step && state.marker);

        // Progress of a running item: nothing drawn, marker untouched.
        assert_eq!(
            render_event(&mut state, ITEM_CMD_STARTED, None),
            Rendered::Drop
        );
        assert_eq!(
            render_event(&mut state, ITEM_CMD_UPDATED, None),
            Rendered::Drop
        );
        assert!(state.marker, "the marker is still on screen");

        // A completed command under the marker: erase, the line, the marker again.
        let done = replaced(render_event(&mut state, ITEM_CMD_DONE, Some(7)));
        assert!(done.starts_with(ERASE_MARKER), "{done:?}");
        assert!(done.ends_with(MARKER), "{done:?}");
        // The erase is cursor-up, carriage return, clear — so the stripped first line still
        // begins with the `\r`.
        let first = plain(&done)
            .lines()
            .next()
            .unwrap_or("")
            .trim_start_matches('\r')
            .to_string();
        assert!(
            first.starts_with("● shell  cargo test --workspace"),
            "{first}"
        );
        assert!(first.ends_with("#7"), "the handle ends the line: {first}");
        assert!(
            !done.contains("running 3 tests"),
            "output is behind the handle"
        );
        assert!(!done.contains("status"), "no JSON keys leak");

        let mut flat = RenderState::default();
        let nonzero = plain(&replaced(render_event(
            &mut flat,
            ITEM_CMD_NONZERO,
            Some(8),
        )));
        assert_eq!(nonzero, "● shell  grep -r TODO src  exit 1  #8");

        let failed = replaced(render_event(&mut flat, ITEM_CMD_FAILED, Some(9)));
        let failed_plain = plain(&failed);
        let mut lines = failed_plain.lines();
        assert_eq!(lines.next(), Some("✗ shell  cargo build  exit 101  #9"));
        assert_eq!(lines.next(), Some("  error: could not compile `x`"));
        assert!(failed.starts_with(RED), "a failure is red: {failed:?}");

        let declined = plain(&replaced(render_event(&mut flat, ITEM_CMD_DECLINED, None)));
        assert_eq!(
            declined,
            "✗ shell  cat ~/.cargo/config.toml\n  declined by the sandbox or the approval policy"
        );

        let edit = plain(&replaced(render_event(
            &mut flat,
            ITEM_FILE_CHANGE,
            Some(10),
        )));
        assert_eq!(edit, "● edit  ~src/parser.rs +src/tabs.rs -src/old.rs  #10");

        let call = plain(&replaced(render_event(&mut flat, ITEM_MCP, Some(11))));
        assert_eq!(
            call, "● cide_task_get  t-14  #11",
            "cide's own tool by its name"
        );
        let other = plain(&replaced(render_event(&mut flat, ITEM_MCP_OTHER, None)));
        assert_eq!(other, "● github:list_prs  cide");
        let refused = plain(&replaced(render_event(
            &mut flat,
            ITEM_MCP_FAILED,
            Some(12),
        )));
        assert_eq!(
            refused,
            "✗ cide_task_update  t-14 done  #12\n  done is the reviewer's call"
        );

        let message = replaced(render_event(&mut flat, ITEM_MESSAGE, None));
        assert_eq!(
            message, "The task is already in doing.\n\nChecking the build.",
            "the model's own words pass whole"
        );
        // A block of thinking is one row and **none** of the thinking: codex states no clock on
        // any item, so the duration is the silence this item ended — the gap between the last
        // line that drew something and this one. The collapse is the feature, so the assertion
        // that the prose is gone matters as much as the duration.
        let mut timed = RenderState {
            now_unix_ms: Some(20_000),
            last_line_unix_ms: Some(16_800),
            ..RenderState::default()
        };
        let reasoning = replaced(render_event(&mut timed, ITEM_REASONING, Some(21)));
        assert!(reasoning.starts_with(DIM), "{reasoning:?}");
        assert_eq!(plain(&reasoning), "∴ thought  3.2s #21");

        // No clock at all — an unstamped caller, or the first line of a run — draws no duration
        // rather than `0ms`, which would be a measurement cide did not make.
        let unmeasured = plain(&replaced(render_event(&mut flat, ITEM_REASONING, Some(22))));
        assert_eq!(unmeasured, "∴ thought  #22");

        let search = plain(&replaced(render_event(
            &mut flat,
            ITEM_WEB_SEARCH,
            Some(13),
        )));
        assert_eq!(search, "● web_search  rust tab parsing  #13");
        let plan = plain(&replaced(render_event(&mut flat, ITEM_TODO, None)));
        assert_eq!(plan, "· plan 1/2: Add tabs");
        let item_error = replaced(render_event(&mut flat, ITEM_ERROR, None));
        assert_eq!(plain(&item_error), "✗ stream disconnected");
        assert!(item_error.starts_with(RED));
        let unknown = plain(&replaced(render_event(&mut flat, ITEM_UNKNOWN, None)));
        assert_eq!(unknown, "· collab_agent_tool_call");
        let unknown_event = plain(&replaced(render_event(
            &mut flat,
            r#"{"type":"thread.renamed","name":"x"}"#,
            None,
        )));
        assert_eq!(unknown_event, "· thread.renamed");

        // The CLI's own prose: verbatim on a quiet screen, under the erase beneath a marker.
        assert_eq!(render_event(&mut flat, BANNER, None), Rendered::Keep);
        let under = replaced(render_event(&mut state, BANNER, None));
        assert!(
            under.starts_with(ERASE_MARKER) && under.ends_with(MARKER),
            "{under:?}"
        );
        assert!(under.contains("OpenAI Codex"), "{under:?}");

        // The end of the turn takes the marker down with it.
        let finish = replaced(render_event(&mut state, TURN_COMPLETED, None));
        assert!(finish.starts_with(ERASE_MARKER), "{finish:?}");
        assert!(!finish.ends_with(MARKER), "{finish:?}");
        assert_eq!(
            plain(&finish).trim_start_matches(|c| c != '·'),
            "· done · 6.1k tok"
        );
        assert!(!state.in_step && !state.marker);

        let mut failing = RenderState::default();
        replaced(render_event(&mut failing, TURN_STARTED, None));
        let failed_turn = replaced(render_event(&mut failing, TURN_FAILED, None));
        assert!(failed_turn.contains(RED), "{failed_turn:?}");
        assert!(
            plain(&failed_turn).ends_with("✗ turn failed: quota exceeded"),
            "{failed_turn:?}"
        );
        assert!(!failing.in_step && !failing.marker);

        let soft = replaced(render_event(&mut flat, ERROR_SOFT, None));
        assert_eq!(plain(&soft), "· Reconnecting... 1/5");
        assert!(soft.starts_with(DIM));
        let hard = replaced(render_event(&mut flat, ERROR_HARD, None));
        assert_eq!(plain(&hard), "✗ stream disconnected before completion");
        assert!(hard.starts_with(RED));

        // What the ring keeps.
        for kept in [
            ITEM_CMD_DONE,
            ITEM_CMD_FAILED,
            ITEM_MCP,
            ITEM_FILE_CHANGE,
            ITEM_MESSAGE,
            ITEM_ERROR,
            ERROR_HARD,
            TURN_FAILED,
        ] {
            assert!(keep_event(kept), "{kept}");
        }
        // Reasoning joined the kept list in M62: the row draws none of it, so this ring is the
        // only copy — and the row names the handle, which is the other half of the pairing.
        assert!(keep_event(ITEM_REASONING));
        for dropped in [
            THREAD_STARTED,
            TURN_STARTED,
            TURN_COMPLETED,
            ITEM_CMD_STARTED,
            ITEM_CMD_UPDATED,
            BANNER,
        ] {
            assert!(!keep_event(dropped), "{dropped}");
        }
    }

    /// The real harness on an ended run's conversation is the TUI on its uuid, and the id is
    /// a uuid or it is not a conversation.
    #[test]
    fn continuing_a_conversation_resumes_the_tui_on_its_uuid() {
        let spec = CodexHarness
            .continue_spec(&HarnessSession {
                harness: cide_ipc::Harness::Codex,
                id: format!(" {THREAD} "),
                cwd: PathBuf::from("/p/.cide/worktrees/developer-t-14"),
            })
            .expect("a uuid is a conversation");
        assert_eq!(spec.program, "codex");
        assert_eq!(
            spec.args,
            vec![
                "resume".to_string(),
                THREAD.to_string(),
                "--include-non-interactive".to_string()
            ]
        );
        assert!(spec.resume.is_none(), "the id rides in the args");

        let refused = CodexHarness
            .continue_spec(&HarnessSession {
                harness: cide_ipc::Harness::Codex,
                id: "thread_abc".into(),
                cwd: PathBuf::from("/p"),
            })
            .expect_err("not a uuid");
        assert!(matches!(refused, HarnessError::NotAConversation { .. }));

        let wrong = CodexHarness
            .continue_spec(&HarnessSession {
                harness: cide_ipc::Harness::Opencode,
                id: THREAD.into(),
                cwd: PathBuf::from("/p"),
            })
            .expect_err("somebody else's conversation");
        assert!(matches!(wrong, HarnessError::WrongHarness { .. }));

        assert_eq!(
            CodexHarness.tool_name("cide_task_get"),
            "mcp__cide__cide_task_get"
        );
    }

    /// The catalog's listed slugs, in its order, once each; a warning above the document is
    /// absorbed; anything else is a sentence.
    #[test]
    fn the_model_catalog_keeps_listed_slugs_in_the_clis_order() {
        const CATALOG: &str = r#"{"fetched_at":"2026-09-07T07:36:48Z","etag":"x","client_version":"0.153.2","models":[{"slug":"gpt-5.6-terra","display_name":"GPT-5.6-Terra","visibility":"list"},{"slug":"gpt-reserve","visibility":"hide"},{"slug":"o3"},{"slug":"gpt-5.5","visibility":"list"},{"slug":"gpt-5.6-terra","visibility":"list"},{"slug":"gpt-5.4-mini","visibility":"list"}]}"#;
        assert_eq!(
            model_slugs(CATALOG).expect("a catalog"),
            ["gpt-5.6-terra", "gpt-5.5", "gpt-5.4-mini"]
        );
        assert_eq!(
            model_slugs(&format!("codex 0.153.4 is available\n{CATALOG}\n")).expect("absorbed"),
            ["gpt-5.6-terra", "gpt-5.5", "gpt-5.4-mini"]
        );
        assert!(model_slugs("").is_err());
        assert!(model_slugs("{not json").is_err());
        assert_eq!(
            model_slugs(r#"{"models":[]}"#).expect("empty is honest"),
            Vec::<String>::new()
        );
    }

    /// The real catalog, from the machine's own `codex`: non-empty, and every slug a single word.
    #[test]
    #[ignore = "runs the real codex; needs the binary on PATH"]
    fn the_real_codex_lists_models() {
        let models = CodexHarness
            .models(None, &cide_ipc::LlmSettings::default())
            .expect("a catalog");
        assert!(!models.is_empty(), "{models:?}");
        assert!(
            models.iter().all(|m| !m.chars().any(char::is_whitespace)),
            "{models:?}"
        );
    }
}
