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
use cide_ipc::{Geometry, HarnessSession, ProjectId, RunId, RunState, SessionId, TaskId, Theme};
use cide_pty::SpawnSpec;

use crate::defs::{LoadedAgent, harness_name};

pub mod claude;
pub mod codex;
pub mod opencode;
pub mod qwen;
mod render;

pub use claude::ClaudeHarness;
pub use codex::CodexHarness;
pub use opencode::{MimoHarness, OpencodeHarness};
pub use qwen::QwenHarness;

/// The name cide's MCP server is registered under, on **every** harness.
///
/// One string, because a role definition's tool list must not have to know which CLI is carrying
/// it. `opencode.rs` said this first and `claude.rs::mcp_config` spelled the literal `"cide"` by
/// hand beside it — two spellings of one fact, which is the drift this const closes.
///
/// It is *not* the name the model sees. What a server's tools are called in a model's function
/// list is the harness's own convention, and the two disagree — see [`Harness::tool_name`].
pub const SERVER: &str = "cide";

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
/// # Namespaced tool names, and why this is a template rather than prose
///
/// A bare `cide_task_get` names nothing a model can see: every CLI namespaces an MCP server's
/// tools, so the name in the function list is always built from the server's name and the tool's.
/// Telling a model to call a name that does not exist is worse than saying nothing.
///
/// **But the two CLIs build it differently, and this paragraph shipped claiming otherwise.** Claude
/// Code spells it `mcp__<server>__<tool>`; opencode spells it `<server>_<tool>`, so cide's tools
/// arrive there as `cide_cide_task_get`. Every sentence cide wrote used the Claude form, on both
/// harnesses. It was invisible for as long as it was because a capable model *guessed the mapping*
/// — a terrastrike audit counted 57 `cide_cide_task_get` calls and not one `mcp__cide__*` — which
/// is not a property to rely on under compaction or a smaller model.
///
/// So the names here are `{cide_task_get}`-style placeholders and [`tracker_preamble`] fills them
/// from [`Harness::tool_name`]. Braced placeholders rather than a substring rewrite of the bare
/// names, because the paragraph also says *the cide_task_\* tools* as prose and a replace over
/// bare names would mangle that sentence. The same idiom [`spec_preamble`] already uses.
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
     {cide_task_get} and {cide_task_list}; change a task's title, body or \
     status with {cide_task_update}, append a comment with {cide_task_comment}, \
     set the role a task is for with {cide_task_assign}, and open a new one with \
     {cide_task_create}. Those comments are how you report back: whoever dispatched you \
     reads the task, not your transcript, so a run that finishes without leaving one has reported \
     nothing, and a comment is markdown a person reads: give a report of any length structure. \
     Before you start, comment your plan on the task, and commit each coherent step of \
     the work as you finish it: a run can die mid-turn, and the plan comment plus your branch's \
     commits are the only record of how far you got. If you notice something wrong that is \
     not this task, do not fix it and do not let it go: open a task for it with \
     {cide_task_create} saying what you saw and where, then carry on with yours — widening \
     your own task makes a branch nobody can review, and a comment about it is read once and \
     lost. \
     When the work is done, set the task's status to review and comment what you did \
     and where; done is the reviewer's call, not yours.";

/// Every tracker tool cide's prose is allowed to name.
///
/// The list a template is filled from, and the list the tests iterate. A placeholder for a tool
/// that is not here survives the fill as a literal `{...}`, which every renderer's test catches —
/// deliberately, because a brace left in a model's instructions is a name it cannot call, which is
/// the failure this whole mechanism exists to remove.
pub const TRACKER_TOOLS: &[&str] = &[
    "cide_task_get",
    "cide_task_list",
    "cide_task_create",
    "cide_task_update",
    "cide_task_comment",
    "cide_task_assign",
];

/// [`TRACKER_PREAMBLE`] with every tool named the way `harness` presents it.
///
/// The one place the paragraph becomes text. Both harnesses call it rather than reading the
/// const, so a harness whose namespacing differs cannot be told the other one's spelling — see
/// [`Harness::tool_name`], which is required for the same reason.
pub fn tracker_preamble(harness: &dyn Harness) -> String {
    fill_tools(TRACKER_PREAMBLE, harness)
}

/// Replace every `{<tool>}` in `text` with the name `harness` makes of it.
///
/// Shared by the tracker and spec paragraphs and by the app's two prompts, so one rule about what
/// a placeholder means covers all four.
pub fn fill_tools(text: &str, harness: &dyn Harness) -> String {
    let mut out = text.to_string();
    for tool in TRACKER_TOOLS {
        out = out.replace(&format!("{{{tool}}}"), &harness.tool_name(tool));
    }
    out
}

/// What a run working an OpenSpec change is told, on top of [`TRACKER_PREAMBLE`]. (M28)
///
/// # It names one command and then gets out of the way
///
/// The rules below are cide's and cannot come from upstream: `openspec` knows nothing about
/// `.cide/tasks.json`, about the review hop cide watches for, or about who is allowed to archive.
/// But the *work list* is upstream's, and so is the apply instruction that belongs to whichever
/// workflow schema the project chose — `apply.tracks` is configurable, the tracked file is a glob,
/// and any summary written here would be a paraphrase that goes stale on the next `npm i -g`.
///
/// So this says its own six sentences and points at
/// `openspec instructions apply --change <c> --json`, whose answer **outranks it**. That sentence
/// is in the text deliberately: a model given two overlapping instruction sets needs to be told
/// which loses.
///
/// # Why the path is interpolated rather than the bare word
///
/// [`RunPlan::hook_bin`]'s rule, for the same failure. `openspec` is installed by `npm -g`, so it
/// lives in a Node directory that a *shell* rc file puts on `PATH` and a desktop launcher does
/// not — and `child_env::prepare_command` gives a child `toolchain::extra_dirs`, which is
/// `~/.cargo/bin` and `~/go/bin`. A bare `openspec` in this paragraph would therefore resolve to
/// nothing for exactly the users whose machine has it. cide has already located the binary; it
/// hands over the path it found.
///
/// The template holds `{CHANGE}`, `{OPENSPEC}` and one tracker tool placeholder;
/// [`spec_preamble`] fills all three.
pub const SPEC_PREAMBLE: &str = concat!(
    "This task implements the OpenSpec change {CHANGE}, whose proposal, design, task checklist and delta specs are in openspec/changes/{CHANGE}/. ",
    "Before your first edit, run `{OPENSPEC} instructions apply --change {CHANGE} --json` from the repository root: it answers with that change's context files, its schema's own apply instruction, and the checklist as data — that list is the work, in that order, and it outranks any summary of it including this one. ",
    "Tick each box off in its file as you finish that step, and commit as you go, so the checklist always says how far you got. ",
    "If implementing shows you that the proposal, the design or a delta spec was wrong, correct that file in the same commit — a spec that no longer describes the code is worse than no spec. ",
    "When every box is ticked, set the task's status to review with {cide_task_update} and comment what you did. ",
    "Never run `{OPENSPEC} archive`: archiving merges the deltas into openspec/specs/, and that is the reviewer's call once your branch has been integrated.",
);

/// The sentence added when the project installed OpenSpec's own apply workflow. (M28)
///
/// # Why it points at the command instead of replacing the paragraph with it
///
/// Because a headless run is not a chat and cide cannot verify that a slash command reached the
/// Skill tool the way it does in a pane. What it *can* do is name the thing and say it outranks
/// the paraphrase — the same move [`SPEC_PREAMBLE`] already makes for
/// `openspec instructions apply`, and for the same reason: the workflow belongs to whichever
/// schema the project chose, and any summary of it here goes stale on the next `npm i -g`.
///
/// `{APPLY}` is the resolved line, never a name — see [`RunPlan::spec_apply`].
const SPEC_APPLY_CLAUSE: &str = concat!(
    " This project installs OpenSpec's own apply workflow as {APPLY}: run it and follow what it says, ",
    "in preference to the paragraph above, which is cide's summary of the same thing.",
);

/// [`SPEC_PREAMBLE`] with its two placeholders filled.
///
/// `cli` is the absolute path cide resolved, or `None` when it could not find one — in which case
/// the bare word is used, which is still a runnable line for a user whose shell has it and is a
/// far better answer than refusing to mention the command at all.
pub fn spec_preamble(
    change: &str,
    cli: Option<&std::path::Path>,
    apply: Option<&str>,
    harness: &dyn Harness,
) -> String {
    let openspec = match cli {
        Some(path) => path.display().to_string(),
        None => "openspec".to_string(),
    };
    let mut text = fill_tools(SPEC_PREAMBLE, harness)
        .replace("{CHANGE}", change)
        .replace("{OPENSPEC}", &openspec);
    // Appended rather than interpolated into the middle, so a project without the command gets
    // byte-for-byte the paragraph it always got — the optionality claim, made structurally.
    if let Some(apply) = apply.map(str::trim).filter(|line| !line.is_empty()) {
        text.push_str(&SPEC_APPLY_CLAUSE.replace("{APPLY}", apply));
    }
    text
}

/// What a run dispatched **without a task** is told, on top of [`TRACKER_PREAMBLE`]. (M40)
///
/// # It exists to countermand one sentence of the paragraph above it
///
/// `TRACKER_PREAMBLE` tells every run to comment its plan on the task, to *commit each coherent
/// step of the work*, and to set the task to review — three instructions about a task the run
/// does not have, and one of them dangerous: a task-less run stands in the **project root**
/// (`crate::run_checkout`'s rule), which is the tree the user is working in, and a run that
/// committed "each coherent step" there would be committing onto the user's branch beside their
/// own uncommitted changes. So this says, in the same durable position, what such a run may not
/// do — commit, stage, stash, checkout, reset, anything that moves HEAD or the index — and where
/// its result goes instead: the working tree, and its own pane. Both harnesses fold it after the
/// tracker paragraph and only alongside it, because it is a correction *to* that paragraph; a run
/// told nothing about the tracker needs no correction.
///
/// # Why the system prompt and not the opening line
///
/// The opening prompt is one PTY line (`cmd::agents::opening_prompt`'s `\r` rule) and is a turn
/// old by the time the work is done; the commit rule this overrides rides the system prompt, so
/// the override has to sit where the rule does. It names no tool, so it needs no namespacing test
/// — `the_adhoc_preamble_stays_within_its_budget_and_names_no_tool` pins that as well as the size.
pub const ADHOC_PREAMBLE: &str = "This run was dispatched with no task on the board. Do only what the opening instruction asks. The directory you started in is the project's own checked-out tree, which the user is working in too — not a worktree of your own — so the paragraph above about committing each step does not apply here: do not commit, stage, stash, checkout, reset or otherwise move HEAD or the index unless the instruction explicitly asks for a commit, and leave your edits in the working tree for the user to read. Do not open a task or comment on one for this work. Whoever dispatched you reads the tree and your pane, so finish by saying plainly what you did or found.";

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

    /// What this CLI calls one of cide's MCP tools in a model's function list.
    ///
    /// `tool` is the bare vocabulary name — `cide_task_get` — and the answer is what the model can
    /// actually type. [`SERVER`] is the server every harness registers; the *namespacing* around
    /// it is per CLI and they do not agree: Claude Code, Qwen Code and Codex build
    /// `mcp__<server>__<tool>`, opencode builds `<server>_<tool>`.
    ///
    /// # Why this is required rather than defaulted
    ///
    /// [`Self::respawn_spec`] is the precedent for a defaulted method, and it earns the default by
    /// *refusing*: its body returns an error naming the harness, so an implementation that ignores
    /// it cannot produce a wrong answer, only an honest failure. A default here could not do that
    /// — it would have to return some real spelling, and whichever one it returned would be
    /// silently wrong for the other CLI.
    ///
    /// That is not hypothetical. cide shipped with the Claude spelling written into every sentence
    /// it hands an agent, on both harnesses; under opencode every one of those names was
    /// uncallable, and nothing anywhere said so. A required method makes a new harness state its
    /// convention instead of inheriting somebody else's by omission.
    fn tool_name(&self, tool: &str) -> String;

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

    /// The bytes that end a turn **already in flight**, for a harness hosting a live TUI.
    ///
    /// [`Self::deliver`] answers *how* to say something to a run; this answers how to make it
    /// stop saying something first. The two are separate questions because only one harness
    /// shape has the second one: a `Delivery::Stdin` harness is a terminal somebody could have
    /// pressed a key in, and interrupting it is pressing that key.
    ///
    /// # Why it is defaulted to `None`, and why that is an answer rather than a gap
    ///
    /// A [`Delivery::Respawn`] harness has **nothing to interrupt**. Its turn is a whole
    /// process, its follow-up is a *new child* on the same conversation, and the old child is
    /// killed by `respawn` on the way — so bytes written at it would land in a process that is
    /// about to die, in a CLI that is not reading a keyboard. `None` says exactly that, and a
    /// required method would have made each of those two write a refusal by hand, which is how
    /// one of them eventually writes a plausible-looking escape sequence instead.
    ///
    /// It lives on the harness and not at the call site because **what ends a turn is a
    /// property of the CLI**, the same argument [`Delivery`] itself is made of. Here it is one
    /// row of a `cargo test -p cide-agents` table; at the call site it would have been a claim
    /// only a running app could check.
    fn interrupt(&self) -> Option<Vec<u8>> {
        None
    }

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

    /// The **real harness**, re-opened on a conversation whose run child has ended, for a
    /// person to read and continue in a pane. (M42)
    ///
    /// Not a run: no role brief, no tracker preamble, no `CIDE_RUN`, no hook that reports to the
    /// registry. The child this describes is a pane's, spawned through
    /// `cide_app::cmd::session::session_spawn` like any pane's child, and everything that
    /// function adds for the program named here — hooks and `--settings` for a `claude`, the
    /// job watch for anything else — applies. What the harness owns is the *vocabulary*: which
    /// binary, and how that binary is told which conversation, from the directory it was filed
    /// under. The frontend never learns either CLI's flag for it.
    ///
    /// Required, not defaulted, on [`Self::tool_name`]'s argument: a harness that cannot re-open
    /// a conversation must say so in its own words rather than inherit a plausible-looking
    /// answer.
    fn continue_spec(&self, conversation: &HarnessSession) -> Result<ContinueSpec, HarnessError>;

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

    /// The model ids this harness can offer the role form, in the order it names them.
    ///
    /// A **suggestion** set and never a closed one. `AgentDraft::model` is passed through and
    /// validated against nothing — `check-settings-agents.mjs` asserts that in as many words —
    /// and this changes none of it: the form draws a menu beside a text box that still accepts
    /// anything. What it removes is the state where selecting opencode leaves a field only
    /// somebody who has memorised `provider/model` for their own providers can fill in, so every
    /// opencode role silently runs the provider default.
    ///
    /// `Err` is a **sentence for the user**, not a failure to handle: the box stays typable and
    /// the sentence becomes its hint. There is no third state — an empty `Ok` is a harness that
    /// genuinely has nothing to suggest.
    ///
    /// # Defaulted, and this one earns it by saying nothing
    ///
    /// [`Self::tool_name`]'s note explains when a default is dishonest: when it must return a
    /// real answer and whichever it returns is silently wrong for somebody. An empty list is not
    /// that — it is the honest answer for a harness nobody has taught to enumerate, and it
    /// degrades to exactly the free-text field that exists today.
    ///
    /// # It forks, which nothing else on this trait does
    ///
    /// `spawn_spec` is pure and answers with a [`SpawnSpec`] the caller runs. This runs a child
    /// itself and waits for it, so it must be called from a thread allowed to block — never a
    /// Tauri command worker directly, and never with the workspace lock held.
    ///
    /// `cwd` is the project root, and it is not ceremony: opencode merges the project's own
    /// `opencode.json`, so a project that configures a provider must be able to see that
    /// provider's models. `None` means "wherever cide is", which is the honest answer when the
    /// project's directory could not be resolved.
    /// `llm` is the user's provider configuration, resolved by the caller for
    /// [`RunPlan::claude`]'s reason — nothing in this crate reads a workspace. A harness with no
    /// provider vocabulary ignores it, which is every harness but opencode.
    ///
    /// **opencode must be given it**, and the failure if it is not is silent and confusing: the
    /// probe would list only the providers the user configured by hand, so cide's own providers
    /// would be invisible in the very dialog where a model is chosen. (M45)
    fn models(
        &self,
        cwd: Option<&std::path::Path>,
        llm: &cide_ipc::LlmSettings,
    ) -> Result<Vec<String>, String> {
        let _ = (cwd, llm);
        Ok(Vec::new())
    }

    /// Whether a provider failure this harness reports through [`Self::diagnose`] still ends the
    /// child with exit **0**. (M81)
    ///
    /// The failover gate refuses a clean exit, on opencode's measured behaviour: both probed
    /// provider failures exit 1, and a zero exit after an error line is a turn opencode retried
    /// internally and *finished* — failing that over would spend a candidate on a success. mimo,
    /// the fork, does not keep that contract. Measured on 0.1.15: a dead endpoint and a typo'd
    /// model both print their `error` event and exit 0, and its internal retries are silent (no
    /// error line until the last one), so for it the error line **is** the verdict. Without this
    /// answer a mimo pool would never fail over, with nothing logged: every candidate's death
    /// would read as a clean end.
    ///
    /// Defaulted to `false`, which is the gate as it stood for every harness before mimo.
    fn failure_exits_zero(&self) -> bool {
        false
    }

    /// What one output line says about this run's **provider**, as distinct from its state. (M45)
    ///
    /// [`Self::observe`] answers *where is this run*; this answers *did the endpoint refuse, in a
    /// way a different endpoint might not*. The two are deliberately separate channels, and that
    /// separation is load-bearing rather than tidy: an `error` line still moves nothing, and this
    /// answer is a latch the caller reads at the **exit**. A diagnosis that moved the run would be
    /// an end-of-turn declared by a line, which is run 06202dd6's bug — see `opencode`'s
    /// `observe` for what that cost.
    ///
    /// # Defaulted, and this one earns its default by saying nothing
    ///
    /// [`Self::tool_name`]'s note explains when a default is dishonest: when it must return a real
    /// answer and whichever it returns is silently wrong for somebody. `None` is not that. It is
    /// the honest answer for a CLI cide has not taught to report its provider's verdict, and it
    /// degrades to exactly the behaviour every harness had before pools existed.
    fn diagnose(&self, line: &str) -> Option<FailoverReason> {
        let _ = line;
        None
    }

    /// What one output line says about this run's **token spend**. (M80)
    ///
    /// A third reading of the same stream, beside [`Self::observe`] and [`Self::diagnose`], and
    /// separate for their reason: it moves nothing and latches nothing about the run's state.
    /// Both CLIs announce a completed step with the figures for it — opencode's `step_finish`,
    /// codex's `turn.completed` — and the answer is a value the app keeps on the run so a log
    /// line's card can say how much of the model's context this conversation is now taking.
    ///
    /// The answer is **normalised** — see [`cide_ipc::TokenUsage`], whose header carries the
    /// whole of why: the two CLIs disagree about whether a cached prompt counts inside the
    /// input or beside it, and a card drawing both raw would report the same conversation as
    /// two different sizes depending on which harness read it.
    ///
    /// # Defaulted, and the default is honest
    ///
    /// `None` is [`Self::diagnose`]'s kind of default rather than [`Self::tool_name`]'s: it is
    /// the true answer for a CLI cide cannot read a spend off — every `claude` and `qwen` run,
    /// whose output is a TUI cide never parses — and it degrades to exactly what the card did
    /// before this existed, which is say nothing about tokens.
    ///
    /// Called on the coalescer thread, once per output line, so an implementation **must** open
    /// with a substring test the way `opencode::failover` does and parse nothing in the
    /// overwhelmingly common case.
    fn usage(&self, line: &str) -> Option<cide_ipc::TokenUsage> {
        let _ = line;
        None
    }
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
    /// **The (role, task) worktree when isolation is on and the run has a task**, produced by
    /// `cide_git::worktree::ensure` before this crate was called; the project root otherwise —
    /// `crate::run_checkout` is the rule, and a run with no task lands in the root under every
    /// isolation (M40). Taken as a path rather than derived here so that nothing in
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
    /// The OpenSpec change this run's task implements, if it has one. (M28)
    ///
    /// Copied at dispatch from `Task::change`, for [`Self::task_title`]'s reason: this crate
    /// reads no disk and holds no `Task`, so everything it needs about the work arrives as a
    /// value. **Derived from the task and never from the dispatch request** — a dispatch that
    /// could name a different change than its task's would put the board and the branch in
    /// disagreement with nothing in a position to correct either.
    pub change: Option<String>,
    /// Where `openspec` is, for [`spec_preamble`] to interpolate. Two fields rather than one
    /// struct, mirroring `hook_bin`/`hook_sock`.
    pub spec_cli: Option<PathBuf>,
    /// What this project types to run OpenSpec's apply workflow — `/openspec-apply-change`, or
    /// `/opsx:apply` on a project set up by an older CLI. `None` when it has no such command.
    ///
    /// A resolved *line* and not a name, for `spec_cli`'s reason one level up: which spelling a
    /// project answers to is read off its `.claude/` directory by `cide_spec::claude`, and this
    /// crate must not carry a second opinion about it. See [`spec_preamble`] for what it does
    /// with it.
    pub spec_apply: Option<String>,
    pub hook_bin: Option<PathBuf>,
    /// The socket `cide-hook <event>` writes frames to. `CIDE_HOOK_SOCK`.
    pub hook_sock: Option<PathBuf>,
    /// The socket `cide-hook mcp` bridges to. `CIDE_AGENT_SOCK`.
    pub agent_sock: Option<PathBuf>,
    /// A FIFO the app made for this run, for a harness that reports through a JSON event
    /// file rather than through hooks — `qwen --json-file`. (M43) The app mints and creates it
    /// before the fork for every run and reads it only when the harness hands it back in
    /// [`HarnessSpawn::events`]; a harness with another state channel ignores it. `None` when
    /// the app could not make one, in which case such a harness runs unobserved rather than
    /// not at all.
    pub events_path: Option<PathBuf>,
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
    /// The provider configuration this child is given, resolved by the caller for
    /// [`Self::claude`]'s reason — nothing in this crate reads a workspace. Emitted into
    /// `OPENCODE_CONFIG_CONTENT`; inert for every other harness. (M45)
    pub llm: cide_ipc::LlmSettings,
    /// The pool candidate this run has settled on, or `None` when no pool applies. (M45)
    ///
    /// Chosen once by the caller and **carried across every respawn** — an opencode run is one
    /// process per turn, and a plan that re-chose per turn would be a conversation answered by
    /// different models with nothing recording which. Only a failover moves it.
    ///
    /// Outranks [`crate::AgentDef::model`] where it is set, and its variant outranks
    /// [`crate::LoadedAgent::effort`]; `cide_ipc::PoolEntry::variant` carries that argument.
    pub choice: Option<cide_ipc::PoolChoice>,
    /// The CLI this run actually forks. (M45)
    ///
    /// **Not `agent.def.harness`**, and the difference is the whole of the local-override layer:
    /// a role's file names the harness the *team* agreed on, and this names the one *this
    /// machine* is running it under. Every `child()` below guards on this rather than on the
    /// definition, or a redirected role would be refused by the very implementation the caller
    /// deliberately routed it to.
    ///
    /// `cide_agents::overrides::resolve` is what folds the two, and it is the only thing that
    /// may: a Claude Code subagent is pinned to `Claude` there for `defs`' stated reason.
    pub harness: cide_ipc::Harness,
    /// [`crate::config::AgentsConfig::unattended`], read fresh at this spawn. (M82)
    ///
    /// Applies only where the role, after any local override has been folded into it
    /// (`overrides::Resolved::apply`), names no `permission-mode` of its own. Carried on the plan
    /// rather than looked up here, because nothing in this crate reads disk. What each
    /// value turns into is per harness, and each harness's mapping carries its own argument:
    /// `claude` takes `auto` literally, and the CLIs with no classifier keep their promptless
    /// flag for it.
    pub unattended: crate::config::Unattended,
}

/// How the real harness is put back on a conversation, as `session_spawn` wants it. (M42)
///
/// Deliberately **not** a [`cide_pty::SpawnSpec`]: `session_spawn` builds the spec — the
/// environment, the hooks, the configured binary, the proxy — and does it the same way for
/// every pane. A second spec builder here would be a second copy of the seventy lines that
/// took three milestones to get right, drifting from the first. So this carries only what the
/// harness knows and `session_spawn` does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueSpec {
    /// The program, as `session_spawn` takes it: a bare name the OS resolves at the spawn,
    /// so a CLI that updates itself underneath a running app is found where it now is. For
    /// `claude` it is the name `program_is_claude` recognises, which is what turns the hooks
    /// and the `--settings` payload on.
    pub program: String,
    /// Arguments carrying the conversation, when the harness names it that way. Written before
    /// anything `session_spawn` appends, so nothing here may be a variadic flag.
    pub args: Vec<String>,
    /// The conversation as a cide [`SessionId`], for a harness whose conversation id *is* one.
    /// `session_spawn` hands it to `cide_claude::conversation`, which writes the `--resume`
    /// and keeps the id — so the pane's session and the run's stay the same value, and every
    /// record naming it goes on naming the right conversation. `None` when the identity
    /// travels in [`Self::args`] instead.
    pub resume: Option<SessionId>,
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
    /// The JSON event file this child writes as it works, when the harness reports that way
    /// (M43): [`RunPlan::events_path`] handed back, so the app knows to read it. Each line is
    /// one event, fed to [`Harness::observe`] as [`Observation::Line`] exactly as an opencode
    /// child's stdout is — the difference is only which stream carries it, and that the pane
    /// shows the child's own TUI rather than a rendering.
    pub events: Option<PathBuf>,
}

/// What takes a live marker off the screen, for a caller replaying a rendered stream from
/// outside this crate.
///
/// [`SessionBinding::Harness`]'s rendering draws its `▸ working…` marker under the last line of
/// a step and erases it from the *next* line's rendering — so a stream that simply stops
/// mid-step leaves the marker drawn with nothing coming to take it off. The live path never
/// notices: a child that stops has nothing after it either. A **replay** does, because
/// `cide_app::agents` re-renders a run's own log into a resumed child's mirror and follows it
/// immediately with cide's separator, which would otherwise be written under a marker claiming
/// the run is working.
///
/// One producer, here rather than a second spelling in the app: the erase goes up exactly one
/// row and is only correct while the marker it erases is the one row `render` draws.
pub const ERASE_MARKER: &str = render::ERASE_MARKER;

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
        /// Whether a person may later ask for this line's **whole event** — the app keeps such
        /// lines in its per-session ring and hands the renderer the handle it minted, which the
        /// rendering carries back to the pane as a visible token a click can resolve. (M42) A
        /// tool call and the model's own words are worth keeping; a step marker is not.
        keep: fn(&str) -> bool,
        /// One event line → display text, with the state the rendering keeps between lines
        /// (which step the run is in, whether a live marker is on screen, and the clock this
        /// line arrived at) and the ring handle for a line `keep` said yes to. `Drop` draws
        /// nothing; anything unrecognised comes back verbatim.
        render: fn(&mut RenderState, &str, Option<u64>) -> cide_pty::Rendered,
    },
}

/// What a [`SessionBinding::Harness`] rendering remembers from one line to the next. (M42)
///
/// Owned by the app's stream hook, one per session, and handed to `render` by `&mut` — the
/// function pointer itself stays stateless so the binding stays `Copy`. The first two facts are
/// what let a line-based renderer show *what the run is doing right now*: a step that has
/// started and not finished is a run that is thinking or running tools, so a dim marker sits
/// under the last line for as long as that is true, and every next line begins by erasing it.
/// The last two are the clock M62's thought row needs, and they are here rather than in a
/// parameter because the render functions must stay pure functions of `(state, line, handle)` —
/// that is what lets a test assert an exact duration.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RenderState {
    /// A `step_start` has been seen and its `step_finish` has not.
    pub in_step: bool,
    /// The marker row is the last thing on screen, so the next rendering must erase it first.
    pub marker: bool,
    /// The wall clock, in Unix milliseconds, as *this* line reached cide — written by the caller
    /// immediately before `render` and read by nothing else. (M62)
    ///
    /// `Option`, emphatically, and not a bare `u64`: this struct derives `Default`, a `0` there
    /// is 1970, and a duration measured against 1970 is a confident number about nothing —
    /// which is the one shape [`render::thought_line`] exists to refuse. A caller that does not
    /// set it leaves every duration unknown, which draws no duration at all.
    pub now_unix_ms: Option<u64>,
    /// When the previous line that actually drew something arrived, so the gap to this one can
    /// be measured for a harness that states no clock. Maintained by `render::compose`, which
    /// is the single funnel both harnesses' renderings pass through. (M62)
    pub last_line_unix_ms: Option<u64>,
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

/// Esc: the one byte that ends a turn in a TUI harness. See [`Harness::interrupt`].
///
/// Here rather than in each harness module because two of them answer with it and a third
/// would, and a byte spelled twice is a byte one of the copies eventually gets wrong — the
/// difference between `0x1b` and `0x1B` is nothing, the difference between one of them and
/// `0x03` is a `SIGINT` that kills the CLI instead of ending its turn.
pub const ESC: u8 = 0x1b;

/// How a follow-up reaches a run that is already alive. See the module header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Write these bytes into the run's PTY.
    Stdin(Vec<u8>),
    /// This CLI cannot be spoken to. Start a fresh child that continues the same conversation.
    Respawn,
}

/// Why a turn died in a way another provider might survive. (M45)
///
/// Deliberately three named cases and not "an error happened". A pool exists to route *around*
/// something, and each of these is a different something a different candidate plausibly fixes;
/// anything not on this list is a failure another candidate would repeat, and repeating it once
/// per candidate is how a typo'd model id burns a whole pool and reports the last one's error as
/// the cause of death.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverReason {
    /// Out of quota for this credential right now — HTTP 429, or a billing refusal.
    RateLimited,
    /// The endpoint could not be reached, or answered in a way the CLI itself calls retryable.
    /// The local case above all: LM Studio not running, llama-server down, ollama not started.
    Unreachable,
    /// The provider refused the credentials — 401, 403, or the CLI's own auth error. An expired
    /// subscription OAuth lands here, which is exactly a case another candidate survives.
    Auth,
}

impl FailoverReason {
    /// Three or four words for the run's row. Lower case: it is spliced into a sentence.
    #[must_use]
    pub fn phrase(self) -> &'static str {
        match self {
            Self::RateLimited => "rate limited",
            Self::Unreachable => "unreachable",
            Self::Auth => "refused the credential",
        }
    }
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
    /// A conversation id this harness cannot re-open: an empty one, or — for `claude`, whose
    /// ids are cide's own uuids — one that is not a uuid at all. Refused rather than passed
    /// through, because the CLI's answer would be a pane that fails after it opened. (M42)
    NotAConversation {
        harness: cide_ipc::Harness,
        id: String,
    },
    /// The definition asks for something this harness has no flag for — an `effort`, a
    /// `permission-mode` with no approval-mode counterpart. Refused rather than dropped: a
    /// role whose author wrote a restriction that silently did not apply is the failure that
    /// looks like success. (M43)
    NoEquivalent {
        harness: cide_ipc::Harness,
        what: String,
    },
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
            Self::NotAConversation { harness, id } => write!(
                f,
                "“{id}” is not a conversation the “{}” harness can re-open",
                harness_name(*harness)
            ),
            Self::NoEquivalent { harness, what } => write!(
                f,
                "the “{}” harness has no equivalent of {what}; drop it from the definition or \
                 run this role on another harness",
                harness_name(*harness)
            ),
        }
    }
}

impl std::error::Error for HarnessError {}

/// The one instance of each harness. Unit structs, so this costs nothing to hold.
static CLAUDE: ClaudeHarness = ClaudeHarness;
static OPENCODE: OpencodeHarness = OpencodeHarness;
static QWEN: QwenHarness = QwenHarness;
static CODEX: CodexHarness = CodexHarness;
static MIMO: MimoHarness = MimoHarness;

/// The registry itself, as a `const` rather than built in [`registry`].
///
/// Written this way because the obvious `&[&CLAUDE]` inside the function is a reference to a
/// temporary and will not compile: const-promotion does not reach through the unsizing coercion
/// to `&dyn Harness`. A `const` gives the slice `'static` storage, which is what the signature
/// promises.
const REGISTRY: &[&dyn Harness] = &[&CLAUDE, &OPENCODE, &QWEN, &CODEX, &MIMO];

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

    /// Exactly the harnesses that can be typed into have something to interrupt, and it is Esc.
    ///
    /// Written against [`Self::deliver`]'s answer rather than against a list of names, because
    /// the two are one fact: a harness cide can send a follow-up to by writing bytes is a
    /// harness cide can interrupt by writing bytes, and one that needs a whole new child has
    /// nothing to interrupt — its old child is killed on the way. A harness that answered
    /// `Stdin` and `None`, or `Respawn` and `Some`, would be a stop that quietly could not stop
    /// or bytes written at a process about to die.
    #[test]
    fn a_harness_can_be_interrupted_exactly_when_it_can_be_typed_into() {
        for harness in registry() {
            match (harness.deliver("carry on"), harness.interrupt()) {
                (Delivery::Stdin(_), Some(bytes)) => assert_eq!(
                    bytes,
                    vec![ESC],
                    "{:?} interrupts with something other than Esc",
                    harness.kind()
                ),
                (Delivery::Respawn, None) => {}
                (delivery, interrupt) => panic!(
                    "{:?} answers {delivery:?} to a follow-up and {interrupt:?} to an interrupt, \
                     which cannot both be true",
                    harness.kind()
                ),
            }
        }
    }

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
        const EVERY: &[cide_ipc::Harness] = &[
            cide_ipc::Harness::Claude,
            cide_ipc::Harness::Opencode,
            cide_ipc::Harness::Qwen,
            cide_ipc::Harness::Codex,
            cide_ipc::Harness::Mimo,
        ];

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

    /// The spec paragraph names the change, the resolved binary and the six rules. (M28)
    #[test]
    fn the_spec_preamble_names_the_change_the_cli_and_the_rules() {
        let cli = std::path::Path::new("/home/u/.nvm/versions/node/v22.21.0/bin/openspec");
        let rendered = spec_preamble("m28-openspec", Some(cli), None, &ClaudeHarness);

        // **The assertion that actually catches a rename.** A renamed placeholder leaves its
        // braces in the text, and a model reading `openspec/changes/{CHANGE}/` would go looking
        // for a directory with braces in its name — a failure with nothing in a log to find.
        assert!(
            !rendered.contains('{'),
            "a placeholder survived into the rendered text: {rendered}"
        );

        assert!(
            rendered.contains("openspec/changes/m28-openspec/"),
            "{rendered}"
        );
        assert!(
            rendered
                .contains("/home/u/.nvm/versions/node/v22.21.0/bin/openspec instructions apply"),
            "the *resolved* path is named, not the bare word — a child's PATH is not a \
             shell's: {rendered}"
        );
        assert!(
            rendered.contains("--change m28-openspec --json"),
            "{rendered}"
        );
        assert!(
            rendered.contains("outranks any summary"),
            "a model given two overlapping instruction sets has to be told which loses: \
             {rendered}"
        );
        assert!(
            rendered.contains("status to review with mcp__cide__cide_task_update"),
            "the hand-back is the hop cide watches for: {rendered}"
        );

        // The prohibition has to attach to archiving. Asserted on word order rather than on a
        // byte distance: the interpolated path is as long as the user's home directory, so a
        // "within N characters" rule fails on a machine with a deep nvm install and passes on a
        // shallow one — which is a test that reports on the wrong thing.
        let never = rendered
            .find("Never run")
            .expect("the prohibition is there");
        assert!(rendered[never..].starts_with("Never run `"), "{rendered}");
        assert!(
            rendered[never..].contains("archive`: archiving merges the deltas"),
            "the prohibition names archiving and says what it does: {rendered}"
        );

        // And the prose is prose: a `\`-continued literal keeps the source's own indentation,
        // which is how this const first shipped with six-space runs inside sentences.
        assert!(!rendered.contains("  "), "{rendered:?}");

        // With no binary found, the bare word — still a runnable line for a user whose shell
        // has it, and a far better answer than not naming the command at all.
        let bare = spec_preamble("x", None, None, &ClaudeHarness);
        assert!(bare.contains("`openspec instructions apply"), "{bare}");
        assert!(!bare.contains('{'), "{bare}");

        // A project that installed OpenSpec's own apply workflow gets one sentence more, naming
        // the *line* that project answers to — `/opsx:apply` on a project set up by an older
        // CLI, which is why this is a resolved string and not a name this crate spells. It says
        // which of the two loses, for the reason the paragraph above already says it once.
        let with_skill = spec_preamble("x", None, Some("/openspec-apply-change"), &ClaudeHarness);
        assert!(
            with_skill.starts_with(&bare),
            "appended, never woven in: {with_skill}"
        );
        assert!(
            with_skill.contains("/openspec-apply-change"),
            "{with_skill}"
        );
        assert!(
            with_skill.contains("in preference to the paragraph above"),
            "two overlapping instruction sets, and the model is told which loses: {with_skill}"
        );
        assert!(!with_skill.contains('{'), "{with_skill}");
        assert!(!with_skill.contains("  "), "{with_skill:?}");
        assert_eq!(
            spec_preamble("x", None, Some("   "), &ClaudeHarness),
            bare,
            "a blank line is no command at all, not a sentence naming nothing"
        );
    }

    /// A budget, not a measurement — [`TRACKER_PREAMBLE`]'s rule, applied to the paragraph that
    /// now sits beside it. Two of these in front of a role's own prompt is already most of what
    /// a short role says about itself.
    #[test]
    fn the_spec_preamble_stays_within_its_budget() {
        assert!(
            SPEC_PREAMBLE.len() < 1_200,
            "the spec preamble has grown to {} bytes",
            SPEC_PREAMBLE.len()
        );
    }

    /// Every name in the paragraph is a name the model can actually call — **on every harness**.
    ///
    /// This test used to assert `mcp__cide__cide_task_*` as though that were a property of the
    /// paragraph. It is a property of *Claude Code*: opencode namespaces the same server's tools
    /// as `cide_cide_task_get`, so on that harness every name this test was green about was
    /// uncallable. A terrastrike audit counted 57 `cide_cide_task_get` calls against zero
    /// `mcp__cide__*` — a capable model reverse-engineering the mapping, which is not a thing to
    /// rely on.
    ///
    /// So it runs over the registry rather than over one spelling. The count comparison is still
    /// the assertion that matters and is now stronger: it fails on the *next* mention somebody
    /// adds without a placeholder, on whichever harness, rather than on the six that are here.
    #[test]
    fn the_tracker_preamble_names_the_tools_in_the_spelling_they_arrive_in() {
        for harness in registry() {
            let rendered = tracker_preamble(*harness);
            let kind = harness.kind();

            for tool in TRACKER_TOOLS {
                let name = harness.tool_name(tool);
                assert!(
                    rendered.contains(&name),
                    "{kind:?} does not name {name}: {rendered}"
                );
            }

            // Nothing was left as a literal `{cide_task_…}`: a brace that survives the fill is a
            // name no model can call, which is the whole failure this mechanism removes.
            assert!(
                !rendered.contains('{'),
                "{kind:?} left a placeholder unfilled: {rendered}"
            );

            // Every mention is namespaced *this harness's way*. `tool_name` is asked for the
            // prefix rather than it being spelled here, so the two cannot drift.
            let namespaced = harness.tool_name("cide_task_");
            assert_eq!(
                rendered.matches("cide_task_").count(),
                rendered.matches(&namespaced).count(),
                "{kind:?} has a bare mention: {rendered}"
            );
        }

        // And the two spellings really are different, so the loop above is testing something. A
        // regression that made `tool_name` answer the same string everywhere would otherwise
        // leave every assertion here green.
        assert_ne!(
            ClaudeHarness.tool_name("cide_task_get"),
            OpencodeHarness.tool_name("cide_task_get"),
            "the harnesses namespace differently; that is why this is a trait method"
        );

        // The template itself keeps no hard-coded namespace: a name written out in full here
        // would be right on one harness and wrong on the other, silently.
        assert!(
            !TRACKER_PREAMBLE.contains("mcp__"),
            "the template names no harness's spelling: {TRACKER_PREAMBLE}"
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

        // **A run that finds something outside its task files it**, rather than fixing it (a
        // branch nobody can review) or mentioning it in passing (a comment read once by the
        // reviewer and never again). Asserted because it is one sentence in a paragraph that
        // has a budget, and the budget is the pressure that would quietly delete it. (M79)
        assert!(
            TRACKER_PREAMBLE.contains("not this task"),
            "{TRACKER_PREAMBLE}"
        );

        // Prepended to every turn of every run for ever. A budget rather than a measurement, so
        // that growing it is a deliberate edit to this line rather than a paragraph that crept.
        // Raised from 900 when the review-workflow sentence was added, from 1000 when the
        // checkpoint sentence was (the debug report's runs died mid-turn leaving no plan comment
        // and no commits, so nothing said how far they got), and from 1300 in M79 for the
        // out-of-scope sentence above — which earns its ~160 characters because the alternative
        // is the two failures it names, both of which cost a review each time they happen. Each
        // raise is the deliberate edit the budget exists to force.
        assert!(
            TRACKER_PREAMBLE.len() < 1_500,
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
    const BRIEF: &str = "You are the developer agent. Finish the task.";

    /// What each binary would tell one role: claude's one `--append-system-prompt` value, the
    /// inline agent's `prompt` out of opencode's configuration document, and codex's
    /// `developer_instructions` override (M44). `task` is the only knob, because the paragraphs
    /// a run is told differ on exactly that (M40).
    fn told_by_all(task: Option<&TaskId>) -> (String, String, String) {
        use cide_ipc::{AgentDef, AgentId};

        fn role(harness: cide_ipc::Harness) -> LoadedAgent {
            LoadedAgent {
                def: AgentDef {
                    id: AgentId("developer".into()),
                    label: "Developer".into(),
                    scope: cide_ipc::agents::AgentScope::Project,
                    harness,
                    description: "Implements one task end to end.".into(),
                    system_prompt: BRIEF.into(),
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

        fn plan<'a>(agent: &'a LoadedAgent, task: Option<&TaskId>) -> RunPlan<'a> {
            RunPlan {
                run: RunId::new(),
                session: SessionId::new(),
                agent,
                cwd: PathBuf::from("/repo/.cide/worktrees/developer"),
                project: ProjectId::new(),
                task: task.cloned(),
                task_title: task.map(|_| "Teach the parser about tabs".into()),
                // No change in the fixture, so every argv assertion below is about the two
                // paragraphs a run has always had; the third has its own fixture beside it.
                change: None,
                spec_cli: None,
                spec_apply: None,
                prompt: "Do the task described above.".into(),
                hook_bin: Some(PathBuf::from("/opt/cide/cide-hook")),
                hook_sock: Some(PathBuf::from("/run/user/1000/cide-hooks-42.sock")),
                agent_sock: Some(PathBuf::from("/run/user/1000/cide-agents-42.sock")),
                events_path: None,
                theme: Theme::Dark,
                proxy: ProxyEnv::default(),
                geometry: Geometry::default(),
                claude: cide_ipc::ClaudeSettings::default(),
                llm: cide_ipc::LlmSettings::default(),
                choice: None,
                harness: agent.def.harness,
                // Off in the fixture, so every argv assertion below is about what the role
                // and the plan actually said; the skip default has tests of its own.
                unattended: crate::config::Unattended::Ask,
            }
        }

        // claude: the value of the one `--append-system-prompt` the argv carries.
        let agent = role(cide_ipc::Harness::Claude);
        let args = ClaudeHarness
            .spawn_spec(&plan(&agent, task))
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
            .spawn_spec(&plan(&agent, task))
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

        // codex: the brief the `-c developer_instructions=` override carries, before encoding.
        let agent = role(cide_ipc::Harness::Codex);
        let told_by_codex = codex::developer_brief(&plan(&agent, task));

        (told_by_claude, told_by_opencode, told_by_codex)
    }

    /// `rendered` with every tool name turned back into the placeholder it came from.
    ///
    /// The inverse of [`fill_tools`], and what lets the two parity tests below go on making their
    /// claim now that the claim has changed shape. They used to assert the two binaries produce
    /// **byte-identical** paragraphs, which was true and is now deliberately false: the same
    /// sentence names `mcp__cide__cide_task_get` on one CLI and `cide_cide_task_get` on the other.
    ///
    /// Comparing normalised forms keeps the part that was ever worth asserting — *one role gets
    /// one brief, in one order, whichever binary carries it* — while allowing the one difference
    /// that is the point of `tool_name`. A test that simply dropped the comparison would have
    /// stopped defending against the copy-paste divergence `TRACKER_PREAMBLE`'s header describes.
    fn unfill_tools(rendered: &str, harness: &dyn Harness) -> String {
        let mut out = rendered.to_string();
        for tool in TRACKER_TOOLS {
            out = out.replace(&harness.tool_name(tool), &format!("{{{tool}}}"));
        }
        out
    }

    #[test]
    fn a_role_is_told_the_same_thing_whichever_binary_runs_it() {
        let (told_by_claude, told_by_opencode, told_by_codex) =
            told_by_all(Some(&TaskId("t-14".into())));

        // The one licensed difference: the tool names, and nothing else.
        assert_eq!(
            unfill_tools(&told_by_claude, &ClaudeHarness),
            unfill_tools(&told_by_opencode, &OpencodeHarness),
        );
        assert_eq!(
            unfill_tools(&told_by_claude, &ClaudeHarness),
            unfill_tools(&told_by_codex, &CodexHarness),
        );
        assert_eq!(
            unfill_tools(&told_by_claude, &ClaudeHarness),
            format!("{BRIEF}\n\n{TRACKER_PREAMBLE}")
        );

        // And each really is in its own spelling — otherwise the normalisation above could be
        // hiding a harness that silently inherited the other's names, which is the bug.
        assert!(
            told_by_claude.contains("mcp__cide__cide_task_get"),
            "{told_by_claude}"
        );
        assert!(
            told_by_opencode.contains("cide_cide_task_get"),
            "{told_by_opencode}"
        );
        assert!(!told_by_opencode.contains("mcp__"), "{told_by_opencode}");
        assert!(
            told_by_codex.contains("mcp__cide__cide_task_get"),
            "{told_by_codex}"
        );
    }

    /// The task-less twin (M40): the correction to the tracker paragraph is the third paragraph,
    /// after it, on both binaries — asserted from [`ADHOC_PREAMBLE`] itself, never a quoted copy.
    #[test]
    fn an_adhoc_run_is_told_the_same_thing_whichever_binary_runs_it() {
        let (told_by_claude, told_by_opencode, told_by_codex) = told_by_all(None);
        assert_eq!(
            unfill_tools(&told_by_claude, &ClaudeHarness),
            unfill_tools(&told_by_opencode, &OpencodeHarness),
        );
        assert_eq!(
            unfill_tools(&told_by_claude, &ClaudeHarness),
            unfill_tools(&told_by_codex, &CodexHarness),
        );
        assert_eq!(
            unfill_tools(&told_by_claude, &ClaudeHarness),
            format!("{BRIEF}\n\n{TRACKER_PREAMBLE}\n\n{ADHOC_PREAMBLE}")
        );
    }

    /// A budget ([`TRACKER_PREAMBLE`]'s rule), and the two facts the paragraph exists for: it
    /// forbids committing in the user's tree, and it names no tool — so a run whose bridge is
    /// absent could read it harmlessly, even though the fold gates it with the tracker paragraph.
    #[test]
    fn the_adhoc_preamble_stays_within_its_budget_and_names_no_tool() {
        assert!(
            ADHOC_PREAMBLE.len() < 800,
            "the ad-hoc preamble has grown to {} bytes",
            ADHOC_PREAMBLE.len()
        );
        assert!(ADHOC_PREAMBLE.contains("do not commit"), "{ADHOC_PREAMBLE}");
        assert!(
            ADHOC_PREAMBLE.contains("not a worktree"),
            "{ADHOC_PREAMBLE}"
        );
        assert!(
            !ADHOC_PREAMBLE.contains("mcp__") && !ADHOC_PREAMBLE.contains("cide_task_"),
            "a tool named here would need the namespacing test: {ADHOC_PREAMBLE}"
        );
    }
}
