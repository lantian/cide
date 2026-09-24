//! Running a role on Qwen Code (`qwen`). (M43)
//!
//! Measured against **0.23.0**, installed by npm under nvm, not remembered. Where a paragraph
//! below says *measured*, a command was run in a pty or the shipped bundle was read; where it
//! says *unmeasured*, the user's model endpoint was down that morning and the fact is pinned by
//! an `#[ignore]`d test rather than by a probe.
//!
//! # Claude-shaped on the outside, hosted like `claude`, observed like `opencode`
//!
//! The CLI's surface is Claude Code's, flag for flag where it matters: `--session-id <uuid>` and
//! `--resume <id>` (measured: a session is filed under
//! `~/.qwen/projects/<cwd with non-alnum → '-'>/chats/<uuid>.jsonl`, the same encoding as
//! claude's with a `chats/` segment, so the worktree rule holds unchanged);
//! `--append-system-prompt`; an inline `--mcp-config` (measured: the server is spawned and asked
//! for its tools at startup); MCP tools named **`mcp__<server>__<tool>`**, so [`SERVER`] gives
//! the tracker the spelling `mcp__cide__cide_task_get`; `--allowed-tools`; `--approval-mode`
//! with a `yolo` rung. So the identity is [`SessionBinding::Caller`] exactly as claude's: cide
//! mints the uuid before the fork, resume is free, an interrupted run continues under its own id.
//!
//! What it does **not** share is the state channel. Its hooks exist, with claude's eleven event
//! names and payload keys, but they are configured only in settings files — the user's home or
//! the project's `.qwen/settings.json` — and there is no `--settings`. cide will not write into
//! either. What the CLI offers instead is **dual output**: `--json-file <path>` writes stream-json
//! events (`system`, `user`, `assistant`, `stream_event`, `result`, `control_request`) to a file
//! or FIFO *while the TUI paints on stdout*. Measured: with a FIFO on the flag and `-i <prompt>`
//! the TUI comes up in the pty, the events arrive on the FIFO, and the process stays at its
//! prompt after the turn. The CLI opens the FIFO read-write itself (its own error text says
//! "opening FIFO for read-write"), so nothing blocks on the order of opens.
//!
//! So a run is the "interactive pane nobody is looking at" of `claude.rs`, with the prompt in
//! the argv (`-i` is an option, not a positional, so there is no variadic flag to swallow it and
//! no boot-buffer paste to peel an Enter off) and the state read off the FIFO by the app's event
//! tap, one line at a time, into [`Harness::observe`] — the same `Observation::Line` opencode's
//! stdout takes. Follow-ups are typed into the TUI, claude's [`Delivery::Stdin`].
//!
//! # What ends a turn
//!
//! **Superseded (M86):** the TUI's dual mode never writes `result` — see [`TurnTracker`], which
//! recovers the turn's end from `message_stop` and hands the tap [`TURN_OVER`] in its place. The
//! paragraph below is what M43 expected, kept because the mapping it describes still reads it.
//!
//! A `result` event. Its emitter in the bundle carries `subtype`, `is_error`, `duration_ms`,
//! `num_turns` and `usage` — claude's `-p` envelope — and the `session_start` event's
//! `supported_events` lists it for the dual mode. **Unmeasured** on a completed turn (the
//! endpoint was down); a `stream_event` of `message_stop` is deliberately *not* read as the
//! end, because a turn that calls tools contains several messages, which is opencode's
//! run-06202dd6 lesson one harness over. A `control_request` whose request is `can_use_tool`
//! is the permission prompt in the control protocol; whether the dual mode writes one while the
//! TUI asks is the second unmeasured fact, and until it is measured a role that wants prompts
//! (any approval mode but `yolo`) is not refused, merely unobserved at that moment.
//!
//! # The definition's vocabulary, mapped or refused
//!
//! `permission-mode` is claude's set (`defs::PERMISSION_MODES`); the CLI's is `plan | default |
//! auto-edit | auto | yolo`. [`approval_mode`] maps what has a counterpart and refuses what
//! does not ([`HarnessError::NoEquivalent`]) — `dontAsk` above all, which means "deny silently"
//! and has no rung here. `effort` has no flag at all and is refused for the same reason: a
//! restriction the author wrote that silently did not apply is the failure that looks like
//! success.
//!
//! # The environment
//!
//! [`cide_core::child_env::terminal_child_env`] with an **empty** `extra`, opencode's rule: the
//! user's *Claude sessions* launch variables are not this child's to inherit. `CIDE_RUN` and
//! `CIDE_AGENT_SOCK` scope the bridge. `QWEN_CODE_IDE_SERVER_PORT` and
//! `QWEN_CODE_IDE_WORKSPACE_PATH` are **removed** for `claude.rs`'s reason: an IDE integration
//! that blocks a turn on a tab nobody will answer.
//!
//! # Finding the binary
//!
//! `qwen` is a `#!/usr/bin/env node` script under a Node version manager. It resolves on a
//! desktop launch because a child's `PATH` carries `toolchain::discovered_dirs` — the login
//! shell's directories, which is where nvm puts its `bin` — and `node` lives beside it.

use crate::config::Unattended;
use cide_ipc::{HarnessSession, RunState};
use cide_pty::{Geometry as PtyGeometry, SpawnSpec};
use serde::Deserialize;

use super::{
    ADHOC_PREAMBLE, ContinueSpec, Delivery, ESC, Harness, HarnessError, HarnessSpawn, Observation,
    RunPlan, SERVER, SessionBinding, tracker_preamble,
};

/// The Qwen Code CLI as a harness. A unit struct: it holds nothing, and must not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QwenHarness;

impl Harness for QwenHarness {
    fn kind(&self) -> cide_ipc::Harness {
        cide_ipc::Harness::Qwen
    }

    /// `mcp__<server>__<tool>` — claude's namespacing, verbatim; the bundle spells the pattern
    /// `mcp__${serverName}__${toolName}` where it builds the allow-list.
    fn tool_name(&self, tool: &str) -> String {
        format!("mcp__{SERVER}__{tool}")
    }

    fn spawn_spec(&self, plan: &RunPlan<'_>) -> Result<HarnessSpawn, HarnessError> {
        assemble(plan, false)
    }

    /// The continuing child for a run restored from the registry's snapshot — `qwen --resume`
    /// into the same worktree the chat file lives under. `_session` is redundant by construction
    /// on this harness, for `claude.rs`'s reason: the caller chose the id and `ResumePoint`
    /// rebinds the run to it before the fork.
    fn respawn_spec(
        &self,
        plan: &RunPlan<'_>,
        _session: &str,
    ) -> Result<HarnessSpawn, HarnessError> {
        assemble(plan, true)
    }

    /// Typed into the TUI, claude's shape: the process outlives its turns.
    fn deliver(&self, text: &str) -> Delivery {
        Delivery::Stdin(submit(text))
    }

    /// Esc, which is how a person at this TUI ends a turn in flight.
    ///
    /// One byte and not two: a second Esc is the history gesture, not a harder interrupt, and a
    /// pair sent together would open the composer's history over the wind-down line about to be
    /// typed. See `cide_app::agents`' stop road for the beat that has to follow it — bytes
    /// arriving in the same `read` as the Esc are read by a TUI that is still tearing its turn
    /// down, which is `type_submitted_line`'s measured failure in a new place.
    fn interrupt(&self) -> Option<Vec<u8>> {
        Some(vec![ESC])
    }

    /// The event stream, as the app's tap hands it over line by line.
    ///
    /// | observation | answer |
    /// | --- | --- |
    /// | `Exit(code)` | `Finished { code }`, from any state — the one ground truth about the child |
    /// | `Line` — `result` | `Idle`: the turn is over and the child is at its prompt |
    /// | `Line` — `control_request` / `can_use_tool` | `AwaitingPermission` |
    /// | `Line` — `system`, `user`, `assistant`, `stream_event` | `Running` |
    /// | `Line` — anything else, or not JSON | `None` |
    /// | `Hook` | `None` — nothing installs a hook into this CLI |
    ///
    /// A line moves only a run that has a live child to describe (`Starting`, `Running`,
    /// `Idle`, `AwaitingPermission`): a `Paused` run's in-flight frame must not thaw it, and a
    /// late line never resurrects a `Finished` one — `claude.rs`'s two refusals.
    fn observe(&self, current: RunState, ob: Observation<'_>) -> Option<RunState> {
        match ob {
            Observation::Exit(code) => {
                let next = RunState::Finished { code };
                (next != current).then_some(next)
            }
            Observation::Hook(_) => None,
            Observation::Line(line) => {
                if !matches!(
                    current,
                    RunState::Starting
                        | RunState::Running
                        | RunState::Idle
                        | RunState::AwaitingPermission
                ) {
                    return None;
                }
                let event = Event::parse(line)?;
                let next = match event.kind.as_str() {
                    "result" => RunState::Idle,
                    "control_request"
                        if event
                            .request
                            .as_ref()
                            .and_then(|request| request.subtype.as_deref())
                            == Some("can_use_tool") =>
                    {
                        RunState::AwaitingPermission
                    }
                    "system" | "user" | "assistant" | "stream_event" => RunState::Running,
                    _ => return None,
                };
                (next != current).then_some(next)
            }
        }
    }

    /// `qwen --resume <uuid>` — the real TUI on the conversation, from the run's directory.
    ///
    /// The id travels in the args rather than as `resume`: `cide_claude::conversation` writes
    /// claude's flags for claude's binary, and `session_spawn` hosts this program on the
    /// shell-like path, where `resume` means a screen replay.
    fn continue_spec(&self, conversation: &HarnessSession) -> Result<ContinueSpec, HarnessError> {
        if conversation.harness != cide_ipc::Harness::Qwen {
            return Err(HarnessError::WrongHarness {
                plan: conversation.harness,
                harness: cide_ipc::Harness::Qwen,
            });
        }
        let id = conversation.id.trim();
        if id.parse::<cide_ipc::SessionId>().is_err() {
            return Err(HarnessError::NotAConversation {
                harness: cide_ipc::Harness::Qwen,
                id: conversation.id.clone(),
            });
        }
        Ok(ContinueSpec {
            program: crate::defs::harness_binary(cide_ipc::Harness::Qwen).to_string(),
            args: vec!["--resume".into(), id.to_string()],
            resume: None,
        })
    }
}

/// One event line off the FIFO. Only the two fields [`QwenHarness::observe`] reads.
#[derive(Debug, Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    request: Option<Request>,
}

/// The `request` of a `control_request`, whose `subtype` names what is being asked.
#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    subtype: Option<String>,
}

impl Event {
    /// One line, or `None` when it is not one of ours — the tap reads a file the CLI alone
    /// writes, so a non-JSON line is a shape this release does not have rather than prose.
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if !line.starts_with('{') {
            return None;
        }
        serde_json::from_str(line).ok()
    }
}

/// The line [`TurnTracker`] hands the tap when a turn is over, for [`QwenHarness::observe`] to
/// read exactly as it would read the CLI's own `result`. Marked `synthetic` so the post-mortem
/// log, which gets it too, does not pass it off as something the child said.
pub const TURN_OVER: &str = r#"{"type":"result","subtype":"turn_over","synthetic":true}"#;

/// Where a turn ends, recovered from the stream the dual mode actually writes. (M86)
///
/// # Why this exists: the `result` the module header waited for never comes
///
/// Read out of the 0.24.4 bundle rather than remembered: the interactive UI's `DualOutputBridge`
/// has `processEvent`, `startAssistantMessage`, `finalizeAssistantMessage`, `emitUserMessage`,
/// `emitToolResult`, the permission pair and `emitSystemMessage` — and **no `emitResult`**. The
/// `supported_events` list on `session_start` names `result` because the list is shared with
/// `-p`'s adapter, not because the TUI emits one. So a qwen run went `Running` on its first
/// line and stayed there for the life of the child: selfcraft's qa-tester on t-239 set its task
/// to review, sat at its prompt, and held its slot and its worktree for over two hours with the
/// board saying `Running`.
///
/// # What ends a turn instead
///
/// The adapter splits one API response into an `assistant` line per block type (thinking, then
/// text, then the tool calls) and closes the response with a main-agent `stream_event` of
/// `message_stop`. The tool-call line carries `stop_reason: "tool_use"`, the only value that
/// field ever takes besides `null`. So a response is the last of its turn when its
/// `message_stop` arrives and **no `tool_use` block was seen since its `message_start`**: a
/// response that called tools is followed by those tools running and another response, one
/// that did not is the model handing the turn back.
///
/// Why state, on a harness whose struct "holds nothing, and must not": the fact is spread over
/// two lines, and at `message_stop` alone the two cases are byte-identical. The option that lost
/// was mapping every `message_stop` to `Idle` and letting the next `user` tool-result line flip
/// it back — which reads `Idle` for as long as every tool runs (a bot session is minutes), and
/// `Idle` is the moment the orchestrator is nudged and the slot released. So the memory lives
/// here, one per run, owned by the event tap that reads that run's lines in order, and
/// [`QwenHarness`] stays a pure map.
///
/// A subagent's lines (`parent_tool_use_id` set) never end the parent's turn and never mark it
/// as having called tools; the `task` call that started the subagent already did.
///
/// If a later CLI starts writing its own `result`, both arrive and the second is `Idle → Idle`,
/// which `observe` answers with `None`.
///
/// What this cannot see: a turn the API fails mid-response, which the TUI reports and which
/// writes no `message_stop`. That run stays `Running` as before, until a person types into it
/// or it is stopped.
#[derive(Debug, Default)]
pub struct TurnTracker {
    called_tools: bool,
}

impl TurnTracker {
    /// Feed one line off the FIFO, in order. `Some(TURN_OVER)` when this line closed the turn.
    pub fn feed(&mut self, line: &str) -> Option<&'static str> {
        let event: TurnEvent = serde_json::from_str(line.trim()).ok()?;
        if event.parent_tool_use_id.is_some() {
            return None;
        }
        match (
            event.kind.as_str(),
            event.event.as_ref(),
            event.message.as_ref(),
        ) {
            ("stream_event", Some(inner), _) if inner.kind == "message_start" => {
                self.called_tools = false;
                None
            }
            ("stream_event", Some(inner), _) if inner.kind == "message_stop" => {
                let over = !self.called_tools;
                self.called_tools = false;
                over.then_some(TURN_OVER)
            }
            ("assistant", _, Some(message)) => {
                if message.stop_reason.as_deref() == Some("tool_use")
                    || message.content.iter().any(|block| block.kind == "tool_use")
                {
                    self.called_tools = true;
                }
                None
            }
            _ => None,
        }
    }
}

/// The fields of a line [`TurnTracker`] reads, and only those.
#[derive(Debug, Deserialize)]
struct TurnEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    parent_tool_use_id: Option<String>,
    #[serde(default)]
    event: Option<Kind>,
    #[serde(default)]
    message: Option<TurnMessage>,
}

#[derive(Debug, Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct TurnMessage {
    #[serde(default)]
    content: Vec<Kind>,
    #[serde(default)]
    stop_reason: Option<String>,
}

/// Build the child, fresh (`resume: false`) or continuing. One function so the two differ in
/// exactly the identity tokens — `claude.rs::assemble`'s shape.
fn assemble(plan: &RunPlan<'_>, resume: bool) -> Result<HarnessSpawn, HarnessError> {
    // `plan.harness`, the *resolved* one, not the definition's: a role a local override moved onto
    // this CLI must not be refused by it. See `RunPlan::harness`.
    if plan.harness != cide_ipc::Harness::Qwen {
        return Err(HarnessError::WrongHarness {
            plan: plan.harness,
            harness: cide_ipc::Harness::Qwen,
        });
    }
    // An interactive `qwen` with nothing to do starts perfectly and sits at its prompt holding
    // a slot and a worktree; `HarnessError::NoPrompt` says why that is refused, not started.
    if plan.prompt.trim().is_empty() {
        return Err(HarnessError::NoPrompt);
    }
    let program = crate::defs::harness_binary(cide_ipc::Harness::Qwen);

    // Whether this run gets the tracker at all — the one question the brief and the flag both
    // answer, computed once so they cannot disagree (`claude.rs`'s "the one question steps 3
    // and 5 both answer").
    let tracker = plan
        .hook_bin
        .as_ref()
        .and_then(|hook| mcp_config(&hook.to_string_lossy()));

    let mut args: Vec<String> = Vec::new();

    // ---- 1. the prompt, as `-i`'s value ----
    //
    // Measured: a positional prompt is a one-shot that exits after the answer; `-i` runs it and
    // stays interactive, which is the whole hosting model. An option value rather than a
    // positional also means no variadic flag can swallow it — the hazard `claude.rs` types its
    // prompt into the terminal to avoid — and the argv accepts a newline where a typed line
    // could not.
    args.push("-i".into());
    args.push(plan.prompt.clone());

    // ---- 2. identity ----
    if resume {
        args.push("--resume".into());
    } else {
        args.push("--session-id".into());
    }
    args.push(plan.session.to_string());
    // Explicit, although it is the default: `--continue/--resume will not work` without it,
    // and a settings file could have switched it off for every session on this machine.
    args.push("--chat-recording".into());

    // ---- 3. the role's brief, in one flag ----
    //
    // Through `fold_append_system_prompt` so there is exactly one `--append-system-prompt`
    // whatever this function appends below: a second one's fate on this CLI is unmeasured, and
    // one flag is right on either reading. The tracker paragraph follows the role's own words
    // only when the tracker is attached, then the OpenSpec paragraph, then the ad-hoc one — the
    // same order and the same gate as `claude.rs`.
    cide_core::claude_cli::fold_append_system_prompt(&mut args, &plan.agent.def.system_prompt);
    // `plan.tracker_paragraphs` is off for a run served another tool set (M85): an MR review
    // keeps its bridge but must not be told to use tools its connection does not list.
    if tracker.is_some() && plan.tracker_paragraphs {
        cide_core::claude_cli::fold_append_system_prompt(
            &mut args,
            &tracker_preamble(&QwenHarness),
        );
        if let Some(change) = &plan.change {
            cide_core::claude_cli::fold_append_system_prompt(
                &mut args,
                &super::spec_preamble(
                    change,
                    plan.spec_cli.as_deref(),
                    plan.spec_apply.as_deref(),
                    &QwenHarness,
                ),
            );
        }
        if plan.task.is_none() {
            cide_core::claude_cli::fold_append_system_prompt(&mut args, ADHOC_PREAMBLE);
        }
    }

    // ---- 4. what the definition asked for, mapped or refused ----
    if let Some(model) = &plan.agent.def.model {
        args.push("-m".into());
        args.push(model.clone());
    }
    if let Some(effort) = &plan.agent.effort {
        return Err(HarnessError::NoEquivalent {
            harness: cide_ipc::Harness::Qwen,
            what: format!("an effort level (`effort: {effort}`)"),
        });
    }
    if let Some(mode) = approval_mode(plan.agent.permission_mode.as_deref(), plan.unattended)? {
        args.push("--approval-mode".into());
        args.push(mode.into());
    }

    // ---- 5. the event file, the tracker ----
    if let Some(path) = &plan.events_path {
        args.push("--json-file".into());
        args.push(path.to_string_lossy().to_string());
    }
    if let Some(json) = &tracker {
        args.push("--mcp-config".into());
        args.push(json.clone());
    }

    // ---- 6. last: the allow-list ----
    //
    // A yargs array collects every following token up to the next `--flag`, so it goes after
    // everything else. An argument added below this line has to think about that.
    if !plan.agent.tools.is_empty() {
        args.push("--allowed-tools".into());
        args.extend(plan.agent.tools.iter().cloned());
    }

    let mut spec = SpawnSpec::new(program, plan.cwd.clone()).geometry(PtyGeometry::new(
        plan.geometry.cols,
        plan.geometry.rows,
        plan.geometry.cell_width,
        plan.geometry.cell_height,
    ));
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
    spec = spec
        .env_remove("QWEN_CODE_IDE_SERVER_PORT")
        .env_remove("QWEN_CODE_IDE_WORKSPACE_PATH");
    spec = spec.env("CIDE_RUN", plan.run.to_string());
    if let Some(sock) = &plan.agent_sock {
        spec = spec.env("CIDE_AGENT_SOCK", sock.to_string_lossy().to_string());
    }

    Ok(HarnessSpawn {
        spec,
        // The prompt went in the argv; see step 1.
        opening: None,
        binding: SessionBinding::Caller,
        events: plan.events_path.clone(),
    })
}

/// The CLI's approval mode for a definition's claude-vocabulary permission mode, or for the
/// project default when the definition names none.
///
/// The role's own word always wins. The project's `agents.permissionMode` fills in only when
/// the role wrote nothing, and `None` there is the CLI's own default (ask), which is the safe
/// end of the range.
fn approval_mode(
    mode: Option<&str>,
    unattended: Unattended,
) -> Result<Option<&'static str>, HarnessError> {
    let refuse = |what: String| HarnessError::NoEquivalent {
        harness: cide_ipc::Harness::Qwen,
        what,
    };
    Ok(match mode {
        Some("bypassPermissions") => Some("yolo"),
        Some("acceptEdits") => Some("auto-edit"),
        Some("plan") => Some("plan"),
        Some("auto") => Some("auto"),
        Some("manual") => Some("default"),
        Some(other) => {
            return Err(refuse(format!("`permission-mode: {other}`")));
        }
        // The project default, mapped literally like claude's. From M82 `auto` here kept
        // `yolo`, on the grounds that qwen's own `auto` (its LLM classifier) had never been
        // measured in a headless child — which meant a project that chose `auto` got a qwen
        // run with no brake at all while the panel said otherwise. The CLI lists `auto` among
        // `--approval-mode`'s choices (0.24.4); a project that wants no brake says `bypass`.
        None => match unattended {
            Unattended::Auto => Some("auto"),
            Unattended::Bypass => Some("yolo"),
            Unattended::Ask => None,
        },
    })
}

/// A typed line, terminated the way the TUI submits one. `claude.rs::submit`'s shape.
fn submit(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 1);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(b'\r');
    bytes
}

/// The inline `--mcp-config` JSON attaching cide's own MCP server — `claude.rs::mcp_config`,
/// because the flag and the document are the same on this CLI (measured: an inline string is
/// parsed, the server spawned, `tools/list` asked).
fn mcp_config(hook_bin: &str) -> Option<String> {
    serde_json::to_string(&serde_json::json!({
        "mcpServers": {
            SERVER: {
                "command": hook_bin,
                "args": ["mcp"],
            }
        }
    }))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, TaskId, Theme};

    use crate::LoadedAgent;

    fn role() -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId("developer".into()),
                label: "Developer".into(),
                scope: cide_ipc::agents::AgentScope::Project,
                harness: cide_ipc::Harness::Qwen,
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
            codex: cide_ipc::CodexSettings::default(),
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

    fn env_value<'a>(spec: &'a SpawnSpec, name: &str) -> Option<&'a str> {
        spec.env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The argv a run gets: the prompt as `-i`'s value and nowhere else, the caller's uuid, one
    /// system-prompt flag carrying the brief and the tracker paragraph, the event file, the
    /// inline MCP config, the allow-list last.
    #[test]
    fn a_run_is_an_interactive_qwen_with_the_prompt_as_an_option() {
        let mut agent = role();
        agent.tools = vec!["mcp__cide__cide_task_get".into()];
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let spawn = QwenHarness.spawn_spec(&plan).expect("spawnable");
        let args = &spawn.spec.args;

        assert_eq!(spawn.spec.program, "qwen");
        assert_eq!(args[at(args, "-i") + 1], plan.prompt);
        assert_eq!(
            args.iter().filter(|a| **a == plan.prompt).count(),
            1,
            "the prompt is an option value, never a positional one-shot: {args:?}"
        );
        assert_eq!(args[at(args, "--session-id") + 1], session.to_string());
        assert!(!args.iter().any(|a| a == "--resume"));
        assert!(args.iter().any(|a| a == "--chat-recording"));

        let briefs: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--append-system-prompt")
            .map(|(i, _)| &args[i + 1])
            .collect();
        assert_eq!(
            briefs.len(),
            1,
            "one flag, whatever cide folds into it: {args:?}"
        );
        assert!(briefs[0].contains("You are the developer agent."));
        assert!(
            briefs[0].contains("mcp__cide__cide_task_get"),
            "the tracker paragraph names the tools in this CLI's spelling: {}",
            briefs[0]
        );

        assert_eq!(
            args[at(args, "--json-file") + 1],
            "/run/user/1000/cide-run-7.events"
        );
        let mcp: serde_json::Value =
            serde_json::from_str(&args[at(args, "--mcp-config") + 1]).expect("inline json");
        assert_eq!(mcp["mcpServers"]["cide"]["command"], "/opt/cide/cide-hook");
        assert_eq!(mcp["mcpServers"]["cide"]["args"][0], "mcp");

        // Variadic, so it is the last flag and its tokens are the last tokens.
        let allow = at(args, "--allowed-tools");
        assert_eq!(
            &args[allow + 1..],
            &["mcp__cide__cide_task_get".to_string()]
        );

        // No approval mode when neither the role nor the project asked for one: the CLI asks.
        assert!(!args.iter().any(|a| a == "--approval-mode"), "{args:?}");

        assert_eq!(
            env_value(&spawn.spec, "CIDE_RUN"),
            Some(plan.run.to_string()).as_deref()
        );
        assert_eq!(
            env_value(&spawn.spec, "CIDE_AGENT_SOCK"),
            Some("/run/user/1000/cide-agents-42.sock")
        );
        assert!(
            env_value(&spawn.spec, "CIDE_SESSION").is_none(),
            "nothing here echoes it"
        );
        assert!(
            spawn
                .spec
                .env_remove
                .iter()
                .any(|name| name == "QWEN_CODE_IDE_SERVER_PORT"),
            "the IDE integration is switched off for a run: {:?}",
            spawn.spec.env_remove
        );
        assert!(spawn.opening.is_none(), "the prompt went in the argv");
        assert_eq!(spawn.binding, SessionBinding::Caller);
        assert_eq!(spawn.events.as_deref(), plan.events_path.as_deref());
    }

    /// A continuation after a restart is the same uuid behind `--resume`, with the
    /// continuation prompt as `-i`'s value; a role's approval mode maps or refuses; the
    /// project default fills in only when the role wrote nothing.
    #[test]
    fn the_definitions_vocabulary_maps_to_approval_modes_or_refuses() {
        let agent = role();
        let session = SessionId::new();
        let mut plan = plan_for(&agent, session);
        plan.prompt = "cide restarted while you were working. Continue.".into();
        let spawn = QwenHarness
            .respawn_spec(&plan, &session.to_string())
            .expect("continuable");
        let args = &spawn.spec.args;
        assert_eq!(args[at(args, "--resume") + 1], session.to_string());
        assert!(!args.iter().any(|a| a == "--session-id"));
        assert_eq!(args[at(args, "-i") + 1], plan.prompt);

        plan.unattended = Unattended::Bypass;
        let args = QwenHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        assert_eq!(args[at(&args, "--approval-mode") + 1], "yolo");

        // The project's `auto` is qwen's `auto`, not a quiet `yolo`.
        plan.unattended = Unattended::Auto;
        let args = QwenHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        assert_eq!(args[at(&args, "--approval-mode") + 1], "auto");

        let mut accepting = role();
        accepting.permission_mode = Some("acceptEdits".into());
        let plan = plan_for(&accepting, session);
        let args = QwenHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        assert_eq!(args[at(&args, "--approval-mode") + 1], "auto-edit");

        let mut silent = role();
        silent.permission_mode = Some("dontAsk".into());
        let refused = QwenHarness
            .spawn_spec(&plan_for(&silent, session))
            .expect_err("no rung for a silent deny");
        assert!(
            matches!(refused, HarnessError::NoEquivalent { .. }),
            "{refused}"
        );
        assert!(refused.to_string().contains("dontAsk"), "{refused}");

        let mut effortful = role();
        effortful.effort = Some("high".into());
        let refused = QwenHarness
            .spawn_spec(&plan_for(&effortful, session))
            .expect_err("no effort flag");
        assert!(refused.to_string().contains("effort"), "{refused}");

        // And the wrong harness's role is the wrong harness's.
        let mut claude = role();
        claude.def.harness = cide_ipc::Harness::Claude;
        assert!(matches!(
            QwenHarness.spawn_spec(&plan_for(&claude, session)),
            Err(HarnessError::WrongHarness { .. })
        ));
    }

    // Lines a real 0.23.0 wrote to `--json-file` in a pty (trimmed), plus the two shapes the
    // bundle emits that the morning's dead endpoint never produced: `result` and the
    // permission `control_request`.
    const SESSION_START: &str = r#"{"type":"system","subtype":"session_start","uuid":"be795a89-89a2-4c43-beb1-51d4597dd72c","session_id":"df2b7670-74eb-46ff-a203-8cdeb0a8fea0","parent_tool_use_id":null,"data":{"session_id":"df2b7670-74eb-46ff-a203-8cdeb0a8fea0","cwd":"/p","protocol_version":2,"version":"0.23.0","supported_events":["system","user","assistant","stream_event","result","control_request","control_response"]}}"#;
    const USER: &str = r#"{"type":"user","uuid":"8cc03f13-f0c7-484d-9c30-e90838a63005","session_id":"df2b7670-74eb-46ff-a203-8cdeb0a8fea0","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"text","text":"Reply with exactly the single word: pong"}]}}"#;
    const STREAM: &str = r#"{"type":"stream_event","uuid":"70f3b7be-1499-4a66-8aab-bde26dc31c6e","session_id":"df2b7670-74eb-46ff-a203-8cdeb0a8fea0","parent_tool_use_id":null,"event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}"#;
    const ASSISTANT: &str = r#"{"type":"assistant","uuid":"5c4d4fe2-5dbc-4ff6-b150-56c6679e1327","session_id":"df2b7670-74eb-46ff-a203-8cdeb0a8fea0","parent_tool_use_id":null,"message":{"id":"5c4d4fe2-5dbc-4ff6-b150-56c6679e1327","type":"message","role":"assistant","model":"coder-model","content":[{"type":"text","text":"pong"}]}}"#;
    const RESULT: &str = r#"{"type":"result","subtype":"success","uuid":"0b1e3a5c-0000-4000-8000-000000000001","session_id":"df2b7670-74eb-46ff-a203-8cdeb0a8fea0","is_error":false,"duration_ms":1234,"duration_api_ms":1200,"num_turns":1,"usage":{"input_tokens":10,"output_tokens":1}}"#;
    const CAN_USE_TOOL: &str = r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","tool_use_id":"call-1","input":{"command":"cargo test"},"permission_suggestions":null,"blocked_path":null}}"#;

    /// The state machine over the stream: every event says the child is working, `result`
    /// hands the turn back, the permission request parks it, the exit ends it from any state,
    /// and nothing moves a paused or ended run.
    #[test]
    fn the_stream_moves_the_run_and_only_the_exit_ends_it() {
        let h = QwenHarness;
        let step = |from: RunState, line: &str| h.observe(from, Observation::Line(line));

        assert_eq!(
            step(RunState::Starting, SESSION_START),
            Some(RunState::Running)
        );
        assert_eq!(step(RunState::Running, USER), None, "already running");
        assert_eq!(step(RunState::Running, STREAM), None);
        assert_eq!(step(RunState::Running, RESULT), Some(RunState::Idle));
        assert_eq!(step(RunState::Idle, ASSISTANT), Some(RunState::Running));
        assert_eq!(
            step(RunState::Running, CAN_USE_TOOL),
            Some(RunState::AwaitingPermission)
        );
        assert_eq!(
            step(RunState::AwaitingPermission, USER),
            Some(RunState::Running)
        );
        assert_eq!(
            step(RunState::Running, "! a line that is not an event"),
            None
        );
        assert_eq!(
            step(
                RunState::Running,
                r#"{"type":"control_response","response":{}}"#
            ),
            None,
            "an event this build does not read says nothing about the run"
        );
        assert_eq!(
            step(RunState::Paused { since_unix_ms: 1 }, RESULT),
            None,
            "a frame in flight must not thaw a frozen run"
        );
        assert_eq!(
            step(RunState::Finished { code: 0 }, ASSISTANT),
            None,
            "a late line never resurrects a finished run"
        );
        assert_eq!(
            h.observe(RunState::Idle, Observation::Exit(0)),
            Some(RunState::Finished { code: 0 })
        );
        assert_eq!(
            h.observe(RunState::Finished { code: 0 }, Observation::Exit(0)),
            None
        );
    }

    // A real 0.24.4 stream (selfcraft run 2ef83c0b, text and inputs blanked): one response
    // per `message_start … message_stop`, an `assistant` line per block type inside it.
    const MSG_START: &str = r#"{"type":"stream_event","uuid":"ad1e7773-bcc4-4e25-83e2-a5fb9de6a6b5","session_id":"9b56ae96-14d7-4843-b485-0ed44aa2efcb","parent_tool_use_id":null,"event":{"type":"message_start","message":{"id":"a79cea68-2cf3-4f74-9628-5b5774d50478","role":"assistant","model":"qwen-36-27b-fp8","content":[]}}}"#;
    const THINKING: &str = r#"{"type":"assistant","uuid":"a79cea68-2cf3-4f74-9628-5b5774d50478","session_id":"9b56ae96-14d7-4843-b485-0ed44aa2efcb","parent_tool_use_id":null,"message":{"id":"a79cea68-2cf3-4f74-9628-5b5774d50478","type":"message","role":"assistant","model":"qwen-36-27b-fp8","content":[{"type":"thinking","thinking":"x","signature":"x"}],"stop_reason":null,"usage":{"input_tokens":0,"output_tokens":0}}}"#;
    const TEXT: &str = r#"{"type":"assistant","uuid":"0c41195f-871e-49ef-a357-8dd630c90970","session_id":"9b56ae96-14d7-4843-b485-0ed44aa2efcb","parent_tool_use_id":null,"message":{"id":"0c41195f-871e-49ef-a357-8dd630c90970","type":"message","role":"assistant","model":"qwen-36-27b-fp8","content":[{"type":"text","text":"x"}],"stop_reason":null,"usage":{"input_tokens":0,"output_tokens":0}}}"#;
    const TOOL_USE: &str = r#"{"type":"assistant","uuid":"dfe3e9fb-e42a-4192-9cb7-5184b4607755","session_id":"9b56ae96-14d7-4843-b485-0ed44aa2efcb","parent_tool_use_id":null,"message":{"id":"dfe3e9fb-e42a-4192-9cb7-5184b4607755","type":"message","role":"assistant","model":"qwen-36-27b-fp8","content":[{"type":"tool_use","id":"call_d9aa212679744f77b6138611","name":"tool_search","input":{}}],"stop_reason":"tool_use","usage":{"input_tokens":24553,"output_tokens":114,"cache_read_input_tokens":0,"total_tokens":24667}}}"#;
    const MSG_STOP: &str = r#"{"type":"stream_event","uuid":"e6cb30f9-31bc-42c5-9282-c55680b55c87","session_id":"9b56ae96-14d7-4843-b485-0ed44aa2efcb","parent_tool_use_id":null,"event":{"type":"message_stop"}}"#;

    /// A response that called tools is not the end of the turn; the next one, which did not,
    /// is — and the line handed back is one `observe` reads as `Idle`. A subagent's responses
    /// say nothing about the parent's turn.
    #[test]
    fn a_turn_ends_at_the_first_response_that_called_no_tools() {
        let mut t = TurnTracker::default();
        for line in [SESSION_START, USER, MSG_START, THINKING, TEXT, TOOL_USE] {
            assert_eq!(t.feed(line), None, "{line}");
        }
        assert_eq!(t.feed(MSG_STOP), None, "tools are about to run");
        assert_eq!(t.feed(USER), None);

        let sub = |line: &str| {
            line.replace(
                r#""parent_tool_use_id":null"#,
                r#""parent_tool_use_id":"call-9""#,
            )
        };
        assert_eq!(t.feed(MSG_START), None);
        assert_eq!(t.feed(&sub(TOOL_USE)), None);
        assert_eq!(
            t.feed(&sub(MSG_STOP)),
            None,
            "a subagent's response ends nothing"
        );
        assert_eq!(t.feed(THINKING), None);
        assert_eq!(t.feed(TEXT), None);
        assert_eq!(
            t.feed(MSG_STOP),
            Some(TURN_OVER),
            "the subagent's tool call did not taint the parent"
        );

        // A follow-up turn (a typed line, or a background job's notification) ends the same way.
        assert_eq!(t.feed(USER), None);
        assert_eq!(t.feed(MSG_START), None);
        assert_eq!(t.feed(TEXT), None);
        assert_eq!(t.feed(MSG_STOP), Some(TURN_OVER));
        assert_eq!(t.feed("not json"), None);

        assert_eq!(
            QwenHarness.observe(RunState::Running, Observation::Line(TURN_OVER)),
            Some(RunState::Idle)
        );
    }

    /// The real harness on an ended run's conversation is the TUI on `--resume`, and the id is
    /// a uuid or it is not a conversation.
    #[test]
    fn continuing_a_conversation_resumes_the_tui_on_its_uuid() {
        let id = SessionId::new();
        let spec = QwenHarness
            .continue_spec(&HarnessSession {
                harness: cide_ipc::Harness::Qwen,
                id: id.to_string(),
                cwd: PathBuf::from("/p/.cide/worktrees/developer-t-14"),
            })
            .expect("a uuid is a conversation");
        assert_eq!(spec.program, "qwen");
        assert_eq!(spec.args, vec!["--resume".to_string(), id.to_string()]);
        assert!(
            spec.resume.is_none(),
            "the id rides in the args, not through claude's road"
        );

        let refused = QwenHarness
            .continue_spec(&HarnessSession {
                harness: cide_ipc::Harness::Qwen,
                id: "ses_not_a_uuid".into(),
                cwd: PathBuf::from("/p"),
            })
            .expect_err("not a uuid");
        assert!(matches!(refused, HarnessError::NotAConversation { .. }));

        assert_eq!(
            QwenHarness.tool_name("cide_task_get"),
            "mcp__cide__cide_task_get"
        );
    }
}
