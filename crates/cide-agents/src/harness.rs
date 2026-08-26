//! What it takes to turn a role and a task into a child process, per CLI. (M18)
//!
//! # One trait, because there are two CLIs and there will be more
//!
//! A run is dispatched against a [`crate::LoadedAgent`], and the role's own definition names the
//! harness that carries it — `claude` and `opencode` today, and [`cide_ipc::Harness`]'s doc
//! says plainly that more are expected. Everything downstream of a spawn is already harness-blind:
//! `cide_pty::SpawnSpec` describes a child without caring what it is, `SessionRegistry` keys it,
//! `lifecycle::watch_for_exit` reaps it, and the shutdown ladder covers it. So the only thing that
//! genuinely differs per CLI is the four questions below, and they are the whole trait.
//!
//! # The five questions, and why each one is a method rather than a field
//!
//! * **What does the child look like?** [`Harness::spawn_spec`] — argv, environment, cwd. The
//!   interesting part is the *order* of the argv, which is load-bearing per CLI and documented at
//!   each implementation rather than here.
//! * **How does the run get its identity?** [`SessionBinding`]. `claude` accepts an id the caller
//!   chooses (`--session-id`) and echoes it back in every hook frame, so cide knows the run's
//!   identity before the process exists. `opencode` mints its own and *prints* it, so the identity
//!   arrives on stdout some milliseconds into the run and has to be scraped. Both facts are real
//!   and neither can be made to look like the other; the enum is that difference, named.
//! * **How does a follow-up reach a run that is already alive?** [`Delivery`], and this is the one
//!   that decided the shape of the whole trait. See below.
//! * **And if the answer to that is “start another child”, what does *that* child look like?**
//!   [`Harness::respawn_spec`], which is defaulted and refusing because only a CLI that cannot be
//!   spoken to needs it. It exists as a second method rather than as a field on [`RunPlan`] so
//!   that a harness which never respawns is not obliged to carry a resume id it would ignore.
//! * **What does an observation mean?** [`Harness::observe`] maps a hook frame, an output line or
//!   an exit into a [`RunState`] — or into `None`, which means *this said nothing about the run*
//!   and is distinct from *it said the run is where it already was*.
//!
//! # Why [`Delivery`] is an enum and not a `write(&self, bytes)` method
//!
//! Because **`opencode run` cannot take a follow-up.** It reads one prompt, answers, and exits;
//! there is no stdin to write a second turn into, and the only way to continue is to respawn with
//! `--session <id>`. A trait with a `write()` on it would have compiled perfectly and forced
//! `OpencodeHarness` to lie — to accept bytes it had nowhere to put, and either drop them or
//! silently buffer them against a process that no longer exists. A queue that cannot see the
//! difference is a queue that reports a message delivered when nothing received it.
//!
//! So the harness *answers* how to deliver rather than performing the delivery, and the caller —
//! which owns the session registry and can therefore both write to a PTY and start a new child —
//! does what it is told. [`Delivery::Respawn`] is a real answer, not a failure.
//!
//! # What is deliberately not here
//!
//! **No spawning, no registry, no git.** Nothing in this module or its children starts a process,
//! inserts into anything, or asks a repository a question. [`RunPlan`] is a value the dispatch
//! site composes — including the worktree path, which `cide_git::worktree::ensure` produced
//! before this crate was ever called — and every implementation is therefore a pure function over
//! it. That is what lets the tests below assert on a real argv and a real environment with no
//! `claude` on `PATH`, no repository, and no `.cide/` anywhere.
//!
//! It is also why `cide-agents` still does not depend on `cide-git` or on tauri, and must not
//! start: the moment a harness reaches for a repository it becomes untestable in exactly the
//! place where the ordering rules live.

use std::path::PathBuf;

use cide_claude::HookFrame;
use cide_core::proxy::ProxyEnv;
use cide_ipc::{Geometry, ProjectId, RunId, RunState, SessionId, TaskId, Theme};
use cide_pty::SpawnSpec;

use crate::defs::{LoadedAgent, harness_name};

pub mod claude;
pub mod opencode;

pub use claude::ClaudeHarness;
pub use opencode::OpencodeHarness;

/// What every role is told about the task tracker, folded into its own system prompt.
///
/// # The second of the two things that keep the tracker single-writer
///
/// `cide_tasks`' header names them both: *the agent prompt preamble, and a `PreToolUse` hook that
/// denies an `Edit`/`Write` resolving to this path*. This is the first of the pair, and the fence
/// in `cide-hook`'s `guard` is the second. The fence alone works and is not enough: a role that
/// only learns the rule by being refused spends a turn discovering it, and leaves a transcript in
/// which it reached for the sensible-looking thing and was slapped. This says it before the first
/// turn, so the refusal is a backstop rather than the teacher — which is also why the two texts
/// use one vocabulary: the same file, the same reason, the same tool names. They are read by the
/// same model minutes apart, and a second wording of one rule reads as a second rule.
///
/// # One definition, because two would be the bug in miniature
///
/// A role is a `.cide/agents/<name>.md` file that names its harness, and the same file may be
/// pointed at either CLI. A paragraph copied into `claude.rs` and `opencode.rs` would let a role
/// be told different things depending on which binary happened to run it — divergence about
/// *concurrency*, arrived at by copy-paste, which is exactly the class of failure the tracker's
/// one-process-one-mutex design exists to remove. So the prose lives here and both harnesses
/// carry it by reference; a test on each side asserts against this constant rather than against a
/// quoted copy, so a copy cannot pass.
///
/// # Namespaced tool names, measured
///
/// `mcp__cide__cide_task_get`, never the bare `cide_task_get`. The CLI namespaces an MCP server's
/// tools as `mcp__<server>__<tool>` — the same fact `claude.rs::mcp_config` and the roster
/// paragraph in `cmd/session.rs` are both written against — so the bare form names nothing the
/// model can see, and telling it to call a name that does not exist is worse than saying nothing.
///
/// # Why the last sentence is the one that matters
///
/// The rest is a rule; that sentence is the *purpose*. A dispatched run reports through its task's
/// comments and through nothing else — the orchestrator that dispatched it reads
/// `cide_task_get`, not a terminal nobody has open — so a run that does good work and finishes
/// silently is indistinguishable from one that did nothing, which is the failure the whole tracker
/// exists to prevent.
///
/// # It is only emitted when the tools are attached
///
/// Both harnesses gate it on [`RunPlan::hook_bin`], which is the one thing that decides whether
/// `cide-hook mcp` is attached at all — `claude.rs`'s `--mcp-config` and `opencode.rs`'s `mcp`
/// block are both written from it. A prompt naming tools the session does not have is a session
/// told to reach for a vocabulary it cannot see, and it has nothing to say about why the call
/// failed; `cmd/session.rs` gates its roster paragraph on the same question for the same reason.
///
/// Kept to six sentences on purpose. This is prepended to every turn of every run for ever, and
/// a page of cide's prose in front of the role's own is a role diluted by its host. The fifth is
/// the checkpoint discipline (P4 of the debug-report plan — its runs died mid-turn with an empty
/// worktree branch and a silent task, so nothing said how far they got): the plan comment proves
/// liveness on the board, and a commit per coherent step makes the worktree branch the durable
/// record a death cannot erase. The sixth is the done-workflow convention's durable half (the
/// opening prompt says it too, but an opening prompt is one turn ago by the time the work is
/// done): finish → review + comment, and `done` belongs to whoever reviews.
pub const TRACKER_PREAMBLE: &str = "This project's tasks live in .cide/tasks.json, and cide holds \
     the only writer for it: one process owns that file, so a write made behind its back is \
     either overwritten by that process's next write or merged unpredictably with it, and the \
     work in it is lost. Editing the file directly is refused, so it is not a fallback. Read with \
     mcp__cide__cide_task_get and mcp__cide__cide_task_list; change a task's title, body or \
     status with mcp__cide__cide_task_update, append a comment with mcp__cide__cide_task_comment, \
     set the role a task is for with mcp__cide__cide_task_assign, and open a new one with \
     mcp__cide__cide_task_create. Those comments are how you report back: whoever dispatched you \
     reads the task, not your transcript, so a run that finishes without leaving one has reported \
     nothing. Before you start, comment your plan on the task, and commit each coherent step of \
     the work as you finish it: a run can die mid-turn, and the plan comment plus your branch's \
     commits are the only record of how far you got. When the work is done, set the task's status to review and comment what you did \
     and where; done is the reviewer's call, not yours.";

/// Everything one implementation of a CLI has to answer.
///
/// `Send + Sync + 'static` because the registry hands out `&'static dyn Harness` and a dispatch
/// may happen on any thread. Every implementation here is a unit struct holding no state at all,
/// which is deliberate: a harness that cached anything would be a second place for a role
/// definition to be remembered after the file behind it changed, and `crate`'s module header is
/// explicit that nothing in this crate caches.
pub trait Harness: Send + Sync + 'static {
    /// Which CLI this is. The registry key, and what a [`RunPlan`] is checked against.
    fn kind(&self) -> cide_ipc::Harness;

    /// The child this run would be, as a value.
    ///
    /// Pure over `plan` save for two reads of this process's own environment that every spawn
    /// site in the workspace makes — `APPDIR`, through [`cide_core::claude_cli::plan_here`], and
    /// the inherited proxy variables, which the caller has already resolved into
    /// [`RunPlan::proxy`]. Nothing here touches the filesystem, so a failure is a value and not
    /// an `ENOENT` from three processes below.
    fn spawn_spec(&self, plan: &RunPlan<'_>) -> Result<HarnessSpawn, HarnessError>;

    /// How `text` should reach a run that is already alive.
    ///
    /// The caller decides *when* — the queue sends a follow-up only into a run whose session is
    /// `Idle | AwaitingInput`, which is a real ready signal rather than a sleep — and this decides
    /// *how*.
    fn deliver(&self, text: &str) -> Delivery;

    /// The child that **continues** an existing conversation, for a harness whose [`Self::deliver`]
    /// answers [`Delivery::Respawn`].
    ///
    /// `session` is the *harness's* own id — the string [`SessionBinding::Harness`]'s `capture`
    /// read off this run's output — and never cide's [`SessionId`]. The two are unrelated: cide
    /// mints the second before the fork, and the first arrives from a process that had already
    /// decided what to call itself.
    ///
    /// # Why it is defaulted, and why the default refuses
    ///
    /// Only a harness that cannot be spoken to needs this, and `claude` can be spoken to: its
    /// follow-up is [`Delivery::Stdin`] into a live child, and *starting a second `claude`* would
    /// not continue that conversation — it would begin a new one, silently, having thrown away
    /// the turn the caller thought it was extending. There is nothing truthful for it to return,
    /// so the default returns nothing truthful: [`HarnessError::NoRespawn`], naming the harness
    /// that was asked.
    ///
    /// A required method would have forced every implementation to write that refusal by hand,
    /// which is exactly how one of them eventually writes a plausible-looking respawn instead.
    fn respawn_spec(
        &self,
        plan: &RunPlan<'_>,
        session: &str,
    ) -> Result<HarnessSpawn, HarnessError> {
        let _ = (plan, session);
        Err(HarnessError::NoRespawn {
            harness: self.kind(),
        })
    }

    /// What one observation means for a run currently in `current`.
    ///
    /// `None` means *nothing here says anything about the run*, which is not the same as
    /// *unchanged*; a caller can therefore distinguish "no transition" from "transitioned to the
    /// value it already held" without comparing, exactly as [`cide_claude::next_state`] does.
    ///
    /// Two rules every implementation owes, because a caller cannot enforce either:
    ///
    /// * **A [`RunState::Paused`] run is never moved by an observation.** The freeze is a fact
    ///   cide asserted with `SIGSTOP`, and only a resume clears it. A frame that was already in
    ///   flight when the signal landed must not thaw the row on screen.
    /// * **A late frame must not resurrect a finished run.** `Finished`/`Failed` absorb hooks and
    ///   output lines. [`Observation::Exit`] is the exception and always answers, because it is
    ///   the only observation carrying ground truth about the child.
    fn observe(&self, current: RunState, ob: Observation<'_>) -> Option<RunState>;
}

/// Everything needed to describe one run's child, composed by the dispatch site.
///
/// # Why so much of this is handed in rather than looked up
///
/// Every field here is something this crate *could* have gone and found — the socket paths from
/// managed state, the theme from the workspace, the worktree from git, the version from an
/// environment variable. Looking any of them up would put a lock, a repository or an `AppHandle`
/// inside a function whose entire value is that it is provable from its arguments. The dispatch
/// site already holds all of them, on a thread that is allowed to block, so the composition is
/// free there and the argv rules stay testable here.
///
/// The lifetime is on [`Self::agent`] alone: a plan is built, handed to one `spawn_spec` and
/// dropped, and cloning a role's whole system prompt to do that would be a copy per dispatch for
/// nothing.
#[derive(Debug, Clone)]
pub struct RunPlan<'a> {
    /// This run's own identity, for the whole of its life. `CIDE_RUN` in the child.
    pub run: RunId,
    /// The session the child will be filed under, and — for `claude` — the value passed to
    /// `--session-id`. `CIDE_SESSION` in the child, which is what every hook frame is routed on.
    pub session: SessionId,
    /// The role, already loaded, already validated, already checked against
    /// [`crate::dispatch_refusal`] by the caller.
    pub agent: &'a LoadedAgent,
    /// The directory the child starts in.
    ///
    /// **The agent's worktree when isolation is on**, produced by `cide_git::worktree::ensure`
    /// before this crate was called. Taken as a path rather than derived here so that nothing in
    /// `cide-agents` has to link git — see the module header.
    pub cwd: PathBuf,
    pub project: ProjectId,
    /// The task this run was dispatched against, if any. Ad-hoc runs are legal.
    pub task: Option<TaskId>,
    /// The task's title, **copied at dispatch rather than looked up at spawn**, for
    /// [`cide_ipc::AgentRun::agent_label`]'s reason: it ends up in the child's terminal title and
    /// in `/resume`, and those must still read correctly after the task has been renamed.
    pub task_title: Option<String>,
    /// The opening prompt: the task body plus whatever extra instruction this dispatch carried.
    ///
    /// Composed by the caller because the composition is a *task tracker* question — which fields
    /// of a [`cide_ipc::Task`] a run should be told about, and how they are fenced — and
    /// `cide_agents::tools::preamble` already owns that vocabulary.
    pub prompt: String,
    /// Absolute path to the `cide-hook` binary, or `None` when it cannot be located.
    ///
    /// Absolute, and that is not a detail: the child's cwd is a worktree and its `PATH` is the
    /// user's, so a bare `cide-hook` in a `--mcp-config` resolves to nothing and the CLI reports
    /// `CONNECTION_CLOSED` from a process three levels below anything cide logs.
    pub hook_bin: Option<PathBuf>,
    /// The socket `cide-hook <event>` writes frames to. `CIDE_HOOK_SOCK`.
    pub hook_sock: Option<PathBuf>,
    /// The socket `cide-hook mcp` bridges to. `CIDE_AGENT_SOCK`.
    pub agent_sock: Option<PathBuf>,
    /// Which way round the CLI should draw itself — see `cide_claude::settings`, which records
    /// that this is the real fix for white-on-white text rather than a palette question.
    pub theme: Theme,
    /// The proxy environment for this child, already resolved against this process's own.
    ///
    /// Resolved by the caller because [`ProxyEnv::for_target`] reads `std::env::var` and the
    /// scope decision (`ProxyScope::claude` versus `::shells`) belongs to the settings layer.
    /// A subagent is a `claude`, so it takes the `claude` column.
    pub proxy: ProxyEnv,
    /// The terminal size the child is told it has.
    ///
    /// A headless run has no pane and therefore no measurement, so this is a default — which is a
    /// decision rather than an oversight, and `PaneRestore`'s "only the frontend knows a pane's
    /// size" is why it has to be written down. A run later opened into a pane is resized then,
    /// through the same path a re-docked pane uses.
    pub geometry: Geometry,
    /// The user's Claude settings: which binary, what they add to the command line, and the
    /// `CLAUDE_CODE_*` switches. A subagent is a Claude child and gets the same treatment a pane
    /// does, for the same reason `terminal_child_env` is shared rather than copied.
    pub claude: cide_ipc::ClaudeSettings,
    /// [`crate::config::AgentsConfig::skip_permissions`], read fresh at this spawn.
    ///
    /// Carried on the plan rather than looked up here for the module's stated reason — nothing
    /// in this crate reads disk — and what it turns on is per-harness: `--permission-mode
    /// bypassPermissions` for a `claude` run whose role names no mode of its own, `--auto` for
    /// an `opencode` run. The config field's doc carries the measured argument for the default.
    pub skip_permissions: bool,
}

/// A child, described — plus the two things the caller cannot work out for itself.
#[derive(Debug, Clone)]
pub struct HarnessSpawn {
    /// The spec, ready for `PtySession::spawn`. The same type a pane builds, deliberately: there
    /// is no second process-hosting path in this application, which is what makes the shutdown
    /// ladder, the orphan arming and the close confirm cover subagent runs for free.
    pub spec: SpawnSpec,
    /// Bytes to write into the PTY once the child is up, or `None` when the prompt travelled in
    /// the argv instead.
    ///
    /// **Not an argument**, for `claude`, and that is the interesting half. A positional prompt
    /// would have to be placed after a variadic flag — `--mcp-config <configs...>` collects every
    /// following token that does not begin with `-`, and a bare prompt is exactly such a token —
    /// so it would be swallowed into an MCP configuration list with no error anywhere. Writing it
    /// into the terminal is also the same mechanism a follow-up uses, so a run's first turn and
    /// its fifth arrive by one code path.
    pub opening: Option<Vec<u8>>,
    /// How this run's identity becomes known. See [`SessionBinding`].
    pub binding: SessionBinding,
}

/// How a run's identity becomes known. **This enum is the claude/opencode difference.**
///
/// It is not an abstraction looking for a second case: the two CLIs genuinely answer "who is this
/// conversation" at different times and from different directions, and a design that assumed
/// either one would have to special-case the other somewhere less visible than a type.
#[derive(Clone, Copy)]
pub enum SessionBinding {
    /// The caller chose the id and told the child (`claude --session-id <uuid>`), so the run is
    /// identifiable before the process exists — which is what makes a queued run cancellable, a
    /// hook frame routable on `CIDE_SESSION`, and resume free.
    Caller,
    /// The harness mints its own and prints it; `capture` is run over each output line until it
    /// answers. Until it does, the run has a [`RunId`] and no session — listable, killable, but
    /// not yet resumable.
    ///
    /// `render` is the other consequence of an identity that arrives on stdout: a harness bound
    /// this way speaks machine events on its output, which is simultaneously the channel the
    /// app observes and the picture a pane shows. It turns one event line into display text —
    /// `None` drops the line, and anything unrecognised should come back verbatim. The app
    /// installs it as the session's `cide_pty::LineRender`, where its header carries the whole
    /// argument for why the rendering happens once, upstream of the mirror and every sink.
    Harness {
        capture: fn(&str) -> Option<String>,
        render: fn(&str) -> Option<String>,
    },
}

impl PartialEq for SessionBinding {
    /// Compares *which* binding this is, and deliberately not the capture function.
    ///
    /// A derived `PartialEq` compares the function pointers, which rustc warns about and is right
    /// to: the address of one function can differ between codegen units, and two different
    /// functions can be merged onto one address. So the derived answer is neither reliably `true`
    /// for the same `capture` nor reliably `false` for a different one.
    ///
    /// Comparing the variant is also the only question a caller has. `SessionBinding` answers
    /// *how this run's identity becomes known*, and a dispatch site branches on that — write the
    /// id down now, or watch the output for it. Which scanner a harness happens to use is that
    /// harness's business.
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl Eq for SessionBinding {}

impl std::fmt::Debug for SessionBinding {
    /// Hand-written because a derived one prints a function pointer's address, which changes
    /// between builds and makes an assertion on a `Debug` string unstable for no gain.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Caller => f.write_str("Caller"),
            Self::Harness { .. } => f.write_str("Harness { capture: .. }"),
        }
    }
}

/// How a follow-up reaches a run that is already alive. See the module header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Write these bytes into the run's PTY.
    Stdin(Vec<u8>),
    /// This CLI cannot be spoken to. Start a fresh child that continues the same conversation.
    Respawn,
}

/// Something cide learned about a run.
///
/// Three sources, because a run has three: the hook socket (`claude` only, and by far the
/// richest), the child's own output (which is all `opencode` offers), and the reaper.
#[derive(Debug, Clone, Copy)]
pub enum Observation<'a> {
    /// A hook frame, already routed to this run by `CIDE_SESSION`.
    Hook(&'a HookFrame),
    /// One line the child printed.
    Line(&'a str),
    /// The child was reaped with this status.
    Exit(i32),
}

/// Why a run cannot be described as a child.
///
/// Three variants, and all three are decided from the plan alone. A harness never reports
/// **"cannot be run"** from here, because that question was answered at load and the sentence is
/// already on the role's [`cide_ipc::AgentDef::unavailable`], where the panel is drawing it. It is
/// two questions, answered by two functions: [`crate::defs::implemented`] asks whether *this build*
/// has a harness at all — a [`for_kind`] lookup, so it cannot disagree with the one the spawn makes
/// — and [`crate::defs::installed`] asks whether the *machine* has the binary. Asking either a
/// second time here would mean a `stat` per `PATH` entry per dispatch, and a second wording of a
/// refusal the user has already read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessError {
    /// A role belonging to one harness was handed to another's implementation. A dispatch-site
    /// bug rather than a user one, and it is caught rather than papered over because the quiet
    /// version of it is an `opencode` role spawned as `claude` with an argv it does not parse.
    WrongHarness {
        plan: cide_ipc::Harness,
        harness: cide_ipc::Harness,
    },
    /// The configured binary is blank. Refused here rather than handed to the OS, which would
    /// answer `ENOENT` naming a program the user never typed.
    NoBinary,
    /// The run has nothing to do.
    ///
    /// Refused rather than spawned, because the failure is otherwise invisible: an interactive
    /// `claude` with no opening prompt starts perfectly, sits at its prompt for ever, holds a
    /// concurrency slot and a worktree, and reports no error to anyone.
    NoPrompt,
    /// The role could not be written as the CLI's own configuration document.
    ///
    /// Only reachable for a harness that carries the role's system prompt *in a configuration*
    /// rather than in an argument — `opencode`, whose whole role definition travels in
    /// `OPENCODE_CONFIG_CONTENT`. Refused rather than degraded, and that is the point of the
    /// variant: without the document the CLI has no agent by that name, warns once, and runs its
    /// own default agent instead — a run that looks like it worked while carrying somebody else's
    /// system prompt.
    NoConfig,
    /// A follow-up was routed as a respawn to a harness that takes follow-ups on stdin.
    ///
    /// A caller bug rather than a user one, like [`Self::WrongHarness`], and caught for the same
    /// reason: the quiet version of it starts a **second** conversation and reports it as a
    /// continuation of the first.
    NoRespawn { harness: cide_ipc::Harness },
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongHarness { plan, harness } => write!(
                f,
                "this run names the “{}” harness and was handed to the “{}” one",
                harness_name(*plan),
                harness_name(*harness)
            ),
            Self::NoBinary => f.write_str(
                "Settings → Claude sessions names no binary to run. Put “claude” back, or the \
                 path to the CLI you want subagents to use.",
            ),
            Self::NoPrompt => f.write_str(
                "this run has no prompt. A task with an empty body gives an agent nothing to do, \
                 and the run would sit at a prompt holding a worktree until somebody noticed.",
            ),
            Self::NoConfig => f.write_str(
                "this role could not be written as a configuration for its CLI, so nothing would \
                 carry its system prompt. The run is refused rather than started as the CLI's \
                 own default agent, which would look like it worked.",
            ),
            Self::NoRespawn { harness } => write!(
                f,
                "a run on the “{}” harness is continued by writing into the child it already \
                 has, not by starting another one",
                harness_name(*harness)
            ),
        }
    }
}

impl std::error::Error for HarnessError {}

/// The one instance of each harness. Unit structs, so this costs nothing to hold.
static CLAUDE: ClaudeHarness = ClaudeHarness;
static OPENCODE: OpencodeHarness = OpencodeHarness;

/// The registry itself, as a `const` rather than built in [`registry`].
///
/// Written this way because the obvious `&[&CLAUDE]` inside the function is a reference to a
/// temporary and will not compile: const-promotion does not reach through the unsizing coercion
/// to `&dyn Harness`. A `const` gives the slice `'static` storage, which is what the signature
/// promises.
const REGISTRY: &[&dyn Harness] = &[&CLAUDE, &OPENCODE];

/// Every harness this build has.
///
/// A slice rather than the `DashMap<Harness, Arc<dyn Harness>>` the design sketched, because
/// every implementation is a stateless unit struct: a map would buy a hash lookup over a
/// two-element scan and cost an allocation, a lock and a dependency.
///
/// The order is the order [`cide_ipc::Harness`] declares, so a caller that lists harnesses lists
/// them the same way the settings surface does.
///
/// Adding `opencode` was, as promised, one `static` and one entry in the slice below — the
/// property the trait was chosen for, now demonstrated rather than asserted.
pub fn registry() -> &'static [&'static dyn Harness] {
    REGISTRY
}

/// The implementation for one harness, or `None` when this build has none.
///
/// `None` is not reachable today — the enum is closed and every variant is implemented — and it
/// is still an `Option` rather than a panic, because the variant that lands *before* its
/// implementation does is the ordinary shape of a staged delivery, and a roster that greys one
/// role is a better answer than a dispatch that aborts the process.
pub fn for_kind(kind: cide_ipc::Harness) -> Option<&'static dyn Harness> {
    registry().iter().copied().find(|h| h.kind() == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant of the wire enum has an implementation, and each answers for itself.
    ///
    /// The test that makes "adding a harness is one insert" true rather than aspirational: a new
    /// variant with no `impl` fails here rather than at a dispatch nobody ran.
    #[test]
    fn the_registry_answers_for_every_harness_the_wire_knows() {
        // Written out by hand, never derived from the registry: this is a check of
        // `cide_ipc::Harness`'s variants *against* the registry, and a list read out of the thing
        // under test would pass no matter what either of them said. `Opencode` joined this line
        // and the registry in the same commit, which was the point; the next variant does too.
        const EVERY: &[cide_ipc::Harness] =
            &[cide_ipc::Harness::Claude, cide_ipc::Harness::Opencode];

        for kind in EVERY.iter().copied() {
            let harness = for_kind(kind).expect("a harness for every variant");
            assert_eq!(harness.kind(), kind);
        }
        assert_eq!(registry().len(), EVERY.len());
    }

    /// `Delivery` is comparable, which is the whole reason a queue can act on it.
    #[test]
    fn a_delivery_is_a_value_a_caller_can_branch_on() {
        assert_ne!(Delivery::Stdin(b"hi\r".to_vec()), Delivery::Respawn);
    }

    /// Every name in the paragraph is a name the model can actually call.
    ///
    /// The tools arrive as `mcp__<server>__<tool>` — measured while `claude.rs::mcp_config` was
    /// written, and the reason cide's server is called `cide` — so a bare `cide_task_update` in
    /// this prose would be an instruction to call something that does not exist. The count
    /// comparison is the assertion that matters: it fails on the *next* mention somebody adds
    /// without the prefix, not just on the six that are here today.
    #[test]
    fn the_tracker_preamble_names_the_tools_in_the_spelling_they_arrive_in() {
        for verb in ["get", "list", "create", "update", "comment", "assign"] {
            let name = format!("mcp__cide__cide_task_{verb}");
            assert!(
                TRACKER_PREAMBLE.contains(&name),
                "{name} is missing: {TRACKER_PREAMBLE}"
            );
        }
        assert_eq!(
            TRACKER_PREAMBLE.matches("cide_task_").count(),
            TRACKER_PREAMBLE.matches("mcp__cide__cide_task_").count(),
            "every mention is namespaced: {TRACKER_PREAMBLE}"
        );

        // The same file the `PreToolUse` fence in `cide-hook` refuses, named the same way, because
        // the two texts are one rule read by one model minutes apart.
        assert!(
            TRACKER_PREAMBLE.contains(".cide/tasks.json"),
            "{TRACKER_PREAMBLE}"
        );
        // And the sentence the whole thing is for: a run that finishes silently has reported
        // nothing, which is the failure the tracker exists to prevent.
        assert!(
            TRACKER_PREAMBLE.contains("report back"),
            "{TRACKER_PREAMBLE}"
        );
        // The done-workflow convention's durable half: finish → review + comment, and `done`
        // belongs to the reviewer. The opening prompt says it too, but that was one turn ago by
        // the time the work is done.
        assert!(
            TRACKER_PREAMBLE.contains("status to review"),
            "{TRACKER_PREAMBLE}"
        );

        // Prepended to every turn of every run for ever. A budget rather than a measurement, so
        // that growing it is a deliberate edit to this line rather than a paragraph that crept.
        // Raised from 900 when the review-workflow sentence was added, and from 1000 when the
        // checkpoint sentence was (the debug report's runs died mid-turn leaving no plan comment
        // and no commits, so nothing said how far they got) — each raise is the deliberate edit
        // the budget exists to force.
        assert!(
            TRACKER_PREAMBLE.len() < 1_300,
            "{} characters is a page, not a paragraph",
            TRACKER_PREAMBLE.len()
        );
    }

    /// **One paragraph, both harnesses** — demonstrated by running both and comparing what a role
    /// ends up being told, rather than by trusting that neither file quoted the prose itself.
    ///
    /// A role is a `.cide/agents/<name>.md` file that names its harness, and the same file can be
    /// pointed at either CLI by changing one line of front matter. The two implementations carry
    /// the paragraph through completely different machinery — a fold into `--append-system-prompt`
    /// on one side, a JSON document in `OPENCODE_CONFIG_CONTENT` on the other — so this is the
    /// only place the property *both roles are told the same thing about who owns the tracker* is
    /// actually checked.
    #[test]
    fn a_role_is_told_the_same_thing_whichever_binary_runs_it() {
        use cide_ipc::{AgentDef, AgentId};

        const BRIEF: &str = "You are the developer agent. Finish the task.";

        fn role(harness: cide_ipc::Harness) -> LoadedAgent {
            LoadedAgent {
                def: AgentDef {
                    id: AgentId("developer".into()),
                    label: "Developer".into(),
                    harness,
                    description: "Implements one task end to end.".into(),
                    system_prompt: BRIEF.into(),
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
            }
        }

        fn plan(agent: &LoadedAgent) -> RunPlan<'_> {
            RunPlan {
                run: RunId::new(),
                session: SessionId::new(),
                agent,
                cwd: PathBuf::from("/repo/.cide/worktrees/developer"),
                project: ProjectId::new(),
                task: Some(TaskId("t-14".into())),
                task_title: Some("Teach the parser about tabs".into()),
                prompt: "Do the task described above.".into(),
                hook_bin: Some(PathBuf::from("/opt/cide/cide-hook")),
                hook_sock: Some(PathBuf::from("/run/user/1000/cide-hooks-42.sock")),
                agent_sock: Some(PathBuf::from("/run/user/1000/cide-agents-42.sock")),
                theme: Theme::Dark,
                proxy: ProxyEnv::default(),
                geometry: Geometry::default(),
                claude: cide_ipc::ClaudeSettings::default(),
                // Off in the fixture, so every argv assertion below is about what the role
                // and the plan actually said; the skip default has tests of its own.
                skip_permissions: false,
            }
        }

        // claude: the value of the one `--append-system-prompt` the argv carries.
        let agent = role(cide_ipc::Harness::Claude);
        let args = ClaudeHarness
            .spawn_spec(&plan(&agent))
            .expect("a spawnable claude plan")
            .spec
            .args;
        let flag = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("a claude run carries one");
        let told_by_claude = args[flag + 1].clone();

        // opencode: the inline agent's `prompt`, out of the configuration document.
        let agent = role(cide_ipc::Harness::Opencode);
        let spec = OpencodeHarness
            .spawn_spec(&plan(&agent))
            .expect("a spawnable opencode plan")
            .spec;
        let document = spec
            .env
            .iter()
            .rev()
            .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
            .map(|(_, value)| value.clone())
            .expect("an opencode run carries one");
        let document: serde_json::Value = serde_json::from_str(&document).expect("it is json");
        let told_by_opencode = document["agent"]["developer"]["prompt"]
            .as_str()
            .expect("the inline role has a prompt")
            .to_string();

        assert_eq!(told_by_claude, told_by_opencode);
        assert_eq!(told_by_claude, format!("{BRIEF}\n\n{TRACKER_PREAMBLE}"));
    }
}
