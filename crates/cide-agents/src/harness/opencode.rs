//! Running a role on the opencode CLI. (M18)
//!
//! Measured against opencode **1.18** on a Linux machine, not remembered. Where a paragraph below
//! says *measured*, it means a command was run or the shipped binary was read; the two facts that
//! decided the whole shape of this file — that a whole configuration can be handed over in one
//! environment variable, and that the inline agent's `prompt` actually reaches the model — were
//! each probed twice, because the first probe of the second one produced a confident false
//! negative.
//!
//! # A run is one process per turn, and that is what [`Delivery::Respawn`] is
//!
//! `opencode run` reads one message, answers it, and exits: its event loop **breaks the moment
//! the session reports idle**, which is visible in the shipped binary and is why there is no
//! second prompt to write. `ClaudeHarness` hosts an interactive TUI and types a follow-up into
//! it; there is nothing here to type into. So a follow-up is a *new child* carrying
//! `--session <id>`, which continues the same conversation server-side — the whole reason
//! [`Delivery`] is an enum rather than a `write(bytes)` method the second implementation would
//! have had to lie to.
//!
//! [`Harness::respawn_spec`] is the other half of that answer: it builds the continuing child,
//! and it is defaulted-and-refusing on the trait precisely so a `Stdin` harness never has to
//! implement a method it would have nothing truthful to put in.
//!
//! # The configuration travels in the environment, and nothing is written into the project
//!
//! **`OPENCODE_CONFIG_CONTENT` carries a whole configuration document.** Measured: an inline
//! `agent` definition supplied only through that variable is registered — `opencode agent list`
//! run with it prints the role — and its `prompt` reaches the model, verified by putting facts in
//! it the model could not otherwise know and asking a neutral question about them. That is the
//! seam this file is built on, and it buys three things at once:
//!
//! * **No file in the user's project.** A run's cwd is a git worktree cide created and will
//!   eventually prune; writing an `opencode.json` into it would be cide editing a checkout on the
//!   user's behalf, and killing cide would leave it there.
//! * **A per-role system prompt**, which opencode has no flag for. There is no
//!   `--append-system-prompt` here, so the role's prompt is *configuration* rather than an
//!   argument — and emphatically not the first line of the message, where a model reads it as the
//!   user's own words rather than as its brief. [`super::TRACKER_PREAMBLE`] is appended to it,
//!   after the role's own words and only when the `mcp` block below is written, so a role reads
//!   the same two paragraphs here that `claude.rs` folds into `--append-system-prompt`.
//! * **cide's MCP server**, in the same document, so an opencode run reaches the task tracker
//!   exactly as a claude run does.
//!
//! Read last of all the config sources — measured in the binary, which merges the global file,
//! then the project's, then this — so it wins where it says anything and leaves everything else
//! alone.
//!
//! ## `OPENCODE_DISABLE_PROJECT_CONFIG` is deliberately **not** set
//!
//! It is the tempting wrong move, so it is named here to stop somebody "fixing" the merge with
//! it. A project's own `opencode.json` is **the user's configuration** — their models, their MCP
//! servers, their instructions — and cide switching it off would mean a run behaving unlike every
//! other opencode invocation in that repository, for reasons no file anywhere records. cide adds
//! a role and a server; it does not take the project's own configuration away.
//!
//! ## What the document does *not* carry, and why each absence is deliberate
//!
//! * **`tools`** — deprecated in the schema in favour of `permission`, so emitting it writes a
//!   key the next release may stop reading. A definition's `tools:` line is `--allowedTools`
//!   vocabulary, which is the *claude* CLI's; there is no mapping from it to opencode's
//!   permission rules that cide could write without inventing one, and a permission rule the
//!   role's author never wrote is the one kind of restriction that must not be invented.
//! * **`permission`** — the same argument. `LoadedAgent::permission_mode` is validated at load
//!   against the literal set in `claude --help`; translating `acceptEdits` into a permission
//!   table here would be cide making up a policy and attributing it to a file.
//! * **`variant`** — carried as the `--variant` flag instead, beside `--model`, because the
//!   schema says the config field applies *only when the agent's configured model is the one in
//!   use* and this run may name another with `--model`.
//!
//! `mode: "primary"` **is** emitted, and it is load-bearing rather than decorative: the shipped
//! binary refuses to select a `subagent`-mode agent from `--agent` and falls back to the default
//! agent with a warning, which would silently run somebody else's prompt.
//!
//! # The argv, and the wager this file does not have to take
//!
//! `opencode run --agent <role> [--model <provider/model>] [--variant <effort>] --format json
//! --dir <cwd> [--session <id> | --title <label>] <message>`
//!
//! `ClaudeHarness`'s argv order is load-bearing because a *user's* variadic flag can swallow the
//! tokens cide adds. **There is no user argument list on this side**: `ClaudeSettings::cli` is the
//! Claude pane's launch configuration and has nothing to do with this binary, so cide owns the
//! whole vector. The day an opencode launch-args setting exists, the user's tokens go first, for
//! the reason `claude.rs` gives at length.
//!
//! Two things about it are still worth knowing:
//!
//! * **The message is a positional argument**, unlike a claude run's, whose prompt is typed into
//!   the terminal. So [`HarnessSpawn::opening`] is `None` here, and the PTY-write rule that a
//!   prompt may not contain a newline — an embedded `\n` is another Enter — does not apply: an
//!   argv token may contain anything. It is still written as one line by the dispatch site, and
//!   that is worth keeping, because the *parser* has its own hazard: the message goes last, and a
//!   message beginning with `-` would be read as a flag by the CLI's option parser.
//! * **`--dir` and the spawn's cwd are both set, to the same directory.** The cwd is what the
//!   child and everything it starts inherit; `--dir` is what opencode resolves the project and
//!   the session store from. Setting only one of them gives a run whose tools and whose conversation
//!   disagree about where the work is.
//!
//! # The environment, and the variable that is deliberately absent
//!
//! | name | value | why |
//! | --- | --- | --- |
//! | `TERM`, `COLORTERM`, `TERM_PROGRAM`… | [`cide_core::child_env::terminal_child_env`] | the identical list a pane and a claude run get; one composition, not three |
//! | proxy variables | [`RunPlan::proxy`], resolved by the caller | one resolved answer per child, `cide-core`'s rule |
//! | `OPENCODE_CONFIG_CONTENT` | the role and cide's MCP server | see above |
//! | `CIDE_RUN` | the run id | what `cide-hook mcp`'s header line names, and what scopes the connection to this run's task tools |
//! | `CIDE_AGENT_SOCK` | the agent-RPC socket | what that bridge connects to |
//! | `CIDE_SESSION` | **not set** | see below |
//! | `CIDE_HOOK_SOCK` | **not set** | nothing here writes a hook frame |
//!
//! The `CLAUDE_CODE_*` switches in that list ride along and are **inert** here, exactly as they
//! are in a shell pane — `terminal_child_env`'s own header argues that case at length, and the
//! alternative (a second, harness-aware composition of the terminal environment) would be two
//! lists to keep in step so that one child could avoid four names it ignores.
//!
//! `terminal_child_env` is handed an **empty** `extra` list. That list is
//! `claude_cli::user_env` — the variables from the user's *Claude sessions* launch configuration
//! — and that function's own header records the rule: the four inert `CLAUDE_CODE_*` names may
//! reach any child, but an arbitrary `NODE_OPTIONS` or `GIT_SSH_COMMAND` from a field labelled
//! *the environment claude panes are spawned with* may not quietly become the environment of a
//! program that is not claude.
//!
//! **`CIDE_SESSION` buys nothing here, so it is not set.** For a claude run it is the routing key
//! that makes liveness free: `cide-hook` reads it out of the child's environment and echoes it
//! back on every one of ten hook points, so the state machine, the phase dot and the live cost
//! figure all arrive without cide watching anything. opencode has no hooks, no `--settings`, and
//! nothing that would read the variable — setting it would be a name in an environment that
//! nobody ever looks at, which is worse than nothing because the next reader would take it for a
//! working channel.
//!
//! ## So liveness comes from the child's own output, and from nowhere else
//!
//! `--format json` makes stdout **newline-delimited JSON, one object per line**, and
//! [`Harness::observe`] over [`Observation::Line`] is the *only* thing that moves an opencode
//! run's row. Two consequences worth stating rather than discovering:
//!
//! * A caller that does not feed the stream in leaves every opencode run sitting at
//!   [`RunState::Starting`] until its child exits. For a claude run the equivalent lapse is
//!   invisible, because hooks arrive anyway.
//! * `sessionID` is on **every** line including the first (measured), which is what makes
//!   [`SessionBinding::Harness`] a one-line scan rather than the three-step ladder the design
//!   sketched — scan for an id, else reconcile through `--title` and `opencode session list`,
//!   else give up and call a run fire-and-forget. None of that is needed and none of it is here.
//!
//! # Permissions, and why a run never sits waiting on one
//!
//! Measured in the shipped binary: `run` answers a permission request by **rejecting it** and
//! printing one line, unless `--auto` was passed. So there is no `AwaitingPermission` for this
//! harness and no wedged turn either.
//!
//! `--auto` was deliberately not passed at first — its own `--help` calls it dangerous, and the
//! theory was that opencode's default rules allow ordinary edits and commands, so the flag would
//! buy nothing except the ability to say yes to questions worth asking. **Live runs disproved
//! the theory's second half**: the first command that strayed past the project directory
//! (`cat ~/.cargo/config.toml`, on the way to a `cargo check`) was auto-rejected and the turn
//! **ended at the rejection** — two real runs died mid-investigation, exit 0, task never
//! reported, which reads on the board as an agent that did nothing. A refusal that ends the
//! turn is not a safety property in a headless child; it is the autonomy failing silently. So
//! `--auto` now rides [`RunPlan::skip_permissions`] — the project-level
//! `agents.skipPermissions`, on by default, off in one config line — and the containment for an
//! unattended child is what it always actually was: worktree isolation.
//!
//! # Opening a run into a pane shows the events **rendered**, and the raw line is still the channel
//!
//! `--format json` is what makes the run legible to cide, and it used to be what made the pane
//! show raw ndjson to a person — a cost this header once accepted, and the first live run
//! reported as unreadable. [`render_event`] is the reconciliation, and *where* it runs is the
//! design: `cide_pty::LineRender` rewrites the stream once, upstream of the mirror and every
//! sink, while the app's observer reads each **raw** line inside the same hook before the
//! rendering is returned. One rendered stream downstream (a rehydrated pane, a choked sink's
//! catch-up and a live pane cannot disagree), one raw channel upstream for `capture` and
//! [`Harness::observe`]. `opencode serve` plus `attach` remains the named upgrade path to a
//! *live* TUI in the pane; this makes the transcript readable without it.

use cide_ipc::RunState;
use cide_pty::{Geometry as PtyGeometry, Rendered, SpawnSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    ADHOC_PREAMBLE, Delivery, Harness, HarnessError, HarnessSpawn, Observation, RunPlan, SERVER,
    SessionBinding, tracker_preamble,
};

/// The document's own contract, so a person reading a dumped configuration can look it up.
const SCHEMA: &str = "https://opencode.ai/config.json";

/// The opencode CLI as a harness. A unit struct: it holds nothing, and must not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpencodeHarness;

impl Harness for OpencodeHarness {
    fn kind(&self) -> cide_ipc::Harness {
        cide_ipc::Harness::Opencode
    }

    /// `<server>_<tool>`, so cide's vocabulary arrives as `cide_cide_task_get`.
    ///
    /// The doubled `cide` looks like a mistake and is not: the server is [`SERVER`] and the tool
    /// is already `cide_task_get`, and opencode joins the two with one underscore. Renaming the
    /// server to flatten it would break the rule [`SERVER`] exists for — one server name across
    /// harnesses, so a role's tool list need not know which CLI is carrying it.
    fn tool_name(&self, tool: &str) -> String {
        format!("{SERVER}_{tool}")
    }

    fn spawn_spec(&self, plan: &RunPlan<'_>) -> Result<HarnessSpawn, HarnessError> {
        child(plan, None)
    }

    fn respawn_spec(
        &self,
        plan: &RunPlan<'_>,
        session: &str,
    ) -> Result<HarnessSpawn, HarnessError> {
        child(plan, Some(session))
    }

    /// Always [`Delivery::Respawn`]. See the module header: `run` is one turn per process, and
    /// the child that answered the last one has already exited.
    fn deliver(&self, _text: &str) -> Delivery {
        Delivery::Respawn
    }

    /// The whole state machine for this harness, because the output stream is the whole channel.
    ///
    /// | observation | answer | why |
    /// | --- | --- | --- |
    /// | `Exit(code)` | `Finished { code }` | the only ground truth about the child, so it answers from any state |
    /// | `Line` — `step_start`, `step_finish`, `text`, `tool_use`, `reasoning` | `Running` | the turn is producing something |
    /// | `Line` — anything else, or not JSON at all | `None` | says nothing about the run |
    /// | `Hook` | `None` | nothing installs a hook into opencode |
    ///
    /// # No line ends the turn — the exit does, and run 06202dd6 is why
    ///
    /// This mapping used to read `step_finish` in two halves: `reason: "tool-calls"` was an
    /// intermediate step (`Running` — a turn that calls tools is several model round trips, and
    /// the intermediate ones finish with that value, the AI SDK's own), any other reason was the
    /// turn handed back (`Idle`). The half-truth in it: a turn's *final* step really does finish
    /// with a non-`tool-calls` reason. What it missed is that a final-looking reason is not
    /// proof the *turn* is over — the stream carries `step_finish` events from **nested
    /// sessions** too (a role that reaches for opencode's own sub-agents sees their final steps
    /// on the same stdout, under their own `sessionID`s), and the SDK has finish reasons
    /// (`length`, `error`, `unknown`) that end a step with the child nowhere near done.
    ///
    /// Believing one was not a display bug. `Idle` releases the run's concurrency slot, and
    /// under worktree isolation the next queued run of the same role is then admitted — whose
    /// `bring_up` **winds down the idle child to reclaim the checkout**. Run 06202dd6 (a `qa`
    /// role, sixteen minutes into a live smoke test) died exactly this way: a queued dispatch
    /// for the same role had been waiting thirteen minutes, a mid-turn `step_finish` read as
    /// `Idle`, and the wind-down killed a working child — logged by the orchestrator as
    /// "dispatch-to-busy-role appears to preempt the active run", which is precisely what the
    /// queue exists never to do. (The observed exit was 143: the CLI traps the wind-down's
    /// signal, shuts its own process tree down — the cleanup was noted as flawless — and dies
    /// as if TERMed. Good manners, wrong death.)
    ///
    /// So no line answers `Idle` any more, and the asymmetry that already stood in this comment
    /// is the whole justification: being *late* to call the turn over costs almost nothing,
    /// because `run` is one turn per process and the child exits within milliseconds of its
    /// last step — `Observation::Exit` follows with the truth, releases the slot then, and the
    /// queued run starts into a checkout whose previous occupant is genuinely gone. Being
    /// *early* costs a working agent its life. An opencode run therefore never holds
    /// [`RunState::Idle`]: its turn ends as `Finished { code }`, which nudges the orchestrator
    /// like every other ending, and the idle wind-down is in practice a claude-only event —
    /// a parked interactive `claude` being the one idle child that never leaves by itself.
    ///
    /// # Why every event answers `Running` rather than `None`
    ///
    /// Because for this harness they are the only evidence that the child is doing anything at
    /// all. A run stays [`RunState::Starting`] from the fork until something says otherwise, and
    /// "otherwise" is a line. Answering `None` to `text` would mean a run whose first `step_start`
    /// was lost — a truncated line, a stream cide attached to a moment late — never leaves
    /// `Starting` again. Answering `Running` from any event costs nothing when the run is already
    /// running, because [`Harness::observe`]'s contract is that a transition to the state a run
    /// already holds is `None`.
    ///
    /// `error` events deliberately do **not** map to [`RunState::Failed`]. The process exits
    /// immediately after one, so `Observation::Exit` would overwrite the sentence within
    /// milliseconds with `Finished { code }` — the truthful record, since there *is* an exit
    /// status — and the error's own text is in the run's transcript, where opening the run into a
    /// pane shows it.
    fn observe(&self, current: RunState, ob: Observation<'_>) -> Option<RunState> {
        match ob {
            // The only observation carrying ground truth about the child, so it answers from any
            // state — including `Paused`, whose frozen child may have been killed under the
            // freeze, and `Idle`, which is alive by definition and which nothing else ends.
            Observation::Exit(code) => {
                let next = RunState::Finished { code };
                (next != current).then_some(next)
            }
            // No hook cide can install reaches this CLI. Answered explicitly rather than by a
            // catch-all so that a frame routed here by mistake is a silent no-op rather than a
            // claude state machine applied to an opencode run.
            Observation::Hook(_) => None,
            Observation::Line(line) => {
                if absorbs(&current) {
                    return None;
                }
                let event = Event::parse(line)?;
                let next = match event.kind.as_str() {
                    "step_start" | "step_finish" | "text" | "tool_use" | "reasoning" => {
                        RunState::Running
                    }
                    _ => return None,
                };
                (next != current).then_some(next)
            }
        }
    }
}

/// The states no output line may move, which are [`Harness::observe`]'s two documented refusals.
///
/// * **`Paused`** — the child is `SIGSTOP`ped. A line that was already in the coalescer when the
///   signal landed must not thaw the row; only a resume clears a freeze cide asserted.
/// * **`Finished` / `Failed`** — terminal. The last few lines of a dying child's output are read
///   after its exit has been reaped, so without this a finished run goes back to `Running` and
///   stays there for ever.
///
/// `Idle` is emphatically not one of them — a rule that now outlives its only producer: since
/// the 06202dd6 fix no line maps to `Idle`, so this harness's runs never hold it. It stays
/// un-absorbed anyway, because the rule is about recovery, not production: if a run ever *is*
/// `Idle` (a hand-set state in a test, a variant a future slice produces), a line from its child
/// must still be able to move it back to `Running` rather than freeze it there.
fn absorbs(current: &RunState) -> bool {
    matches!(
        current,
        RunState::Paused { .. } | RunState::Finished { .. } | RunState::Failed { .. }
    )
}

/// One event line, in the two fields anything here reads.
///
/// Measured shape, from the shipped binary's own emitter:
/// `{"type":<name>,"timestamp":<ms>,"sessionID":<id>, ...}` — the id is spread onto **every**
/// line, including the first, which is the whole of [`capture`]. A `step_finish`'s finish
/// `reason` is deliberately **not** here any more: it used to route the event to
/// [`RunState::Idle`], which is the misreading that killed run 06202dd6 — see
/// [`OpencodeHarness::observe`]. [`render_event`] still shows the reason to a person, from the
/// raw JSON it reads for display.
#[derive(Debug, Deserialize)]
struct Event {
    /// `step_start`, `step_finish`, `text`, `tool_use`, `reasoning`, `error`.
    #[serde(rename = "type")]
    kind: String,
    /// opencode's own session id, `ses_…`. Not cide's [`cide_ipc::SessionId`].
    #[serde(rename = "sessionID")]
    session: Option<String>,
}

impl Event {
    /// One line, or `None` when it is not one of ours.
    ///
    /// A run's stdout is **not** all JSON: the CLI prints a plain warning line when it rejects a
    /// permission, and another if an agent name does not resolve — with ANSI styling, because it
    /// is talking to a terminal. Those must fall through rather than raising anything.
    ///
    /// `trim` because this is a PTY: the line discipline turns every `\n` into `\r\n`, so a line
    /// split on `\n` still ends in a carriage return.
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if !line.starts_with('{') {
            return None;
        }
        serde_json::from_str(line).ok()
    }
}

/// The session id this line names, for [`SessionBinding::Harness`].
///
/// Answers from the **first** line a run prints, and from every line after it, so a run is
/// resumable from its first event onward rather than only once it has said something.
///
/// Both refusals are ordinary rather than exceptional: a line that is not JSON is a warning the
/// CLI printed, and a JSON line with no `sessionID` would be a shape this release does not have —
/// neither is worth an error, and either would be worth one if this returned a `Result`.
fn capture(line: &str) -> Option<String> {
    Event::parse(line)?.session.filter(|id| !id.is_empty())
}

// ==========================================================================================
// Rendering the event stream for a person.
// ==========================================================================================

/// How much of a tool's output the pane shows before eliding. Characters, then lines.
const OUTPUT_CHAR_BUDGET: usize = 700;
const OUTPUT_LINE_BUDGET: usize = 6;

/// How much of a reasoning fragment survives — it is texture, not record.
const REASONING_BUDGET: usize = 240;

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// One event line, as a person should read it — [`SessionBinding::Harness`]'s `render`.
///
/// The stream this rewrites is `--format json`, which is the *channel* (see the module header)
/// and which used to reach the pane raw: opening a working run showed ndjson, one event per
/// line, with a whole file read arriving as a single JSON document. Reported, verbatim:
/// *"it opens strange console with json output that isn't understandable"*. The events carry
/// everything a readable transcript needs — tool, input, output, the model's words, tokens —
/// so this renders them and drops nothing a person acts on:
///
/// * a tool call is `● name title`, its output clipped to a budget (the full text is one
///   `cide_task_get`/transcript away; the pane is a progress view, not an archive);
/// * a rejected or failed call is red, with the error verbatim — that line *is* the answer to
///   "why did this run die";
/// * the model's own text passes whole, reasoning passes dimmed and clipped;
/// * `step_start` draws nothing, `step_finish` is one dim token count;
/// * an event type this build has never heard of becomes a dim one-word marker rather than a
///   screenful of JSON, and **a line that is not JSON is kept verbatim** — that is the CLI's own
///   prose (a warning, a rejected permission) and hiding it would hide the failure.
///
/// Styling is bare SGR (dim/bold/cyan/red), which every theme already maps; no colour is load-
/// bearing.
pub fn render_event(line: &str) -> Rendered {
    let Ok(Value::Object(event)) = serde_json::from_str::<Value>(line) else {
        return Rendered::Keep;
    };
    let Some(kind) = event.get("type").and_then(Value::as_str) else {
        return Rendered::Keep;
    };
    let part = event.get("part").unwrap_or(&Value::Null);

    match kind {
        "step_start" => Rendered::Drop,
        "step_finish" => {
            let reason = part.get("reason").and_then(Value::as_str).unwrap_or("done");
            let total = part
                .pointer("/tokens/total")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Rendered::Replace(format!("{DIM}· {reason} · {} tok{RESET}", thousands(total)))
        }
        "text" => {
            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
            let text = text.trim_end();
            if text.is_empty() {
                return Rendered::Drop;
            }
            Rendered::Replace(format!("{BOLD}{text}{RESET}"))
        }
        "reasoning" => {
            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
            let text = one_line_of(text);
            if text.is_empty() {
                return Rendered::Drop;
            }
            Rendered::Replace(format!("{DIM}∴ {}{RESET}", clip(&text, REASONING_BUDGET)))
        }
        "tool_use" => Rendered::Replace(render_tool(part)),
        "error" => Rendered::Replace(format!("{RED}✗ {}{RESET}", one_line_of(&part.to_string()))),
        other => Rendered::Replace(format!("{DIM}· {other}{RESET}")),
    }
}

/// A tool call: what ran, on what, and a clipped view of what came back.
fn render_tool(part: &Value) -> String {
    let tool = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
    let state = part.get("state").unwrap_or(&Value::Null);
    let status = state.get("status").and_then(Value::as_str).unwrap_or("");

    // The CLI's own `title` when it wrote one (for `bash` it is the command), else the input
    // document, compact — never the output, which gets its own budgeted block below.
    let title = match state.get("title").and_then(Value::as_str) {
        Some(title) if !title.trim().is_empty() => title.to_string(),
        _ => state.get("input").map(compact_input).unwrap_or_default(),
    };
    let title = clip(&one_line_of(&title), 160);

    if status == "error" {
        let error = state
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        return format!(
            "{RED}✗ {tool}{RESET} {title}\n{RED}  {}{RESET}",
            one_line_of(error)
        );
    }

    let mut out = format!("{CYAN}● {tool}{RESET} {title}");
    if let Some(output) = state.get("output").and_then(Value::as_str) {
        let preview = clip_block(output);
        if !preview.is_empty() {
            out.push_str(&format!("\n{DIM}{preview}{RESET}"));
        }
    }
    out
}

/// The input document with its top-level values flattened — `{"command":"ls"}` reads `ls`.
fn compact_input(input: &Value) -> String {
    match input {
        Value::Object(map) => map
            .values()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    }
}

/// At most [`OUTPUT_LINE_BUDGET`] lines and [`OUTPUT_CHAR_BUDGET`] characters, indented, with
/// an elision that says how much it hid.
fn clip_block(text: &str) -> String {
    let text = text.trim_end();
    if text.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut spent = 0;
    for (n, line) in lines.iter().enumerate() {
        if n >= OUTPUT_LINE_BUDGET || spent >= OUTPUT_CHAR_BUDGET {
            out.push(format!("  … (+{} more lines)", lines.len() - n));
            break;
        }
        let clipped = clip(line, OUTPUT_CHAR_BUDGET - spent);
        spent += clipped.chars().count();
        out.push(format!("  {clipped}"));
    }
    out.join("\n")
}

/// `chars`, not bytes: a byte slice of UTF-8 panics on a boundary, and this crate is linked
/// into a process built with `panic = "abort"`.
fn clip(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let kept: String = text.chars().take(budget).collect();
    format!("{}…", kept.trim_end())
}

/// Whitespace runs collapsed to one space — a reasoning fragment arrives with its own layout.
fn one_line_of(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `10454` reads `10.5k`; small counts stay exact.
fn thousands(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    format!("{:.1}k", n as f64 / 1_000.0)
}

/// The child, fresh (`resume: None`) or continuing a conversation the harness already minted.
///
/// One function for both, so the two children differ in exactly the tokens the difference is
/// about and cannot drift in the environment, the cwd or the configuration.
fn child(plan: &RunPlan<'_>, resume: Option<&str>) -> Result<HarnessSpawn, HarnessError> {
    if plan.agent.def.harness != cide_ipc::Harness::Opencode {
        return Err(HarnessError::WrongHarness {
            plan: plan.agent.def.harness,
            harness: cide_ipc::Harness::Opencode,
        });
    }
    // Refused before anything is built. `run` with an empty message is a process that starts, has
    // nothing to do and holds a concurrency slot and a worktree while it works that out.
    if plan.prompt.trim().is_empty() {
        return Err(HarnessError::NoPrompt);
    }
    // Refused rather than degraded, and this is the interesting refusal on this side: without the
    // document there is no role — `--agent <id>` would name an agent that does not exist, the CLI
    // would warn once and fall back to its own default agent, and the run would look like it
    // worked while carrying somebody else's system prompt.
    let config = config_json(plan).ok_or(HarnessError::NoConfig)?;

    // A bare name, resolved by the OS at this spawn rather than pinned at load: opencode updates
    // itself underneath a running app. `defs::harness_binary` is the one place that name lives,
    // and it is the same one `defs::installed` probed for the roster.
    let program = crate::defs::harness_binary(cide_ipc::Harness::Opencode);

    let mut args: Vec<String> = vec![
        "run".into(),
        "--agent".into(),
        plan.agent.def.id.to_string(),
    ];

    // Each of these only when the definition carried it. The CLI's defaults are the safe end of
    // both ranges, and a value invented here is a behaviour the role's author never wrote and
    // cannot find in their file.
    if let Some(model) = &plan.agent.def.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    // Carried verbatim and unvalidated, for `LoadedAgent::effort`'s stated reason: the set is
    // per-harness and per-release, and a bad value is the CLI's refusal to make, loudly.
    if let Some(effort) = &plan.agent.effort {
        args.push("--variant".into());
        args.push(effort.clone());
    }

    // The project's default for unattended children (`agents.skipPermissions`, on unless
    // switched off). For this harness the alternative is not a prompt — `run` auto-rejects and,
    // measured, the **turn ends at the rejection** — so the module header's old argument for
    // never passing `--auto` is rewritten there, beside the measurement that lost it.
    if plan.skip_permissions {
        args.push("--auto".into());
    }

    // Not a display preference. This is the channel — see the module header.
    args.push("--format".into());
    args.push("json".into());
    args.push("--dir".into());
    args.push(plan.cwd.to_string_lossy().to_string());

    match resume {
        // The harness's own id, captured off this run's output. Deliberately no `--title`: the
        // session already has one, and passing a second would rename a conversation mid-flight.
        Some(session) => {
            args.push("--session".into());
            args.push(session.to_string());
        }
        // What `opencode session list` and the terminal title show, built like a claude run's
        // `-n` from the title copied at dispatch — so it still reads correctly after the task has
        // been renamed.
        None => {
            args.push("--title".into());
            args.push(session_name(plan));
        }
    }

    // **Last, and nothing may be added after it.** The message is a positional argument; a token
    // written after it would be read as another word of the message.
    args.push(plan.prompt.clone());

    let mut spec = SpawnSpec::new(program, plan.cwd.clone()).geometry(PtyGeometry::new(
        plan.geometry.cols,
        plan.geometry.rows,
        plan.geometry.cell_width,
        plan.geometry.cell_height,
    ));
    for arg in args {
        spec = spec.arg(arg);
    }

    // The same list a pane and a claude run get, with an **empty** `extra` — see the module
    // header: the user's Claude launch variables are not this child's to inherit.
    spec = spec.apply(cide_core::child_env::terminal_child_env(
        &plan.claude,
        env!("CARGO_PKG_VERSION"),
        Vec::new(),
    ));
    spec = spec.apply(plan.proxy.changes().to_vec());
    spec = spec.env("OPENCODE_CONFIG_CONTENT", config);
    // What scopes this child's MCP connection to this run's task tools, and nothing else — the
    // app resolves the header's `run` against the registry rather than trusting anything the
    // child says about itself.
    spec = spec.env("CIDE_RUN", plan.run.to_string());
    if let Some(sock) = &plan.agent_sock {
        spec = spec.env("CIDE_AGENT_SOCK", sock.to_string_lossy().to_string());
    }

    Ok(HarnessSpawn {
        spec,
        // The prompt went in the argv, so there is nothing to type. See `HarnessSpawn::opening`.
        opening: None,
        binding: SessionBinding::Harness {
            capture,
            render: render_event,
        },
    })
}

/// The `--title`: the role, then the task it was dispatched for. `claude.rs`'s `-n`, verbatim.
fn session_name(plan: &RunPlan<'_>) -> String {
    match plan
        .task_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(title) => format!("{} · {}", plan.agent.def.label, title),
        None => plan.agent.def.label.clone(),
    }
}

/// The whole `OPENCODE_CONFIG_CONTENT` document: this role, and cide's MCP server.
///
/// # Built with serde, never `format!`
///
/// A system prompt is a multi-paragraph document a human wrote. It contains quotation marks, it
/// contains backslashes, and it contains newlines — and a `format!`-built document would turn any
/// of them into a configuration that parses as something *else*: a truncated prompt, a different
/// key, or a document the CLI rejects wholesale, in which case the run silently becomes opencode's
/// default agent. `claude.rs`'s `--mcp-config` builder makes the same argument for a worktree path
/// with an apostrophe in it; this one is worse, because the hostile input is the feature.
///
/// # The shape, checked against `https://opencode.ai/config.json`
///
/// `$defs.AgentConfig` accepts `model`, `variant`, `temperature`, `top_p`, `prompt`, `disable`,
/// `description`, `mode`, `hidden`, `steps`, `permission`, `options` and `color`; `tools` is
/// deprecated in favour of `permission`. Four are emitted and the module header argues each
/// absence.
///
/// `$defs.McpLocalConfig` requires `type: "local"` and a `command` **array** — program first, then
/// its arguments, unlike the `{command, args}` shape claude's `--mcp-config` takes. The path is
/// absolute for the reason `RunPlan::hook_bin` states: the child's cwd is a worktree and its
/// `PATH` is the user's, so a bare `cide-hook` resolves to nothing and the failure arrives as a
/// connection error from a process three levels below anything cide logs.
///
/// `None` is unreachable — `serde_json::to_string` of a `Value` whose keys are all strings cannot
/// fail — and is still an `Option` rather than an `unwrap`, because the caller turns it into a
/// refusal and a refused run is always better than a panicking one.
fn config_json(plan: &RunPlan<'_>) -> Option<String> {
    let def = &plan.agent.def;

    let mut role = serde_json::Map::new();
    role.insert("description".into(), json!(def.description));
    // Load-bearing: `--agent` refuses a `subagent`-mode agent and falls back to the default one.
    role.insert("mode".into(), json!("primary"));
    // The role's own brief **first and byte for byte**, then — only when the `mcp` block below is
    // actually written — cide's paragraph about the task tracker, separated by a blank line.
    //
    // The order is the claim: the role's file is what this run is for, and cide's housekeeping is
    // an addition to it rather than a preamble in front of it. The join is `\n\n`, which is the
    // join `cide_core::claude_cli::fold_append_system_prompt` makes on the claude side, so one
    // role pointed at either CLI reads the same two paragraphs in the same order.
    //
    // `plan.hook_bin` is the gate, and it is the same value the `mcp` block is written from a few
    // lines down — one `if let` there, one `is_some` here, and no way for a run to be told about
    // tools that were never attached. See `TRACKER_PREAMBLE`'s header.
    role.insert(
        "prompt".into(),
        json!(match plan.hook_bin.is_some() {
            true => {
                // Three paragraphs at most, in the same order and with the same `\n\n` join the
                // claude side's fold produces — so one role pointed at either CLI reads the same
                // brief. (M28 added the third.)
                let mut prompt = format!(
                    "{}\n\n{}",
                    def.system_prompt,
                    tracker_preamble(&OpencodeHarness)
                );
                if let Some(change) = plan.change.as_deref() {
                    prompt.push_str("\n\n");
                    prompt.push_str(&super::spec_preamble(
                        change,
                        plan.spec_cli.as_deref(),
                        plan.spec_apply.as_deref(),
                        &OpencodeHarness,
                    ));
                }
                // And, for a run with no task, the correction to the tracker paragraph — last,
                // exactly where the claude side folds it, so `harness.rs`'s parity test holds.
                // (M40)
                if plan.task.is_none() {
                    prompt.push_str("\n\n");
                    prompt.push_str(ADHOC_PREAMBLE);
                }
                prompt
            }
            false => def.system_prompt.clone(),
        }),
    );
    // Also on the command line as `--model`, which is what *this run* uses. Here as well so the
    // registered agent is self-consistent — `opencode agent list` and any other opencode process
    // that reads this definition see the model the role names rather than a provider default.
    if let Some(model) = &def.model {
        role.insert("model".into(), json!(model));
    }

    let mut agents = serde_json::Map::new();
    agents.insert(def.id.to_string(), Value::Object(role));

    let mut config = serde_json::Map::new();
    config.insert("$schema".into(), json!(SCHEMA));
    config.insert("agent".into(), Value::Object(agents));

    if let Some(hook) = &plan.hook_bin {
        let mut servers = serde_json::Map::new();
        servers.insert(
            SERVER.into(),
            json!({
                "type": "local",
                // `cide-hook mcp` is the bridge: stdio in, `$CIDE_AGENT_SOCK` out, and no
                // knowledge of the vocabulary at all. See its module doc.
                "command": [hook.to_string_lossy(), "mcp"],
                "enabled": true,
            }),
        );
        config.insert("mcp".into(), Value::Object(servers));
    }

    serde_json::to_string(&Value::Object(config)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tracker paragraph as *this* harness renders it — `cide_cide_task_get`, not
    /// `mcp__cide__cide_task_get`. Derived from the one definition, never quoted.
    fn tracker() -> String {
        tracker_preamble(&OpencodeHarness)
    }

    use crate::defs::LoadedAgent;
    use cide_claude::HookFrame;
    use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, TaskId, Theme};
    use std::path::PathBuf;

    /// Three lines captured from a real `opencode run --format json`, filled out to the shape the
    /// shipped emitter writes: `{type, timestamp, sessionID, ...part}`.
    ///
    /// Note the two spellings of one event, which is not a transcription error: the **envelope's**
    /// `type` is `step_start`, the **part's** is `step-start`. Anything reading the envelope must
    /// read the underscore form, and a fixture that quietly normalised them would let a bug
    /// through in exactly the field this file dispatches on.
    const STEP_START: &str = r#"{"type":"step_start","timestamp":1755402000000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_fe6da2c30ffe8Ab","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_fe6da2c2affe8Aa","type":"step-start"}}"#;
    const TEXT: &str = r#"{"type":"text","timestamp":1755402001000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_fe6da2c40ffe8Ac","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_fe6da2c2affe8Aa","type":"text","text":"middle","time":{"start":1755402000500,"end":1755402001000}}}"#;
    const STEP_FINISH: &str = r#"{"type":"step_finish","timestamp":1755402002000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_fe6da2c50ffe8Ad","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_fe6da2c2affe8Aa","type":"step-finish","reason":"stop","cost":0,"tokens":{"total":5725,"input":5696,"output":4,"reasoning":25,"cache":{"read":0,"write":0}}}}"#;
    /// The same event in the middle of a turn: one step ended and another is about to begin.
    const STEP_FINISH_TOOLS: &str = r#"{"type":"step_finish","timestamp":1755402003000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_fe6da2c60ffe8Ae","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_fe6da2c2affe8Aa","type":"step-finish","reason":"tool-calls","cost":0.0012,"tokens":{"total":812,"input":700,"output":112,"reasoning":0,"cache":{"read":0,"write":0}}}}"#;

    const SESSION: &str = "ses_fe6da2c1effe8Yz";

    /// The role every test starts from: enough to spawn, nothing optional set.
    fn role() -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId("developer".into()),
                label: "Developer".into(),
                scope: cide_ipc::agents::AgentScope::Project,
                harness: cide_ipc::Harness::Opencode,
                description: "Implements one task end to end.".into(),
                system_prompt: "You are the developer agent. Finish the task.".into(),
                model: None,
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

    fn plan_for(agent: &LoadedAgent) -> RunPlan<'_> {
        RunPlan {
            run: RunId::new(),
            session: SessionId::new(),
            agent,
            cwd: PathBuf::from("/repo/.cide/worktrees/developer"),
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
            theme: Theme::Dark,
            proxy: cide_core::proxy::ProxyEnv::default(),
            geometry: Geometry::default(),
            claude: cide_ipc::ClaudeSettings::default(),
            // Off in the fixture, so every argv assertion below is about what the role
            // and the plan actually said; the skip default has tests of its own.
            skip_permissions: false,
        }
    }

    fn spawn(plan: &RunPlan<'_>) -> HarnessSpawn {
        OpencodeHarness.spawn_spec(plan).expect("a spawnable plan")
    }

    fn at(args: &[String], token: &str) -> usize {
        args.iter()
            .position(|a| a == token)
            .unwrap_or_else(|| panic!("no {token} in {args:?}"))
    }

    fn value_of<'a>(args: &'a [String], flag: &str) -> &'a str {
        &args[at(args, flag) + 1]
    }

    fn env_value<'a>(spec: &'a SpawnSpec, name: &str) -> Option<&'a str> {
        // Last wins, matching what the child actually gets.
        spec.env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn config_of(spec: &SpawnSpec) -> Value {
        serde_json::from_str(env_value(spec, "OPENCODE_CONFIG_CONTENT").expect("a config"))
            .expect("the config is json")
    }

    /// The pane's rendering of the event stream, over the shapes a real run printed — including
    /// the two that motivated it: a whole-file `read` arriving as one JSON line, and a rejected
    /// permission, which is the line that answers "why did this run die".
    #[test]
    fn the_rendering_reads_as_a_transcript_and_hides_no_failure() {
        // The channel's own noise draws nothing or one dim marker.
        assert_eq!(render_event(STEP_START), Rendered::Drop);
        let finish = render_event(STEP_FINISH)
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            finish.contains("stop") && finish.contains("5.7k tok"),
            "{finish}"
        );

        // A tool call: name, the CLI's own title, and a clipped output block.
        let big_output = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let tool = format!(
            r#"{{"type":"tool_use","sessionID":"s","part":{{"type":"tool","tool":"bash","state":{{"status":"completed","input":{{"command":"ls -la"}},"output":{},"title":"ls -la"}}}}}}"#,
            serde_json::to_string(big_output).unwrap()
        );
        let rendered = render_event(&tool).text().expect("rendered").to_string();
        assert!(
            rendered.contains("● bash") && rendered.contains("ls -la"),
            "{rendered}"
        );
        assert!(rendered.contains("line6"), "{rendered}");
        assert!(
            !rendered.contains("line7") && rendered.contains("+2 more lines"),
            "the clip must say what it hid: {rendered}"
        );
        assert!(
            !rendered.contains(r#""status""#),
            "JSON structure leaked into the display: {rendered}"
        );

        // The rejected permission — the failure that used to be findable only in opencode's own
        // database — is red and verbatim.
        let rejected = r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"bash","state":{"status":"error","input":{"command":"cat ~/.cargo/config.toml"},"error":"The user rejected permission to use this specific tool call."}}}"#;
        let rendered = render_event(rejected).text().expect("rendered").to_string();
        assert!(
            rendered.contains("✗ bash") && rendered.contains("rejected permission"),
            "{rendered}"
        );

        // The model's words pass whole; reasoning passes dimmed and clipped.
        let text = r#"{"type":"text","sessionID":"s","part":{"type":"text","text":"The task is already in doing.\n\nChecking the build."}}"#;
        let rendered = render_event(text).text().expect("rendered").to_string();
        assert!(rendered.contains("Checking the build."), "{rendered}");
        let reasoning = format!(
            r#"{{"type":"reasoning","sessionID":"s","part":{{"type":"reasoning","text":{}}}}}"#,
            serde_json::to_string(&"x".repeat(500)).unwrap()
        );
        let rendered = render_event(&reasoning)
            .text()
            .expect("rendered")
            .to_string();
        assert!(rendered.contains('…') && rendered.len() < 400, "{rendered}");

        // A CLI prose line — a warning, a rejection notice — passes verbatim: hiding it would
        // hide the failure. An event type this build has never met becomes a dim marker, not a
        // screenful of JSON.
        assert_eq!(
            render_event("! permission requested: bash (*)"),
            Rendered::Keep,
            "the CLI's own prose is kept as it arrived — rendering it back as itself would \
             re-terminate it and lose the bytes the child actually wrote"
        );
        let unknown = render_event(r#"{"type":"session_share","sessionID":"s"}"#)
            .text()
            .expect("marker")
            .to_string();
        assert!(
            unknown.contains("session_share") && !unknown.contains("sessionID"),
            "{unknown}"
        );
    }

    /// `--auto` rides the project default (`agents.skipPermissions`) — and the message stays the
    /// last token either way, because a flag written after the positional would be read as
    /// another word of the message. The module header carries the measurement that reversed the
    /// old never-pass-`--auto` stance: a headless auto-reject does not refuse one tool, it ends
    /// the turn.
    #[test]
    fn the_skip_default_passes_auto_and_the_message_stays_last() {
        let agent = role();
        let mut plan = plan_for(&agent);
        plan.skip_permissions = true;
        let args = spawn(&plan).spec.args;
        assert!(args.iter().any(|a| a == "--auto"), "{args:?}");
        assert_eq!(
            args.last().map(String::as_str),
            Some(plan.prompt.as_str()),
            "a token after the positional message is another word of the message: {args:?}"
        );

        // Off means off — the way back to auto-rejects is one config line, and it must work.
        let args = spawn(&plan_for(&agent)).spec.args;
        assert!(!args.iter().any(|a| a == "--auto"), "{args:?}");
    }

    /// The document is valid JSON and the role's prompt survives it **byte for byte**, which is
    /// the whole reason it is built with serde: a prompt is a document a human wrote, so a quote,
    /// a backslash and a newline are ordinary content rather than exotic input.
    ///
    /// (M40) The task-less twin of the test below: the correction to the tracker paragraph is the
    /// third paragraph of the inline agent's prompt, after the other two, so a role pointed at
    /// either binary reads the same brief — `harness.rs`'s parity test is the cross-check.
    #[test]
    fn the_config_tells_an_adhoc_run_it_stands_in_the_users_tree() {
        let agent = role();
        let mut plan = plan_for(&agent);
        plan.task = None;
        plan.task_title = None;
        let config = config_of(&spawn(&plan).spec);
        assert_eq!(
            config["agent"]["developer"]["prompt"],
            json!(format!(
                "{}\n\n{}\n\n{ADHOC_PREAMBLE}",
                agent.def.system_prompt,
                tracker()
            ))
        );
        // No bridge, no tracker paragraph, no correction: the role's own words alone.
        plan.hook_bin = None;
        assert_eq!(
            config_of(&spawn(&plan).spec)["agent"]["developer"]["prompt"],
            json!(agent.def.system_prompt)
        );
    }

    /// cide's paragraph about the task tracker follows it, after a blank line and never before it,
    /// and the expectation is built from [`TRACKER_PREAMBLE`] rather than from a quoted copy — so
    /// a second definition of that prose cannot pass this file, which is the point of there being
    /// one.
    #[test]
    fn the_config_carries_the_roles_prompt_verbatim() {
        let mut agent = role();
        let prompt = "Say \"hello\".\nUse C:\\tools\\bin and never \\n as a literal.\nReport back.";
        agent.def.system_prompt = prompt.into();

        let config = config_of(&spawn(&plan_for(&agent)).spec);
        assert_eq!(
            config["agent"]["developer"]["prompt"],
            json!(format!("{prompt}\n\n{}", tracker()))
        );
        assert_eq!(
            config["agent"]["developer"]["description"],
            json!("Implements one task end to end.")
        );
        // Load-bearing rather than decorative: the CLI refuses to select a `subagent`-mode agent
        // from `--agent` and silently falls back to its own default one.
        assert_eq!(config["agent"]["developer"]["mode"], json!("primary"));
        assert_eq!(config["$schema"], json!(SCHEMA));

        // The key is the role's id, because that is what `--agent` names.
        assert_eq!(
            value_of(&spawn(&plan_for(&agent)).spec.args, "--agent"),
            "developer"
        );
    }

    /// The other half of the document: cide's own MCP server, so an opencode run reaches the task
    /// tracker exactly as a claude run does.
    #[test]
    fn the_mcp_server_names_the_hook_binary_in_the_shape_the_schema_wants() {
        let agent = role();
        let config = config_of(&spawn(&plan_for(&agent)).spec);

        let server = &config["mcp"]["cide"];
        // `McpLocalConfig`: `type` and a `command` **array**, program first — not claude's
        // `{command, args}` pair.
        assert_eq!(server["type"], json!("local"));
        assert_eq!(server["command"], json!(["/opt/cide/cide-hook", "mcp"]));
        assert_eq!(server["enabled"], json!(true));

        // A path with a quote in it is the case serde is used for, so it is the case asserted.
        let mut plan = plan_for(&agent);
        plan.hook_bin = Some(PathBuf::from("/home/o\"brien/bin/cide-hook"));
        let config = config_of(&spawn(&plan).spec);
        assert_eq!(
            config["mcp"]["cide"]["command"],
            json!(["/home/o\"brien/bin/cide-hook", "mcp"])
        );
    }

    /// The paragraph and the server are one decision, gated on one value.
    ///
    /// `plan.hook_bin` is what writes the `mcp` block, so it is also what licenses the sentences
    /// telling the role to call `mcp__cide__cide_task_*`. Without the bridge there is no such
    /// server in this document — the user's own `opencode.json` may still have others — and a role
    /// pointed at a vocabulary it cannot see would try, fail, and have nothing to say about why.
    /// The role's own prompt is then in the document exactly as its file wrote it.
    #[test]
    fn without_the_bridge_there_is_no_server_and_nothing_is_said_about_it() {
        let agent = role();
        let mut plan = plan_for(&agent);
        plan.hook_bin = None;

        let config = config_of(&spawn(&plan).spec);
        assert!(config.get("mcp").is_none(), "{config}");
        assert_eq!(
            config["agent"]["developer"]["prompt"],
            json!(agent.def.system_prompt)
        );
    }

    /// The invocation shape, in the order it is written, with the message last.
    #[test]
    fn the_argv_is_the_measured_invocation() {
        let mut agent = role();
        agent.def.model = Some("anthropic/claude-sonnet-4-5".into());
        agent.effort = Some("high".into());
        let plan = plan_for(&agent);
        let spawned = spawn(&plan);
        let args = &spawned.spec.args;

        assert_eq!(args[0], "run", "{args:?}");
        assert_eq!(value_of(args, "--agent"), "developer");
        assert_eq!(value_of(args, "--model"), "anthropic/claude-sonnet-4-5");
        assert_eq!(value_of(args, "--variant"), "high");
        assert_eq!(value_of(args, "--format"), "json");
        assert_eq!(value_of(args, "--dir"), "/repo/.cide/worktrees/developer");
        assert_eq!(
            value_of(args, "--title"),
            "Developer · Teach the parser about tabs"
        );

        // The message is positional, so it is last and nothing may be written after it.
        assert_eq!(args.last().map(String::as_str), Some(&*plan.prompt));
        // And it is *not* typed into the terminal: there is no interactive prompt to type into,
        // which is the same fact `deliver` answers `Respawn` to.
        assert_eq!(spawned.opening, None);
        assert_eq!(
            spawned.spec.cwd,
            PathBuf::from("/repo/.cide/worktrees/developer")
        );
        assert_eq!(spawned.spec.program, "opencode");
    }

    /// What a definition did not say, cide does not say either.
    #[test]
    fn nothing_the_definition_left_out_reaches_the_command_line() {
        let agent = role();
        let spawned = spawn(&plan_for(&agent));
        let args = &spawned.spec.args;

        for absent in ["--model", "--variant", "--session", "--auto"] {
            assert!(
                !args.iter().any(|a| a == absent),
                "{absent} must not appear for a plan that did not carry it: {args:?}"
            );
        }
        // `tools` is deprecated in favour of `permission`, and cide has no honest translation from
        // a claude `--allowedTools` list into an opencode permission table.
        let config = config_of(&spawned.spec);
        assert_eq!(config["agent"]["developer"].get("tools"), None);
        assert_eq!(config["agent"]["developer"].get("permission"), None);
    }

    /// The environment: what the child is told, and the two names that are deliberately absent.
    #[test]
    fn the_child_is_told_which_run_it_is_and_nothing_it_cannot_use() {
        let agent = role();
        let plan = plan_for(&agent);
        let spec = spawn(&plan).spec;

        assert_eq!(env_value(&spec, "CIDE_RUN"), Some(&*plan.run.to_string()));
        assert_eq!(
            env_value(&spec, "CIDE_AGENT_SOCK"),
            Some("/run/user/1000/cide-agents-42.sock")
        );
        // No hook can be installed into this CLI, so a routing key would be a name nobody reads —
        // and a hook socket would be a path nothing writes to. Liveness comes from the stream.
        assert_eq!(env_value(&spec, "CIDE_SESSION"), None);
        assert_eq!(env_value(&spec, "CIDE_HOOK_SOCK"), None);

        // The tempting wrong move, refused in the test as well as in the comment: a project's own
        // `opencode.json` is the user's configuration.
        assert_eq!(env_value(&spec, "OPENCODE_DISABLE_PROJECT_CONFIG"), None);
        assert!(
            !spec
                .env_remove
                .iter()
                .any(|k| k == "OPENCODE_DISABLE_PROJECT_CONFIG"),
            "{:?}",
            spec.env_remove
        );

        // The shared list every child in this application gets.
        assert_eq!(env_value(&spec, "TERM"), Some("xterm-256color"));
        assert_eq!(env_value(&spec, "TERM_PROGRAM"), Some("cide"));
    }

    /// A follow-up is a new child that continues the same conversation.
    #[test]
    fn a_follow_up_respawns_into_the_session_the_harness_minted() {
        let agent = role();
        let plan = plan_for(&agent);

        assert_eq!(OpencodeHarness.deliver("carry on"), Delivery::Respawn);

        let spawned = OpencodeHarness
            .respawn_spec(&plan, SESSION)
            .expect("a continuable plan");
        let args = &spawned.spec.args;
        assert_eq!(value_of(args, "--session"), SESSION);
        // The conversation already has a title; renaming it mid-flight is not this call's to do.
        assert!(!args.iter().any(|a| a == "--title"), "{args:?}");
        // Same worktree, same configuration, same message position — only the two tokens above
        // differ, which is why one function builds both.
        assert_eq!(spawned.spec.cwd, plan.cwd);
        assert_eq!(args.last().map(String::as_str), Some(&*plan.prompt));
        // The configuration includes cide's paragraph about the tracker, because a second turn of
        // this conversation is a second *process*: everything the first child was told has to be
        // told again, and a follow-up that quietly dropped the role's brief or the tracker rule
        // would behave differently from the turn before it for reasons nothing anywhere records.
        assert_eq!(
            config_of(&spawned.spec)["agent"]["developer"]["prompt"],
            config_of(&spawn(&plan).spec)["agent"]["developer"]["prompt"]
        );
        assert_eq!(
            config_of(&spawned.spec)["agent"]["developer"]["prompt"],
            json!(format!("{}\n\n{}", plan.agent.def.system_prompt, tracker()))
        );
    }

    /// The claude side of a respawn, asserted from here because this file owns the trait's
    /// respawn vocabulary. It used to refuse (`deliver` is `Stdin`, so a *follow-up* never
    /// respawns) — what changed is the snapshot: a run restored after a cide restart has no
    /// child to type into, and its continuation is `--resume` into the same conversation. The
    /// argv is the claim: resume the run's own session, mint nothing.
    #[test]
    fn a_claude_respawn_resumes_the_runs_own_session() {
        let mut agent = role();
        agent.def.harness = cide_ipc::Harness::Claude;
        let plan = plan_for(&agent);
        let spawn = crate::harness::ClaudeHarness
            .respawn_spec(&plan, &plan.session.to_string())
            .expect("a continuing claude child");

        let args = &spawn.spec.args;
        assert_eq!(value_of(args, "--resume"), plan.session.to_string());
        assert!(
            !args.iter().any(|a| a == "--session-id"),
            "a resume that also minted an id would be the 2.1.227 shape the CLI rejects: {args:?}"
        );
        assert!(
            spawn.opening.is_some(),
            "the continuation prompt is still typed into the restored TUI"
        );
    }

    /// Two refusals that are values rather than an `ENOENT` from three processes below.
    #[test]
    fn a_plan_this_harness_cannot_run_is_refused_with_a_reason() {
        let mut agent = role();
        agent.def.harness = cide_ipc::Harness::Claude;
        assert_eq!(
            OpencodeHarness.spawn_spec(&plan_for(&agent)).unwrap_err(),
            HarnessError::WrongHarness {
                plan: cide_ipc::Harness::Claude,
                harness: cide_ipc::Harness::Opencode,
            }
        );

        let agent = role();
        let mut plan = plan_for(&agent);
        plan.prompt = "   ".into();
        assert_eq!(
            OpencodeHarness.spawn_spec(&plan).unwrap_err(),
            HarnessError::NoPrompt
        );
    }

    /// The id is on the first line, which is what makes the capture a scan rather than a ladder.
    #[test]
    fn the_session_id_is_read_off_the_first_line_and_a_warning_is_survived() {
        assert_eq!(capture(STEP_START).as_deref(), Some(SESSION));
        assert_eq!(capture(STEP_FINISH).as_deref(), Some(SESSION));

        // A PTY turns every `\n` into `\r\n`, so a line split on `\n` still ends in a carriage
        // return. It must not be the difference between a resumable run and a lost one.
        assert_eq!(
            capture(&format!("{STEP_START}\r")).as_deref(),
            Some(SESSION)
        );

        // The CLI prints plain, ANSI-styled lines of its own — a rejected permission, an agent
        // name that did not resolve — into the same stream.
        assert_eq!(
            capture("\u{1b}[33m!\u{1b}[0m permission requested: bash (*); auto-rejecting"),
            None
        );
        assert_eq!(capture(""), None);
        assert_eq!(capture("{not json at all"), None);
        // JSON, but nothing that names a session.
        assert_eq!(
            capture(r#"{"type":"error","timestamp":1,"error":{"name":"x"}}"#),
            None
        );
        assert_eq!(capture(r#"{"type":"text","sessionID":""}"#), None);
    }

    /// The mapping the module header tabulates, over the real lines.
    #[test]
    fn the_stream_is_the_whole_state_machine() {
        let h = OpencodeHarness;
        let line = |current: RunState, text: &str| h.observe(current, Observation::Line(text));

        // A run leaves `Starting` on the first thing its child says.
        assert_eq!(
            line(RunState::Starting, STEP_START),
            Some(RunState::Running)
        );
        assert_eq!(line(RunState::Starting, TEXT), Some(RunState::Running));
        // …and a transition to the state it already holds is not a transition.
        assert_eq!(line(RunState::Running, TEXT), None);

        // Both halves of `step_finish` answer the same thing now. The final-looking one
        // (`reason: "stop"`) used to answer `Idle` — the misreading that released the slot,
        // admitted the queued run and got the working child of run 06202dd6 wound down
        // mid-task; see `observe`'s doc. No line ends the turn; the exit does.
        assert_eq!(
            line(RunState::Running, STEP_FINISH),
            None,
            "a step finishing is a run that is working — never a turn handed back"
        );
        assert_eq!(line(RunState::Running, STEP_FINISH_TOOLS), None);
        assert_eq!(
            line(RunState::Starting, STEP_FINISH),
            Some(RunState::Running),
            "…and from `Starting` it is evidence of life, exactly like any other event"
        );
        assert_eq!(
            line(RunState::Starting, STEP_FINISH_TOOLS),
            Some(RunState::Running),
            "an intermediate step is a run that is working, not one that has stopped"
        );

        // Nothing else in the stream says anything about the run.
        assert_eq!(line(RunState::Running, "some ANSI-styled warning"), None);
        assert_eq!(
            line(
                RunState::Running,
                r#"{"type":"error","timestamp":1,"sessionID":"ses_x","error":{"name":"ProviderAuthError"}}"#
            ),
            None,
            "the exit that follows carries the truthful record"
        );

        // No hook cide can install reaches this CLI.
        assert_eq!(
            h.observe(
                RunState::Running,
                Observation::Hook(&HookFrame::new("Stop", json!({})))
            ),
            None
        );
    }

    /// Only the reaper ends a run, and two states absorb everything else.
    #[test]
    fn a_frozen_or_finished_run_is_not_moved_by_a_late_line() {
        let h = OpencodeHarness;

        let paused = RunState::Paused {
            since_unix_ms: 1_700_000_000_000,
        };
        assert_eq!(
            h.observe(paused.clone(), Observation::Line(STEP_START)),
            None,
            "only a resume clears a freeze cide asserted"
        );

        let finished = RunState::Finished { code: 0 };
        assert_eq!(
            h.observe(finished.clone(), Observation::Line(STEP_FINISH)),
            None,
            "the last lines of a dead child's output are read after it is reaped"
        );

        // The exception, from any state: an exit is the only observation carrying ground truth.
        assert_eq!(
            h.observe(RunState::Idle, Observation::Exit(3)),
            Some(RunState::Finished { code: 3 })
        );
        assert_eq!(
            h.observe(paused, Observation::Exit(0)),
            Some(RunState::Finished { code: 0 })
        );
        assert_eq!(h.observe(finished, Observation::Exit(0)), None);
    }

    /// The document is one the **real** `opencode` accepts, and the role is registered under the
    /// name `--agent` will look for.
    ///
    /// `#[ignore]`d for this workspace's stated reason: it needs the binary on `PATH`. It needs
    /// nothing else — no account, no network turn, no quota — because `agent list` resolves the
    /// configuration and prints the roster without ever reaching a model. That makes it the
    /// cheapest possible check of the one claim this whole file rests on, and the one to run when
    /// a release moves the schema:
    ///
    /// ```sh
    /// cargo test -p cide-agents opencode -- --ignored
    /// ```
    #[test]
    #[ignore = "needs the real opencode binary on PATH"]
    fn the_real_opencode_registers_the_inline_role() {
        let mut agent = role();
        agent.def.system_prompt = "Quote \" backslash \\ newline\nand a second line.".into();
        let plan = plan_for(&agent);
        let spec = spawn(&plan).spec;
        let config = env_value(&spec, "OPENCODE_CONFIG_CONTENT").expect("a config");

        let out = std::process::Command::new("opencode")
            .args(["agent", "list"])
            .env("OPENCODE_CONFIG_CONTENT", config)
            .output()
            .expect("opencode on PATH");
        let listed = String::from_utf8_lossy(&out.stdout);
        assert!(
            listed.contains("developer (primary)"),
            "the inline role is not registered:\n{listed}"
        );
    }
}
