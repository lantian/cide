//! The subagent run registry: what has been dispatched, what is running, what is waiting. (M18)
//!
//! # A run is a session with a queue position in front of it
//!
//! Everything below the spawn is machinery this application already had. A run's child is a
//! [`cide_pty::PtySession`] built from the same [`cide_pty::SpawnSpec`] a pane builds, filed in
//! the same [`SessionRegistry`], watched by the same [`crate::lifecycle::watch_for_exit`], and
//! wound down by the same shutdown ladder. **There is no second process-hosting path**, which is
//! what makes the orphan sweep, the close confirm and `lifecycle::shutdown` cover subagent runs
//! without a second implementation of any of them — and it is why a `LiveRun` stores a
//! [`SessionId`] and never an `Arc<PtySession>`. Two owners of one child is how a registry
//! outlives a process: the pane registry would reap it while this one still handed the handle out.
//!
//! What is genuinely new is the part in front of the spawn — who may run, when, and where — and
//! that is the whole of this module.
//!
//! # The state, and the one lock over it
//!
//! One [`parking_lot::Mutex`] over [`Inner`], which holds the runs and the per-agent queues
//! together. Not a `DashMap` per field, and the reason is that every decision here spans both
//! maps: admitting a run reads a queue, writes a slot count and writes a run's state, and a
//! reader that saw two of those three would see an agent holding a slot for a run that is still
//! queued — or worse, admit a second run into the same worktree. `DashMap` gives per-key
//! atomicity and the invariant is not per-key.
//!
//! **The lock is never held across a spawn, a disk read or an emit.** Every public method takes
//! it, decides, drops it, and only then acts; the shape is `lock → decide → unlock → act`
//! throughout. That is what makes it safe to take this lock from the hook applier thread, whose
//! ordering guarantee is load-bearing (see [`note_hook`]).
//!
//! # Concurrency and the checkout are the same lock
//!
//! With [`cide_agents::Isolation::Worktree`] a run on a task has its own checkout —
//! `.cide/worktrees/<role>-<task>` on branch `cide/<role>-<task>`, `cide_agents::run_checkout`
//! being the rule — and two children in one checkout would be two processes editing one tree
//! with nothing arbitrating between them. So the checkout is stamped on the run at dispatch
//! ([`DispatchSpec::checkout`]) and admission holds a run whose checkout is occupied; the role's
//! `max-concurrent` and the project's cap are recorded the same way ([`LiveRun::agent_limit`])
//! rather than recomputed later. A run with **no task** stands in the project root and has no
//! checkout to hold (M40): nothing serialises two of them, and nothing may wind an idle one down
//! to reclaim a directory the user is standing in too. A dispatch for a busy agent **queues**; it
//! does not spawn.
//!
//! # Slot accounting is held from dispatch, never derived from the phase
//!
//! **This is the most important correctness note in the file.**
//!
//! [`cide_ipc::RunState::Idle`]'s doc records the window: `SessionStart` reaches
//! `SessionState::Idle`, so a run is briefly *idle* between its child reporting in and its
//! opening prompt being processed — a gap of milliseconds, since those bytes are typed at spawn,
//! but a real one. A slot count recomputed from the current phase on every observation would see
//! a just-dispatched run holding nothing and start a second run of the same role into the same
//! worktree.
//!
//! So a slot is **taken at admission and released exactly once**, when the run leaves the working
//! set, and "leaving" is a fact about the run's *history*:
//!
//! * an **edge** from `Running` or `AwaitingPermission` into `Idle` — the turn was handed back;
//! * `Finished` or `Failed` — the run is over.
//!
//! `Starting → Idle` is not that edge, which is precisely the `SessionStart` window closed. The
//! release is also idempotent ([`LiveRun::slot`]), so a run that goes idle, is given another turn
//! and goes idle again cannot release a slot twice and let two runs of one role start.
//!
//! # Liveness is the hooks that already exist
//!
//! A run's child is spawned with `CIDE_SESSION = <its SessionId>`, so its hook frames arrive on
//! the same socket, keyed the same way, and are decided by the same `cide_claude::next_state` as
//! a pane's. There is no new state machine and no polling: [`note_hook`] is one hop off
//! `hooks::apply`, and [`AgentRegistry::observe`] maps the frame through
//! [`cide_agents::Harness::observe`], which owns the two refusals a caller could not enforce (a
//! `Paused` run is never thawed by a frame in flight; a late frame never resurrects a finished
//! one).
//!
//! # Pause is a mark and a signal, in that order
//!
//! [`AgentRegistry::pause`] closes the dispatch queue **and** freezes the children — two halves
//! of one gesture, which is why `AgentRoster::Ready::dispatching` is on the wire beside the run
//! states rather than derived from them.
//!
//! **The mark happens first and the `SIGSTOP` second, and swapping them is a real bug.** Between
//! a freeze and a mark there is a window in which the queue is still open over a stopped child:
//! the follow-up it writes into that child's PTY *succeeds* — into the kernel's buffer, which a
//! stopped process is perfectly able to be written to — and is read on resume, out of order with
//! whatever the model was mid-turn on. Nothing anywhere would report it. So [`take_freezes`]
//! takes the lock, writes every `Paused` and closes the queue, and returns the sessions to
//! signal; the signal happens to its return value, outside the lock.
//!
//! Resume is the mirror, for the mirror-image reason: `SIGCONT` first, and only then the marks
//! cleared and the queue reopened, so no admission can reach a child that is still frozen.
//!
//! # The stale turn is a suspicion, and stays one
//!
//! cide cannot see the model request. It can see that a process was stopped and continued, which
//! is not the same fact, so a freeze that outlasts an HTTP idle timeout is *grounds to ask* and
//! never grounds to re-dispatch: a re-send would double-bill a turn that in fact survived.
//! [`AgentRun::stale_turn`] is therefore an offer, cleared by `agents_retry_turn` (which spends
//! the quota) or by `agents_ack_stale_turn` (which does not) — two commands rather than one with
//! a boolean, because a boolean between a user and a call that spends their money is the wrong
//! shape.
//!
//! The arithmetic is in [`freeze_may_have_killed_the_turn`], over wall-clock milliseconds
//! injected by the caller — wall clock and not [`Instant`], because the question is whether a
//! *timeout* elapsed, and a laptop suspended for an hour with a frozen child in it has elapsed
//! one however little the monotonic clock moved.
//!
//! # Why the emit is coalesced
//!
//! `cide://agents-changed` carries a whole roster, and a run produces a transition per tool call
//! plus a statusline frame about once a second. Four runs and a queue turning over is a burst,
//! and every emit crosses into every window. [`Coalescer`] is `crate::lsp`'s shape — a 120 ms
//! trailing debounce with a 1 s ceiling — for `crate::lsp`'s reason: the debounce collapses the
//! burst, and the ceiling means a continuously-working run still updates about once a second
//! rather than never.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cide_agents::tools::Stopped;
use cide_agents::{Delivery, Observation, RenderState, RunPlan, SessionBinding};
use cide_claude::HookFrame;
use cide_core::CoreError;
use cide_ipc::{
    AgentId, AgentRun, ClaudeSettings, Geometry, Harness, HarnessSession, PaneRole, ProjectId,
    ProxySettings, RunId, RunNotify, RunOpen, RunState, SessionId, SessionState, TaskId, Theme,
};
use cide_pty::PtySession;
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use crate::event_tap;
use crate::state::SessionRegistry;

type Result<T> = std::result::Result<T, CoreError>;

/// Trailing debounce on `cide://agents-changed`. See the module header.
const COALESCE: Duration = Duration::from_millis(120);
/// And the ceiling, so a run that never stops working still updates about once a second.
const COALESCE_CEILING: Duration = Duration::from_secs(1);
/// How often the flusher wakes while a burst is settling. Short next to [`COALESCE`], so the
/// emit lands within a tick of becoming due rather than a whole debounce late.
const COALESCE_TICK: Duration = Duration::from_millis(20);

/// How many *finished* runs a project keeps.
///
/// Finished runs are kept at all because [`RunId`]'s doc says why: a run goes on meaning
/// something after its child dies — its exit code, the task it was dispatched against — and the
/// row a user most wants to read is the one that just failed. They are capped because nothing
/// else drops them: a long session dispatching every few minutes would otherwise grow this map
/// for the life of the process, and every roster emit copies it into every window.
const RECENT_KEPT: usize = 50;

/// How long a freeze has to last before the turn under it is worth suspecting.
///
/// A minute, and the number is a floor rather than a measurement: nothing in this process knows
/// the timeout of the model endpoint, of the user's proxy, or of whatever sits between them.
/// **Corporate users will meet this far more often than anybody else** — this codebase carries a
/// whole proxy subsystem, and a proxy's idle timeout is normally much shorter than the API's — so
/// the value is deliberately low enough to catch those and high enough that a pause taken to read
/// a diff never raises anything.
const STALE_FREEZE_MS: u64 = 60_000;

/// How long a thawed run is watched for any sign of life before its turn is presumed dead.
///
/// Twenty seconds, because the evidence is cheap and immediate when it exists: a `claude` whose
/// request survived repaints its spinner within a frame or two of `SIGCONT`, and one whose
/// request died prints its error just as fast. What this length is really buying is the case
/// where neither happens — a socket the peer closed silently, where the CLI itself is waiting on
/// a read that will never return — and for that, waiting longer only delays the offer.
const THAW_WATCH: Duration = Duration::from_secs(20);

/// Which agent, in which project. Runs are per project, and two projects may define a role with
/// the same name — a shared key would make one project's queue block the other's.
type AgentKey = (ProjectId, AgentId);

/// One dispatched execution, for the whole of its life in this process.
///
/// The session is an id and **not** an `Arc<PtySession>`: a pane holds an id and asks
/// [`SessionRegistry`], and so does this. See the module header.
#[derive(Debug, Clone)]
struct LiveRun {
    run: RunId,
    agent: AgentId,
    /// Copied at dispatch, for [`cide_ipc::AgentRun::agent_label`]'s reason: a history row must
    /// still read correctly after the role has been renamed or its definition deleted.
    agent_label: String,
    harness: Harness,
    project: ProjectId,
    /// `None` until the child is about to exist. A queued run has no session, which is exactly
    /// why the panel withholds the Open action rather than drawing it disabled.
    session: Option<SessionId>,
    task: Option<TaskId>,
    /// Copied at dispatch like the label, and for the same reason: it ends up in the child's
    /// terminal title and in `/resume`, which must still read correctly after a rename.
    task_title: Option<String>,
    /// The OpenSpec change this run is working, copied at dispatch. (M28) Kept beside the title
    /// so a **respawn** tells the new child the same thing the first one was told — a resumed run
    /// that lost its change would go back to working from the task body alone.
    change: Option<String>,
    state: RunState,
    started_unix_ms: u64,
    /// Milliseconds this run has worked, over its **closed** working intervals.
    ///
    /// Maintained by [`move_to`] and nothing else, which is the whole of the correctness here:
    /// it is the only thing in this process that assigns [`Self::state`], so it is the only
    /// thing that can see the transition an interval opens or closes on. See
    /// [`RunState::counts_as_work`] for which states count, and [`cide_ipc::AgentRun::worked_ms`]
    /// for why a run needs this clock beside [`Self::started_unix_ms`] at all.
    worked_ms: u64,
    /// When the open working interval began, or `None` when this run is not working.
    ///
    /// `Some` in exactly the states [`RunState::counts_as_work`] admits. Not persisted: a
    /// restored run is `Interrupted`, which is not working.
    working_since_unix_ms: Option<u64>,
    /// The last prompt delivered — the opening one today, a follow-up once the queue can send
    /// them. Kept because the retry a frozen turn offers has to re-send *this* text: a retry
    /// that recomposed the prompt from the task would silently drop whatever extra instruction
    /// the dispatch carried.
    prompt: String,
    /// One line of context for the row: what this run is waiting on, or which worktree it holds.
    note: Option<String>,
    /// Whether this run is currently holding a slot. The latch that makes release idempotent —
    /// see the module header's slot rule.
    slot: bool,
    /// The role's effective concurrency **as it was when this run was dispatched**, already
    /// clamped by [`cide_agents::effective_max_concurrent`]. Recorded rather than recomputed so
    /// that admission needs no disk read under the lock, and so a config edited mid-queue cannot
    /// release a slot that was never taken.
    agent_limit: u16,
    /// The project-wide ceiling, recorded for the same reason.
    project_limit: u16,
    /// The worktree this run stands in, recorded at dispatch for the same reason as the limits.
    /// See [`DispatchSpec::checkout`]; [`admit_a_pass`]'s checkout gate is what reads it.
    checkout: Option<String>,
    /// Where this run's turn endings are announced, as the dispatch chose. (M40) Read at the
    /// edge by `agent_rpc::note_run_over`, carried on the wire so the run list can show
    /// `quiet`, and through the snapshot so a resumed run keeps its address.
    notify: RunNotify,
    /// See [`RunPurpose`]. Never persisted: a review stands in a checkout that dies with the
    /// process, so the snapshot leaves review runs out entirely.
    purpose: RunPurpose,
    /// The directory the child was started in — the worktree, or the root under shared
    /// isolation — once it has been. `None` while queued: nothing has forked, so nothing can
    /// have connected to the agent socket and asked. (M39)
    ///
    /// Not on [`cide_ipc::AgentRun`]: the panel has no use for an absolute path, and the one
    /// consumer is `agent_rpc`, which resolves a relative path in `cide_task_attach` against
    /// it. [`Self::note`] carries the same fact as a sentence for a person; this is the value.
    cwd: Option<std::path::PathBuf>,
    /// The ordered candidates this run may fall down, stamped at **dispatch**. (M45, moved from
    /// the first fork when entries gained a running limit — admission chooses the entry now.)
    ///
    /// Stamped once and never re-read, for `agent_limit`'s reason one field up: a pool edited
    /// under a live run would mean the candidate a failover advances *to* is not the one the user
    /// arranged. It is also what makes [`Self::pool_index`] mean anything — an index into a list
    /// that changes underneath it is not a position.
    ///
    /// Empty for every harness but opencode, and for an opencode role with no pool override, in
    /// which case the role's own `model:` stands and nothing here does anything.
    pool: Vec<cide_ipc::PoolEntry>,
    /// Which element of [`Self::pool`] this run is on.
    ///
    /// **Monotonic**: set by admission to the first entry at or after it with room (so `0` for a
    /// fresh run whose first choice is free), advanced on a qualifying provider failure, and never
    /// decreased by anything but a person's Resume onto changed settings ([`restamp`]). That single property is the whole of "sticky for the run" *and* the
    /// infinite-loop guard — a run can fail over at most `pool.len() - 1` times for its entire
    /// life, across any number of turns, whatever else goes wrong.
    pool_index: usize,
    /// The last provider failure **this child** reported, taken (not read) by the exit handler.
    ///
    /// Written by the stream hook from a classified `error` line and consumed by
    /// [`AgentRegistry::plan_failover`]. A latch and not a state: [`RunState`] says what the
    /// *run* is doing and this says what its provider said, and the two are answered by different
    /// observations milliseconds apart. Taken rather than read so a stale verdict cannot fail the
    /// *next* candidate over on its own clean exit.
    provider_failure: Option<cide_agents::FailoverReason>,
    /// Every candidate this run has spent, and why, oldest first.
    ///
    /// What makes the exhaustion sentence able to say more than "they all failed" — by the second
    /// failover, "2 of 3" is not enough to reconstruct what happened to the first.
    spent: Vec<(cide_ipc::PoolEntry, cide_agents::FailoverReason)>,
    /// A sentence about this run's pool, for the row.
    ///
    /// Beside [`Self::note`] rather than in it, because `note_cwd` overwrites `note` the moment a
    /// child comes up — a failover message written there would live for about a second.
    /// [`Self::wire`] composes the two.
    pool_note: Option<String>,
    /// The name of the pool [`Self::pool`] was taken from, for the row. (M89)
    ///
    /// Stamped at the first fork beside the list itself and cleared by [`restamp`], so a Resume
    /// onto a renamed pool names the new one. Persisted, so a history row still says which pool
    /// chose its model after a restart — the question the panel's History tab exists to answer.
    pool_name: Option<String>,
    /// The single model the last child was forked with, `provider/model` where the harness
    /// spells it that way. (M89)
    ///
    /// A copy of `forked_with.model` that **survives a restart**: `forked_with` is deliberately
    /// not persisted (a restored run has no frozen child to compare against), but a finished
    /// run's row must still say what it ran on. Only the fallback — a pool candidate outranks it,
    /// exactly as it outranks the single model in the fork's own argv. See [`LiveRun::using`].
    last_model: Option<String>,
    /// What the child that stands was forked with, as far as a person can change it from
    /// Settings or a role file. `None` until the first fork, and after a restore.
    ///
    /// Written at every fork by [`AgentRegistry::stamp_and_choose`] and read by exactly one
    /// decision — [`AgentRegistry::plan_restarts`], which asks at Resume whether the child it
    /// would fork *now* is a different child from the one that is frozen. A `SIGCONT` cannot
    /// change a process's argv or environment, so a paused run whose settings moved is put on a
    /// new child continuing the same conversation instead; one whose settings did not is thawed
    /// in place, which is what keeps a short pause from costing the in-flight turn.
    forked_with: Option<cide_agents::overrides::ChildSettings>,
    /// cide killed this child on purpose, so its death is not a provider's fault.
    ///
    /// Set by [`AgentRegistry::stop`] and read by [`AgentRegistry::plan_failover`]: without it, a
    /// user pressing Stop on a run would spend a pool candidate and restart it.
    ///
    /// Still a bare `bool` after M67, and deliberately: it answers *whose hand this was*, which
    /// is true from the instant Stop is pressed however the stop then proceeds. [`Self::stop`]
    /// is the second fact beside it — see that field for why the two must not be one.
    stopping: bool,
    /// What was asked for when this run was stopped, and how far it got. (M67)
    ///
    /// # Why this is not just a widened `stopping`
    ///
    /// [`AgentRegistry::plan_restart`] reads `stopping` and **fails the run** on it, which is
    /// right for *a person ended this* and catastrophic for *the agent has been asked to write
    /// down where it got to* — it would end the run at the exact moment it was being asked for
    /// its record. Two facts, two fields.
    ///
    /// Written by [`AgentRegistry::plan_stop`] on every road, including the ones that never ask
    /// (a force, a queued cancel, an interrupted discard), so [`AgentRegistry::death_facts`] has
    /// one field to read rather than a bool and a maybe.
    ///
    /// **Not persisted** in [`SavedRun`], for `death_noted`'s and `provider_failure`'s reason: a
    /// restored row is [`RunState::Interrupted`] with no child, and a latch about a dead
    /// process's pending politeness is a claim about nothing. The cost is named in the journal —
    /// a cide that quits mid-wind-down loses the epitaph, though not the stop-time comment.
    stop: Option<StopRecord>,
    /// How many prompts this run has been given — the opening one, plus each follow-up.
    ///
    /// Not a count of *forks*: a failover re-sends the same prompt to a new child and does not
    /// increment it. That is exactly the distinction the retry needs, because a failure on turn 1
    /// can abandon its conversation with nothing lost, and a failure later cannot.
    turns: u32,
    /// Dispatch order. The panel's groups are ordered within themselves by this, and admission
    /// walks it so a queue is first-in-first-out across agents as well as within one.
    seq: u64,
    /// What this run was doing the instant before `SIGSTOP`, and when that was.
    ///
    /// **Here rather than on the wire.** `cide_ipc::SessionState::Paused` carries no payload for
    /// the reason its doc gives at length: the frontend needs one fact ("this is frozen") and the
    /// post-thaw state, which arrives as its own event. The pre-freeze state is needed by exactly
    /// one decision — was this freeze long enough to have killed an in-flight model request — and
    /// that decision is taken here, so the value lives here.
    ///
    /// `Some` is also the authoritative answer to "is this run frozen": `state` says `Paused` too,
    /// but only this can be restored from.
    frozen: Option<Frozen>,
    /// See [`cide_ipc::AgentRun::stale_turn`]. Set by the thaw watch, cleared by a retry or an
    /// acknowledgement, and never acted on without the user.
    stale_turn: bool,
    /// The **harness's own** conversation id, once its output has named one.
    ///
    /// `None` for every `claude` run and for an `opencode` one that has not printed a line yet,
    /// and the two absences mean different things. A claude run's identity is
    /// [`SessionBinding::Caller`] — cide chose the uuid before the fork, so there is nothing to
    /// scrape and this field would only ever hold a copy of [`Self::session`]. An opencode run's
    /// is [`SessionBinding::Harness`]: the CLI mints `ses_…` itself and prints it on every event
    /// line, so this fills in milliseconds into the run and is what a follow-up passes to
    /// `--session`.
    ///
    /// **Not on the wire.** `AgentRun` carries cide's [`SessionId`], which is the join key for
    /// the phase dot and the Open gesture; a second id that means something only to one CLI is
    /// not a fact a row can draw, and widening the DTO for it would put a harness's private
    /// vocabulary into the panel's.
    harness_session: Option<String>,
    /// Set when a resumed [`RunState::Interrupted`] run goes back through the queue, and read
    /// once at admission: it is what tells [`admit_a_pass`] to build a [`ResumePoint`] and a
    /// continuation prompt instead of replaying the original dispatch. Never persisted — a
    /// restored run is `Interrupted`, and this flag exists only on the short walk from Resume
    /// to admission.
    continuing: bool,
    /// What the next admission forks, when a child's opening prompt never started a turn and
    /// the run went back to the queue for it. See [`AgentRegistry::abandon_unsubmitted`].
    /// Taken at admission and read nowhere else; never persisted, for `continuing`'s reason.
    requeued: Option<Requeued>,
    /// Lines for this run's conversation that arrived mid-turn — a person discussing one of an
    /// MR reviewer's drafts (M101) — typed in order at its next hand-back. Held rather than
    /// typed at once because a line written into a TUI mid-turn lands in whatever the model is
    /// composing, and an opencode child is one process per turn with nobody to read it. Never
    /// persisted: the reply itself is saved with the draft, and a run that dies holding these
    /// is revived from the draft's conversation by the next Discuss.
    follow_ups: Vec<String>,
    /// How many times this run has been put back on the queue for an opening that never took.
    /// Bounded by [`OPENING_REQUEUES`], because a child that cannot be given a prompt twice
    /// running is failing for a reason a third fork will not change.
    opening_requeues: u8,
    /// Whether this run's abnormal end has already been written to its task as a comment
    /// (P2 of the debug-report plan: a task stuck in `doing` after its run died was invisible
    /// until someone opened the Agents panel). A latch, not persisted: a restored row starts
    /// `false`, and [`AgentRegistry::death_facts`] refuses restored rows by other means — the
    /// snapshot only carries ended runs as history, and history rows are never re-noted
    /// because the end edge that calls this fired in the process that watched them die.
    death_noted: bool,
    /// Whether the **real harness** can be put back on this run's conversation once its child
    /// is gone — what [`cide_ipc::AgentRun::openable`] is computed from, beside `session`. (M42)
    ///
    /// Set from in-memory facts only, so a roster broadcast never touches a disk: a `claude`
    /// child that has started (its transcript exists from `SessionStart`, under the cwd this
    /// row records), or an `opencode` run that has named its `ses_…`. A restored row gets the
    /// one filesystem check it needs at restore time, once, in [`AgentRegistry::restore_snapshot`].
    /// [`AgentRegistry::open_plan`] asks the filesystem again at the click, so a `true` that
    /// has gone stale costs a sentence on the row, never a pane that fails after it opened.
    reopenable: bool,
    /// What this run's **last completed step** reported it spent. (M80)
    ///
    /// Written by [`AgentRegistry::note_usage`] from the stream hook — `cide_agents::Harness`'s
    /// `usage`, a third reading of the same lines beside `observe` and `diagnose` — and read by
    /// exactly one thing: [`AgentRegistry::log_run_info`], which answers the card a click on a
    /// `#7` opens.
    ///
    /// **Last, never a sum.** A conversation's spend is not the total of its steps: each step's
    /// prompt *contains* the last one's, so adding them counts the same tokens once per turn and
    /// the figure grows quadratically with a conversation that is merely long. The last step is
    /// the honest one, and it is what [`cide_ipc::TokenUsage::context`] is defined over.
    ///
    /// `None` for a run that has not finished a step, for every `claude` and `qwen` run — cide
    /// never parses their output — and after a restore. Not persisted: the number describes a
    /// child that no longer exists, and the run's next one starts a fresh context.
    usage: Option<cide_ipc::TokenUsage>,
}

/// What was asked for when a run was stopped. See [`LiveRun::stop`]. (M67)
#[derive(Debug, Clone)]
pub struct StopRecord {
    /// Whose hand it was, for the sentence on the task.
    pub by: StopBy,
    /// The stopper's own words, already flattened to one line at the door.
    ///
    /// Three destinations, in descending order of value: **typed into the child** as part of
    /// the wind-down instruction, which is what lets the run's own final comment answer the
    /// objection rather than merely describe where it got to; written **onto the task**; and
    /// the log. Until M67 only the last was true.
    pub reason: Option<String>,
    /// How it went. Moves from [`StopHow::WindingDown`] to one of the terminal spellings.
    pub how: StopHow,
    /// The grace this project allows, for the forced sentence's number. Zero on every road that
    /// never asked.
    pub grace_secs: u64,
}

/// What a caller asked for when it stopped a run. (M67)
pub struct StopRequest {
    /// Whose hand. Set by the command from its caller, never inferred here.
    pub by: StopBy,
    /// The caller's own words. Flattened at the door by the command.
    pub reason: Option<String>,
    /// Skip the asking and kill now.
    pub force: bool,
    /// This project's grace, **read from `.cide/config.json` at the door and passed in**.
    ///
    /// A value and not a read, for the rule this module states twice over: the registry
    /// takes a lock the hook applier also takes, and nothing under it may touch a disk. It
    /// is read fresh per stop rather than cached at dispatch, because that file is committed
    /// and a teammate's commit or a `git checkout` can change it under a running app — a
    /// deadline computed at dispatch would honour a number nobody now has.
    pub grace: Duration,
}

/// What [`AgentRegistry::stop`] needs after it has let the lock go. (M67)
///
/// Read once, under the lock that decided the route, because every one of these can change
/// while a stop is acting: a run can be rebound to a new session by the very `respawn` the ask
/// triggers, and a task can be deleted by anybody.
struct StopFacts {
    session: Option<SessionId>,
    task: Option<TaskId>,
    harness: Harness,
}

/// Whose stop it was. (M67)
///
/// Not a [`cide_ipc::TaskAuthor`]: cide's own writes have exactly one voice
/// ([`cide_ipc::TaskAuthor::Orchestrator`] — see `TaskEdit::Comment`'s doc), so the hand is
/// named *inside* the sentence instead. Minting an author variant for one clause would put a
/// new author into a committed, hand-editable file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBy {
    /// `cide_agent_stop`, from an orchestrating session.
    Orchestrator,
    /// The Stop button in the Agents panel.
    User,
}

impl StopBy {
    /// How the epitaph names the hand. Lower case: it is spliced into a sentence.
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Orchestrator => "the orchestrator",
            Self::User => "the user",
        }
    }
}

/// What became of a stop. (M67)
///
/// Five spellings and not three, because every pair this collapses is a pair somebody later
/// needs apart: *we asked and it went* against *we asked and it would not*, and both of those
/// against *we never asked* — which is this milestone's own bug, one level down, and exactly
/// what `exit 129` meaning both a stop and a crash was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopHow {
    /// Asked, and the child is still alive. The only non-terminal spelling.
    WindingDown {
        /// Whether the wind-down turn has been *seen to start*. See [`plan_wind_down_end`].
        saw_a_turn: bool,
        /// Wall clock, not an `Instant`: a laptop suspended for an hour has elapsed the grace
        /// however little a monotonic clock moved, and the run should not come back to a
        /// deadline that is still politely waiting.
        deadline_unix_ms: u64,
    },
    /// Asked, and it left on its own inside the grace.
    WoundDown,
    /// Asked, and the grace ran out — or the ask could not be delivered after all, which is the
    /// same outcome and carries its own sentence.
    Forced { why: &'static str },
    /// Killed without being asked. `why` is `None` for an explicit `force` and names the
    /// obstacle otherwise; it is never left implicit, because a stop that silently declined to
    /// ask first is a feature that appears not to work.
    Immediate { why: Option<&'static str> },
    /// Cancelled in the queue. There was never a child to speak to.
    BeforeStart,
    /// An interrupted row discarded. The conversation stays on disk.
    Discarded,
    /// Wound down to give its worktree to another task, which is not a stop anybody asked for.
    ///
    /// `bring_up` kills an idle child to reclaim its checkout (`idle_children_in`), and until
    /// M67 that produced `ended with exit 129 before finishing this task` — a crash report about
    /// a run whose turn had *already ended*. Harmless while that sentence meant nothing in
    /// particular; a lie the moment it came to mean **nobody asked**.
    Reclaimed,
    /// An idle child ended because its task reached `done`, which is also not a stop anybody
    /// asked for. See [`AgentRegistry::retire_done`].
    ///
    /// Its own spelling and not `Reclaimed`'s, because the two differ in what they leave on the
    /// board: a reclaim is news to the task it interrupted, while a retirement happens to a task
    /// that is already closed — so [`AgentRegistry::death_facts`] writes no epitaph for it and
    /// `note_run_over` types no line, where either would be one comment and one knock per
    /// finished task, saying only that the work that was done is done.
    Retired,
}

impl StopHow {
    /// Is this stop still waiting on the child?
    pub fn is_winding_down(&self) -> bool {
        matches!(self, Self::WindingDown { .. })
    }
}

/// What an abnormal end leaves for its task's comment. See [`AgentRegistry::death_facts`],
/// which is the only producer and holds the rules; the consumer in `agent_rpc` only formats.
#[derive(Debug, Clone)]
pub struct DeathFacts {
    pub task: TaskId,
    /// The dispatch-time label, [`LiveRun::agent_label`]'s reason: the sentence must still
    /// read correctly after the role is renamed or deleted.
    pub agent_label: String,
    /// `Some(code)` for a child that exited nonzero, `None` for a run that failed before or
    /// beside its child — the two produce different sentences because "exit 129" is a lead
    /// worth printing and a spawn failure has no number to offer.
    ///
    /// Since M67 it is also `Some(0)` for a run that was **stopped** and wound down as asked,
    /// which is the one clean exit worth a line. See [`AgentRegistry::death_facts`].
    pub code: Option<i32>,
    /// What was asked of this run, if anything was. `None` is a death nobody ordered — a crash,
    /// a spawn failure, an exhausted pool — and is the only case whose sentence is unchanged
    /// from before M67. (M67)
    pub stop: Option<StopRecord>,
}

/// What the stop-time comment says. See [`AgentRegistry::stop_asked_facts`]. (M67)
#[derive(Debug, Clone)]
pub struct StopAsked {
    pub task: TaskId,
    pub agent_label: String,
    pub by: StopBy,
    pub reason: Option<String>,
    pub grace_secs: u64,
}

/// Where a run's conversation is filed: the directory its child ran in, or — for a row restored
/// from the snapshot, which records no cwd — the worktree it stamped at dispatch, under `root`.
/// A run with no checkout ran in the root. (M42)
fn run_cwd(root: &std::path::Path, live: &LiveRun) -> PathBuf {
    live.cwd
        .clone()
        .unwrap_or_else(|| checkout_dir(root, live.checkout.as_deref()))
}

/// What a harness is called in a sentence to a person. (M43)
fn harness_label(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude => "Claude Code",
        Harness::Opencode => "opencode",
        Harness::Qwen => "Qwen Code",
        Harness::Codex => "Codex",
        Harness::Mimo => "MiMo Code",
    }
}

/// [`run_cwd`] for a row that is not a `LiveRun` yet — the snapshot's, at restore.
fn checkout_dir(root: &std::path::Path, checkout: Option<&str>) -> PathBuf {
    match checkout {
        Some(name) => cide_git::worktree::path_of(root, name),
        None => root.to_path_buf(),
    }
}

/// The conversation a run's harness would re-open, if the run has named one yet: a `claude`
/// run's session uuid, an `opencode` run's captured `ses_…`. (M42)
fn conversation_of(live: &LiveRun, cwd: &std::path::Path) -> Option<HarnessSession> {
    let id = match live.harness {
        Harness::Claude | Harness::Qwen => live.session.map(|session| session.to_string()),
        Harness::Opencode | Harness::Codex | Harness::Mimo => live.harness_session.clone(),
    }?;
    Some(HarnessSession {
        harness: live.harness,
        id,
        cwd: cwd.to_path_buf(),
    })
}

/// Mint a run and put it on its agent's queue, with the caller's lock already held. (M66)
///
/// The body of what used to be `AgentRegistry::enqueue`, lifted out so that the checking door
/// ([`AgentRegistry::enqueue_unique`]) can ask its question and insert without unlocking in
/// between — the same argument [`admit_a_pass`] makes for deciding an admission and taking its
/// slot under one lock.
fn insert_run(inner: &mut Inner, spec: DispatchSpec) -> RunId {
    let run = RunId::new();
    inner.seq += 1;
    let seq = inner.seq;
    let key = (spec.project, spec.agent.clone());
    let ahead = inner.queues.get(&key).map_or(0, VecDeque::len);
    inner.runs.insert(
        run,
        LiveRun {
            run,
            agent: spec.agent,
            agent_label: spec.agent_label,
            harness: spec.harness,
            project: spec.project,
            session: None,
            task: spec.task,
            task_title: spec.task_title,
            change: spec.change,
            state: RunState::Queued,
            started_unix_ms: now_unix_ms(),
            // Correct by construction: a new run is `Queued`, which is not work, so there is no
            // interval open and nothing accumulated. Every later change goes through `move_to`.
            worked_ms: 0,
            working_since_unix_ms: None,
            prompt: spec.prompt,
            note: (ahead > 0).then(|| {
                format!("{ahead} ahead of it in this role's queue; runs start as slots free")
            }),
            // Stamped at dispatch, so admission can place the run on an entry with room. A run
            // that reaches its first fork with this still empty (a restored snapshot from before
            // the field existed) is stamped there instead — see `stamp_and_choose`.
            pool: spec.pool,
            pool_index: 0,
            provider_failure: None,
            spent: Vec::new(),
            pool_note: None,
            pool_name: None,
            last_model: None,
            forked_with: None,
            stopping: false,
            stop: None,
            // The opening prompt is turn one, so a provider failure before any answer is a
            // first-turn failure — the case that can abandon its conversation for free.
            turns: 1,
            slot: false,
            agent_limit: spec.agent_limit.max(1),
            project_limit: spec.project_limit.max(1),
            checkout: spec.checkout,
            notify: spec.notify,
            purpose: spec.purpose,
            cwd: None,
            seq,
            frozen: None,
            stale_turn: false,
            harness_session: None,
            continuing: false,
            requeued: None,
            follow_ups: Vec::new(),
            opening_requeues: 0,
            death_noted: false,
            reopenable: false,
            // Nothing has run, so nothing has been spent. The stream hook is the only writer.
            usage: None,
        },
    );
    inner.queues.entry(key).or_default().push_back(run);
    run
}

/// The context window a **custom** provider's declaration gives this candidate, in tokens. (M80)
///
/// The only window cide can honestly state, and the narrowness is the point. A
/// [`cide_ipc::LlmProvider::Custom`] model is one the user described themselves — its
/// `limit.context` is a number they typed, written into the document opencode is given, and
/// therefore the window this very child is running against. A catalogued model's window lives in
/// opencode's catalog, which cide deliberately keeps no copy of (see that variant's doc: a copy
/// is a list cide gets wrong the week after), and an external provider's belongs to a plugin.
///
/// Matched on the provider id **and** the model id as two separate keys, never by splitting the
/// joined `provider/model` — [`cide_ipc::PoolEntry`]'s header refuses to own that parser, and a
/// model id may contain both `/` and `:`.
///
/// `None` rather than a guess, and the card then draws a token count with no percentage. A
/// percentage of an invented window is the one shape here that is worse than no number at all:
/// it looks like a measurement.
fn context_limit(providers: &[cide_ipc::LlmProvider], entry: &cide_ipc::PoolEntry) -> Option<u32> {
    providers.iter().find_map(|provider| match provider {
        cide_ipc::LlmProvider::Custom { id, models, .. } if *id == entry.provider => models
            .iter()
            .find(|model| model.id == entry.model)
            // `0` is opencode's "write no limit key" — see `LlmModel::context` — so it is an
            // absence here too, not a window of nothing.
            .and_then(|model| (model.context > 0).then_some(model.context)),
        _ => None,
    })
}

/// The run holding `(project, agent, task)`, read from an `Inner` the caller already has. (M66)
///
/// A free function over `Inner` rather than a method, so the lock-holding
/// [`AgentRegistry::enqueue_unique`] can ask the same question the public
/// [`AgentRegistry::run_holding`] asks without taking the lock a second time — and so the two
/// answers are one definition. The state table and the argument for it are on `run_holding`.
fn holder_in(
    inner: &Inner,
    project: ProjectId,
    agent: &AgentId,
    task: &TaskId,
) -> Option<HeldPair> {
    inner
        .runs
        .values()
        .find(|run| {
            run.project == project
                && &run.agent == agent
                && run.task.as_ref() == Some(task)
                && holds_a_pair(&run.state)
        })
        .map(|run| HeldPair {
            run: run.run,
            state: run.state.clone(),
        })
}

/// Whether a run in this state holds its (role, task) pair against a new dispatch. (M66)
///
/// Written as an exhaustive match rather than a negated `matches!` of the three that do not:
/// a state added later must be classified by whoever adds it, and the compiler is the only
/// reviewer that never forgets to ask. Getting it wrong in the permissive direction spawns a
/// second process; in the strict direction it wedges a task nobody can dispatch.
fn holds_a_pair(state: &RunState) -> bool {
    match state {
        RunState::Queued
        | RunState::Starting
        | RunState::Running
        | RunState::AwaitingPermission
        | RunState::Idle
        | RunState::Paused { .. } => true,
        // Its child died with a previous cide — nothing is running, so nothing is doubled.
        RunState::Interrupted => false,
        RunState::Finished { .. } | RunState::Failed { .. } => false,
    }
}

/// The pane session driving `live`'s conversation right now, if a person has one open and its
/// child is still alive in `sessions`. See [`Inner::viewers`]. (M42)
fn viewed_by(inner: &Inner, sessions: &SessionRegistry, live: &LiveRun) -> Option<SessionId> {
    let id = match live.harness {
        Harness::Claude | Harness::Qwen => live.session?.to_string(),
        Harness::Opencode | Harness::Codex | Harness::Mimo => live.harness_session.clone()?,
    };
    let viewer = *inner.viewers.get(&(live.harness, id))?;
    sessions
        .get(viewer)
        .filter(|pty| !pty.has_exited())
        .map(|_| viewer)
}

/// The sentence a refused continuation carries when a pane holds the conversation. (M42)
const VIEWED_IN_A_PANE: &str = "this run's conversation is open in a pane, and two harness \
                                processes must not write one conversation. Close that pane to \
                                continue the run here — or type the follow-up into it yourself.";

/// What a freeze remembers. See [`LiveRun::frozen`].
#[derive(Debug, Clone)]
struct Frozen {
    /// The run's state the instant before the signal, restored verbatim on resume.
    state: RunState,
    /// Wall clock, not [`Instant`]: the question this feeds is whether a *timeout* elapsed, and a
    /// machine suspended with a frozen child in it has elapsed one however little a monotonic
    /// clock moved. See [`freeze_may_have_killed_the_turn`].
    at_unix_ms: u64,
}

impl LiveRun {
    fn key(&self) -> AgentKey {
        (self.project, self.agent.clone())
    }

    /// What this run is on: the model (`provider/model`) and, where a pool chose it, the pool's
    /// name and position — `("zai/glm-4.6", "fast 2 of 3")`. (M89)
    ///
    /// **Read off the run, never off the role's file**, for the reason [`AgentRegistry::
    /// log_run_info`] gives at length: the file names what the next dispatch would get, the run
    /// names what this one got, and after a local override or a failover those differ. One
    /// ladder, in the order the fork resolves them — the pool's candidate outranks the single
    /// model — and `model_flag` is the join, so this spells the candidate exactly as the child
    /// was told it.
    fn using(&self) -> (Option<String>, Option<String>) {
        let candidate = self.pool.get(self.pool_index);
        let model = match candidate {
            Some(entry) => Some(entry.model_flag()),
            None => self
                .forked_with
                .as_ref()
                .and_then(|settings| settings.model.clone())
                .or_else(|| self.last_model.clone()),
        };
        let position = candidate.map(|_| {
            let at = format!("entry {} of {}", self.pool_index + 1, self.pool.len());
            match &self.pool_name {
                Some(name) => format!("{name} {at}"),
                None => at,
            }
        });
        (model, position)
    }

    fn wire(&self) -> AgentRun {
        let (model, pool_position) = self.using();
        AgentRun {
            run: self.run,
            agent: self.agent.clone(),
            agent_label: self.agent_label.clone(),
            harness: self.harness,
            project: self.project,
            session: self.session,
            state: self.state.clone(),
            task: self.task.clone(),
            started_unix_ms: self.started_unix_ms,
            // Both verbatim, and no `now` is needed to build a row — which is the point of
            // splitting the closed total from the open stamp. A consumer adds the time since
            // the stamp at render, so a live figure ticks between broadcasts and a paused or
            // finished one does not tick at all.
            worked_ms: self.worked_ms,
            working_since_unix_ms: self.working_since_unix_ms,
            notify: self.notify.clone(),
            stale_turn: self.stale_turn,
            // The stop's sentence, then the pool's, then the run's: a run that has been asked
            // to wind down outranks which model it is on, which outranks which worktree it
            // holds. All three are composed here rather than by overwriting `note`, because
            // `note_cwd` rewrites that field whenever a child comes up — and a `Delivery::
            // Respawn` wind-down brings one up, so a stop sentence written into `note` would be
            // clobbered by its own successor with nothing to say it had gone.
            note: compose_note(stop_note(self.stop.as_ref()), &self.pool_note, &self.note),
            openable: self.session.is_some() || self.reopenable,
            model,
            pool_position,
            // Filled by the roster builder, which has the root; see `AgentRun::worktree`.
            worktree: false,
        }
    }
}

/// One run as the agent socket sees it: who it signs as, and where it stands. (M39)
///
/// Not the wire [`AgentRun`] — that carries no path and must not grow one for the panel's
/// sake — and not [`LiveRun`], which is private to this module and carries a dozen fields
/// `agent_rpc` has no business reading. `cwd` is present by construction: see
/// [`AgentRegistry::run_scopes_for`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunScope {
    pub run: RunId,
    pub project: ProjectId,
    pub agent: AgentId,
    pub label: String,
    pub cwd: std::path::PathBuf,
    /// What the run was started for, which decides the tool set its connection is served.
    pub purpose: RunPurpose,
    pub harness: Harness,
}

/// What a run is for. (M85)
///
/// `Work` is every run there was before M85: a project role, standing in the root or its
/// worktree, served the task tracker. `MrReview` is a GitLab merge-request review started from
/// the MR panel: a role built in memory (`LoadedAgent::synthetic`) on the harness the user
/// picked, standing in the MR's disposable checkout, served the `cide_mr_*` tools and nothing
/// else — `agent_rpc`'s `Scope::Review`.
///
/// Carried on the run rather than looked up from its role id, because the id `mr-review` is a
/// name a user could also give a real role file; a run is a review because it was *started* as
/// one, and nothing on disk can make it one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RunPurpose {
    #[default]
    Work,
    MrReview {
        review: String,
        /// The review checkout the child stands in.
        cwd: PathBuf,
        /// The role's whole brief, composed by the launcher for the harness it chose.
        brief: String,
        /// `--allowedTools` for a claude reviewer; ignored by the others.
        tools: Vec<String>,
        /// The harness the user picked. The brief spells tools this harness's way, so the two
        /// travel together.
        harness: Harness,
    },
}

/// Everything the command worker resolved before the queue was touched.
///
/// A value rather than a `DispatchRequest` because the refusal, the role lookup and the prompt
/// composition all happen on the blocking pool, before anything is enqueued — see
/// [`crate::cmd::agents::plan_dispatch`], which is the function that builds this and the place
/// the refusal short-circuits.
#[derive(Debug, Clone)]
pub struct DispatchSpec {
    pub project: ProjectId,
    pub agent: AgentId,
    pub agent_label: String,
    pub harness: Harness,
    pub task: Option<TaskId>,
    pub task_title: Option<String>,
    /// The OpenSpec change the task names, if any. (M28) Stamped at dispatch beside
    /// `task_title`, and for its reason: `cide-agents` reads no disk, so a run's brief has to
    /// arrive as values.
    pub change: Option<String>,
    pub prompt: String,
    pub agent_limit: u16,
    pub project_limit: u16,
    /// The worktree this run will stand in — `cide_agents::run_checkout` over (config, role,
    /// task) — or `None` for a run that stands in the project root: shared isolation, a
    /// `worktree: false` role, or a dispatch with no task (M40). Stamped at dispatch for the
    /// same reason the limits are: the queue must enforce the fact the project had when the
    /// run was dispatched, without a disk read under the lock — and `start_child` calls the
    /// same function at the fork, so the gate and the directory cannot disagree.
    pub checkout: Option<String>,
    /// Where the run's turn endings are announced. (M40) See [`cide_ipc::RunNotify`].
    pub notify: RunNotify,
    /// See [`RunPurpose`]. (M85)
    pub purpose: RunPurpose,
    /// The pool this run will fall down, resolved at dispatch — empty when no pool applies.
    ///
    /// Stamped here rather than at the first fork since pool entries carry a running limit:
    /// [`admit_a_pass`] chooses the entry a run starts on by which one has room, and admission
    /// comes *before* the fork. The limits' reason one field up applies as well — no disk or
    /// settings read under the lock.
    pub pool: Vec<cide_ipc::PoolEntry>,
}

/// A run that a new dispatch of the same role onto the same task would double. (M66)
///
/// Carries the state as well as the id because the refusal has to say *which kind* of live this
/// is: "stop it" is the right advice for a running run and the wrong advice for a paused one,
/// and a sentence that names a run without saying what it is doing is one the caller cannot act
/// on. See [`AgentRegistry::run_holding`] for which states hold and which do not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldPair {
    pub run: RunId,
    pub state: RunState,
}

/// One run the queue has decided to start. Produced under the lock, acted on outside it.
#[derive(Debug, Clone)]
struct Admission {
    run: RunId,
    project: ProjectId,
    agent: AgentId,
    task: Option<TaskId>,
    task_title: Option<String>,
    change: Option<String>,
    prompt: String,
    /// `Some` for a resumed [`RunState::Interrupted`] run: the conversation the new child
    /// continues. `None` is an ordinary first start.
    resume: Option<ResumePoint>,
    /// See [`RunPurpose`]. Copied off the run at admission, like everything else here.
    purpose: RunPurpose,
}

/// How a resumed run's new child finds the old conversation. Built by [`admit_a_pass`] from
/// what the snapshot preserved; which field carries the identity is the harness split
/// [`cide_agents::SessionBinding`] names.
#[derive(Debug, Clone)]
struct ResumePoint {
    /// For a `claude` run: reuse the old [`SessionId`] — it *is* the conversation id
    /// (`--resume <id>` keeps it), so hook routing, the row's session and a pane opened onto
    /// the run all stay continuous. `None` for `opencode`, whose child gets a fresh cide
    /// session while `--session` names the conversation.
    rebind: Option<SessionId>,
    /// What the harness's `respawn_spec` is handed: the claude session id, or opencode's
    /// `ses_…`.
    conversation: String,
}

/// The child a stuck opening is replaced by. See [`AgentRegistry::abandon_unsubmitted`].
#[derive(Debug, Clone)]
struct Requeued {
    /// The conversation the lost child was continuing, as a **fork** (`rebind: None`), or
    /// `None` when it was starting one. A fork rather than the old id: the dying child is still
    /// filed under that id, and its exit must find no run that owns it.
    resume: Option<ResumePoint>,
    /// The prompt the lost child was typed and never submitted.
    prompt: String,
}

/// One run about to be continued by a second child. See [`AgentRegistry::plan_respawn`].
#[derive(Debug, Clone)]
struct Continuation {
    /// The same run, with the prompt this follow-up carries.
    admission: Admission,
    /// The session the run's previous child was filed under, wound down before its successor
    /// starts. `None` for a run that never reached a child at all.
    previous: Option<SessionId>,
    /// The conversation the CLI minted, as its own output named it.
    harness_session: String,
}

/// The runs, the queues and the slots. Behind one lock; see the module header.
#[derive(Default)]
struct Inner {
    runs: HashMap<RunId, LiveRun>,
    /// Per agent, the runs waiting for that agent's slot, oldest first.
    ///
    /// Redundant with "the `Queued` runs in `runs`, in `seq` order", and kept anyway: it is what
    /// makes *serial per agent* structural rather than a property that happens to fall out of an
    /// iteration order somebody could change.
    queues: HashMap<AgentKey, VecDeque<RunId>>,
    /// Slots held right now, per agent and per project. Incremented at admission, decremented
    /// exactly once when the run leaves the working set.
    agent_slots: HashMap<AgentKey, u16>,
    project_slots: HashMap<ProjectId, u16>,
    seq: u64,
    /// Projects whose dispatch queue is shut. **The half of a pause that is not a signal**, and
    /// the reason `AgentRoster::Ready::dispatching` is a separate field on the wire: a project
    /// with the queue closed and nothing running is otherwise indistinguishable from an idle one,
    /// and the user is about to press a button that depends on the difference.
    paused_projects: HashSet<ProjectId>,
    /// Every session this process has `SIGSTOP`ped and not yet `SIGCONT`ed.
    ///
    /// Keyed by session rather than by run because **not every frozen session is a run**: a
    /// project-scope pause also freezes the project's primary console session, which has no row
    /// in the panel and no [`RunId`]. This is also the list `lifecycle::shutdown` walks before
    /// the ladder — see [`AgentRegistry::thaw_for_shutdown`].
    frozen_sessions: HashMap<SessionId, FrozenSession>,
    /// Thawed sessions currently being watched for a sign of life, and the flag each watch reads.
    ///
    /// Populated for the length of [`THAW_WATCH`] and empty the rest of the time. The flag is an
    /// `Arc<AtomicBool>` and not a field on the run because it is set from two threads that must
    /// not take this lock to do it — the hook applier, whose ordering is a correctness
    /// requirement, and `cide-pty`'s coalescer, which is the thread every byte of every session
    /// flows through.
    life: HashMap<SessionId, Arc<AtomicBool>>,
    /// Which pane session is the **real harness** on which conversation — a person reading or
    /// continuing a run whose child has ended, through `SplitIntent::Continue`. (M42)
    ///
    /// Keyed by the conversation as the harness names it: a `claude` run's own session uuid,
    /// an `opencode` run's `ses_…`. That is the identity two processes must never share — a
    /// registry respawn onto a conversation somebody is typing into would be two CLIs writing
    /// one transcript, and neither would say so. `cmd::session::session_spawn` writes a row
    /// after the pane's child is up; `lifecycle::report_exit` clears it when that child is
    /// reaped. Consulted by [`AgentRegistry::respawn`] and the interrupted-run requeue, which
    /// refuse rather than race.
    viewers: HashMap<(Harness, String), SessionId>,
    /// Restarts a Resume has decided and the old child's exit has not yet carried out, keyed by
    /// the session that child was filed under. See [`AgentRegistry::plan_restarts`].
    ///
    /// Keyed by the **old** session on purpose: the run itself is already bound to its successor's
    /// fresh id, so the exit handler — which knows only the session it was attached to — could not
    /// find the run through `runs` at all. That is the point of the rebind (nothing the dying
    /// child says can reach the run) and this table is what the rebind costs.
    restarts: HashMap<SessionId, Failover>,
    /// Pool targets a provider refused recently, which admission steers every run around until
    /// the bench expires or a person resets it. (M90)
    ///
    /// Before this a refusal was a fact about *one run*: it walked that run down its own list and
    /// told nobody else, so every new run tried the server that had just refused and spent a turn
    /// learning what the last run already knew — and a pool with a local server nobody had started
    /// looked, from the outside, like a pool that always began at its last entry. In memory only;
    /// see [`cide_ipc::PoolBench`] for why a restart clears it.
    benched: Vec<Bench>,
    /// What pools did lately, newest last and at most [`POOL_EVENTS`] long — the card's "recent
    /// decisions", which is the only place an admission that passed over an entry is written
    /// down. (M90)
    pool_events: VecDeque<cide_ipc::PoolEvent>,
}

/// How many pool events [`Inner::pool_events`] keeps. Enough for an afternoon's dispatches to be
/// read back; a ring, because nothing reads further back than "why did the last few start there".
const POOL_EVENTS: usize = 100;

/// One benched target. See [`Inner::benched`]. (M90)
#[derive(Debug, Clone)]
struct Bench {
    entry: cide_ipc::PoolEntry,
    reason: cide_agents::FailoverReason,
    since_unix_ms: u64,
    until_unix_ms: u64,
    run: RunId,
    agent_label: String,
}

/// One frozen session, as the registry files it. See [`Inner::frozen_sessions`].
#[derive(Debug, Clone, Copy)]
struct FrozenSession {
    project: ProjectId,
    /// The run this session belongs to, or `None` for a project's primary console session.
    run: Option<RunId>,
}

/// Every dispatched run in the process.
#[derive(Default)]
pub struct AgentRegistry {
    inner: Mutex<Inner>,
    emit: Coalescer,
    /// Set by [`Self::final_snapshot`], read by every later [`Self::save_snapshot`]: once the
    /// teardown has written what the user actually arranged, no flush may write again. The
    /// hazard is concrete and was reported before it was reasoned about — the shutdown ladder
    /// kills every child, the reaper marks each run `Finished`, and the coalescer's next flush
    /// (120 ms, comfortably inside the ladder's 2.5 s of graces) overwrote the snapshot with
    /// those kills recorded as outcomes. Two paused runs restored as *history*, and Resume had
    /// nothing to resume.
    snapshot_sealed: std::sync::atomic::AtomicBool,
}

impl AgentRegistry {
    /// Enqueue unless this role already has a run holding this task — **decided and inserted
    /// under one lock**, which is the half of the duplicate guard a check at the call site
    /// cannot be. (M66)
    ///
    /// [`crate::cmd::agents::plan_dispatch`] refuses the same thing earlier and with a whole
    /// sentence about it; this is the backstop for the window between that answer and this
    /// insert. That window is not theoretical: `task_triggers::dispatch` spawns, so two task
    /// mutations in one burst — a creation naming an assignee and a comment @mentioning the same
    /// role — genuinely can both pass a check before either reaches here. That is the pair that
    /// produced the report.
    ///
    /// A spec with `task: None` never duplicates: such a run stands in the project root with no
    /// worktree to contend for (M40), and N of them is what that road says on its face.
    pub fn enqueue_unique(&self, spec: DispatchSpec) -> std::result::Result<RunId, HeldPair> {
        let mut inner = self.inner.lock();
        if let Some(task) = spec.task.as_ref()
            && let Some(held) = holder_in(&inner, spec.project, &spec.agent, task)
        {
            return Err(held);
        }
        Ok(insert_run(&mut inner, spec))
    }

    /// Put a run on its agent's queue and answer with its id, **without asking whether one is
    /// already there**.
    ///
    /// **Never spawns and never blocks**, which is this whole module's version of the
    /// `openDiff` rule: that tool blocking an agent's turn is a documented invariant precisely
    /// because it is dangerous, and a dispatch that waited on the run would let one wedged
    /// subagent freeze whoever called it — including the orchestrating session, over the MCP
    /// socket, in the middle of its own turn.
    ///
    /// **`#[cfg(test)]` since M66, and that is the guard.** The duplicate that put two runs on
    /// one task got in because the one non-test caller of this function did not check first, and
    /// a check that lives at the call site is a check the next call site forgets. So the shipped
    /// binary now has exactly one door onto the queue, [`Self::enqueue_unique`], and this insert
    /// survives only for the tests below — most of which are about slots and ordering rather than
    /// about duplication, and have no business minting a task id to say so.
    #[cfg(test)]
    fn enqueue(&self, spec: DispatchSpec) -> RunId {
        insert_run(&mut self.inner.lock(), spec)
    }

    /// How many slot-holding runs, in every project, stand on each pool target — the figure
    /// [`cide_ipc::PoolEntry::max_running`] limits, for the roster an orchestrator reads.
    /// [`entry_load`]'s count, grouped; targets with nothing on them are absent.
    pub fn pool_load(&self) -> Vec<(cide_ipc::PoolEntry, u32)> {
        let inner = self.inner.lock();
        let mut load: Vec<(cide_ipc::PoolEntry, u32)> = Vec::new();
        for live in inner.runs.values().filter(|live| live.slot) {
            let Some(on) = live.pool.get(live.pool_index) else {
                continue;
            };
            match load.iter_mut().find(|(entry, _)| entry.same_target(on)) {
                Some((_, n)) => *n += 1,
                None => load.push((on.clone(), 1)),
            }
        }
        load
    }

    /// Every configured pool as admission sees it right now: load, benches, the runs on each
    /// entry, the runs waiting, and the recent decisions — for the pool-state card. (M90)
    ///
    /// Read in one lock so the card cannot show a run on an entry *and* waiting for it. Pools are
    /// taken from `llm` (the settings as they stand), and runs are matched to entries by target
    /// rather than by pool name, because that is what a running limit and a bench are both about:
    /// a run stamped from a pool that has since been renamed is still using that server.
    pub fn pool_state(&self, llm: &cide_ipc::LlmSettings) -> cide_ipc::PoolStateReport {
        let inner = self.inner.lock();
        let now = now_unix_ms();
        let run_ref = |live: &LiveRun| cide_ipc::PoolRunRef {
            run: live.run,
            agent_label: live.agent_label.clone(),
            project: live.project,
            state: live.state.clone(),
            position: live.using().1.unwrap_or_default(),
            model: live.using().0,
            note: compose_note(None, &live.pool_note, &live.note),
        };
        let pools = llm
            .pools
            .iter()
            .map(|pool| cide_ipc::PoolState {
                name: pool.name.clone(),
                description: pool.description.clone(),
                entries: pool
                    .entries
                    .iter()
                    .map(|entry| {
                        let mut runs: Vec<&LiveRun> = inner
                            .runs
                            .values()
                            .filter(|live| live.slot)
                            .filter(|live| {
                                live.pool
                                    .get(live.pool_index)
                                    .is_some_and(|on| on.same_target(entry))
                            })
                            .collect();
                        runs.sort_by_key(|live| live.seq);
                        cide_ipc::PoolEntryState {
                            entry: entry.clone(),
                            provider: match llm.provider(&entry.provider) {
                                None => cide_ipc::PoolProviderState::Missing,
                                Some(provider) if !provider.enabled() => {
                                    cide_ipc::PoolProviderState::Disabled
                                }
                                Some(_) => cide_ipc::PoolProviderState::Ready,
                            },
                            running: u32::try_from(runs.len()).unwrap_or(u32::MAX),
                            runs: runs.into_iter().map(run_ref).collect(),
                            bench: bench_on(&inner, entry, now).map(|bench| cide_ipc::PoolBench {
                                reason: bench.reason.wire(),
                                since_unix_ms: bench.since_unix_ms,
                                until_unix_ms: bench.until_unix_ms,
                                run: bench.run,
                                agent_label: bench.agent_label.clone(),
                            }),
                        }
                    })
                    .collect(),
            })
            .collect();
        let mut waiting: Vec<&LiveRun> = inner
            .runs
            .values()
            .filter(|live| matches!(live.state, RunState::Queued) && !live.pool.is_empty())
            .collect();
        waiting.sort_by_key(|live| live.seq);
        // A pool applies only to a harness that reads the provider document, so only those can
        // be "off" one — a claude run on no pool is not a run that missed one.
        let mut off_pool: Vec<&LiveRun> = inner
            .runs
            .values()
            .filter(|live| live.slot && live.pool.is_empty())
            .filter(|live| live.harness.reads_provider_document())
            .collect();
        off_pool.sort_by_key(|live| live.seq);
        cide_ipc::PoolStateReport {
            now_unix_ms: now,
            pools,
            waiting: waiting.into_iter().map(run_ref).collect(),
            off_pool: off_pool.into_iter().map(run_ref).collect(),
            events: inner.pool_events.iter().rev().cloned().collect(),
        }
    }

    /// Clear the bench on one target, or on every target (`None`), and move the runs that are
    /// **waiting** back up their lists onto it. Answers the projects whose rows changed, for the
    /// caller to redraw before it pumps the queue. (M90)
    ///
    /// # Which runs move
    ///
    /// Only a run that is `Queued` or `Interrupted` and holds no slot — one with no child, whose
    /// next fork is decided by admission from `pool_index` on. A run standing on a child keeps
    /// the model that child was forked with: moving its index would make `entry_load` count it
    /// against a server it is not talking to, and there is no argv to change under a live
    /// process anyway. Its *next* dispatch picks the reset entry up like any new run's.
    ///
    /// This is the one place besides [`restamp`] that lowers `pool_index`, and it is allowed to
    /// for the same reason: a person said so. The monotonic rule exists so a run cannot loop on
    /// a refusal by itself, not to overrule the user who has just fixed what refused.
    pub fn reset_pool_entry(&self, target: Option<&cide_ipc::PoolEntry>) -> Vec<ProjectId> {
        let mut inner = self.inner.lock();
        inner
            .benched
            .retain(|bench| target.is_some_and(|target| !bench.entry.same_target(target)));
        let mut touched = Vec::new();
        let mut rewound = 0u32;
        for live in inner.runs.values_mut() {
            if live.slot || !matches!(live.state, RunState::Queued | RunState::Interrupted) {
                continue;
            }
            let back_to = match target {
                Some(target) => live.pool.iter().position(|entry| entry.same_target(target)),
                None => (!live.pool.is_empty()).then_some(0),
            };
            let Some(back_to) = back_to.filter(|&at| at < live.pool_index) else {
                continue;
            };
            live.pool_index = back_to;
            match target {
                Some(target) => live.spent.retain(|(entry, _)| !entry.same_target(target)),
                None => live.spent.clear(),
            }
            live.pool_note = Some(format!(
                "moved back to {} (entry {} of {}) by a reset",
                live.pool[back_to].model_flag(),
                back_to + 1,
                live.pool.len()
            ));
            rewound += 1;
            if !touched.contains(&live.project) {
                touched.push(live.project);
            }
        }
        log_pool_event(
            &mut inner,
            cide_ipc::PoolEvent {
                at_unix_ms: now_unix_ms(),
                pool: None,
                run: None,
                agent_label: None,
                project: None,
                kind: cide_ipc::PoolEventKind::Reset {
                    entry: target.cloned(),
                    rewound,
                },
            },
        );
        touched
    }

    /// Take every queued run that may start now, marking each `Starting` and holding its slot.
    ///
    /// The admission decision and the slot it takes happen under one lock, which is what makes
    /// two concurrent pumps safe: whichever gets the lock first takes the slot, and the second
    /// sees a full one. Production goes through [`Self::take_admissions_noting`]; this is its
    /// face for a test that only counts admissions.
    #[cfg(test)]
    fn take_admissions(&self) -> Vec<Admission> {
        self.take_admissions_noting().0
    }

    /// Take every queued run that may start now, under one lock for the reason given on
    /// `take_admissions` above — plus every project where a queued run's row now says
    /// something new — a pool that is full — so [`Self::pump`] can redraw it. Without that the
    /// sentence would be written under the lock and shown at the next unrelated change.
    fn take_admissions_noting(&self) -> (Vec<Admission>, Vec<ProjectId>) {
        let mut inner = self.inner.lock();
        let mut admitted = Vec::new();
        let mut noted = Vec::new();

        // A pass admits at most one run per agent, because only the *front* of an agent's queue
        // is ever eligible — that is what makes a queue serial. So it repeats until a pass admits
        // nothing: under `Isolation::Shared` a role may legitimately hold several slots, and a
        // single pass would start one of three and leave the other two waiting for an unrelated
        // event to call this again.
        loop {
            let before = admitted.len();
            admit_a_pass(&mut inner, &mut admitted, &mut noted);
            if admitted.len() == before {
                break;
            }
        }

        (admitted, noted)
    }
}

/// How many runs **holding a slot**, in any project, stand on this pool target right now —
/// the count [`cide_ipc::PoolEntry::max_running`] is a ceiling on.
///
/// Counted by scan rather than kept in a map beside `agent_slots`, because the thing counted is
/// a *derived* fact — which entry a slot-holding run is on — and a counter would need every
/// writer of `slot` and of `pool_index` to remember it. The registry holds tens of runs.
///
/// A slot and not "live", for the project cap's reason: an idle run has handed its turn back and
/// its child is not talking to the provider, so it is not using the concurrency a provider sells.
/// `except` leaves one run out — a failover asking where *it* can go next.
fn entry_load(inner: &Inner, entry: &cide_ipc::PoolEntry, except: Option<RunId>) -> usize {
    inner
        .runs
        .values()
        .filter(|live| live.slot && Some(live.run) != except)
        .filter(|live| {
            live.pool
                .get(live.pool_index)
                .is_some_and(|on| on.same_target(entry))
        })
        .count()
}

/// The bench standing on this target at `now`, if any. See [`Inner::benched`]. (M90)
///
/// Expiry is read here rather than swept by a timer: an expired bench is simply one this answers
/// `None` for, so there is no clock thread to own and no moment at which it is half-cleared.
fn bench_on<'a>(inner: &'a Inner, entry: &cide_ipc::PoolEntry, now: u64) -> Option<&'a Bench> {
    inner
        .benched
        .iter()
        .find(|bench| bench.until_unix_ms > now && bench.entry.same_target(entry))
}

/// Which entry a run should stand on, and every entry it passed over on the way. (M90)
#[derive(Debug, Clone)]
struct Choice {
    at: usize,
    skipped: Vec<cide_ipc::PoolSkipped>,
    /// Every entry with room was benched, and this is the first of them — tried anyway.
    benched_fallback: bool,
}

/// The first entry of `pool` at or after `from` that has room **and is not benched**; failing
/// that, the first with room at all; `None` only when every one is at its running limit.
///
/// `from` is how monotonicity survives the limits: a run a failover moved to entry 2 never
/// climbs back to entry 0 because entry 0 has since freed up — that entry already refused it.
///
/// # The bench is a preference, never a gate (M90)
///
/// A benched entry is passed over while anything later has room, and taken anyway when nothing
/// does. The other reading — wait until a bench expires — would park a whole pool behind a timer
/// the moment its last entry refused once, which is the deadlock a pool exists to avoid, and it
/// would make "the server is back" something the user can only express by finding the Reset
/// button. Trying a benched entry costs at most the turn it would have cost without benches.
fn choose_entry(
    inner: &Inner,
    pool: &[cide_ipc::PoolEntry],
    from: usize,
    except: Option<RunId>,
    now: u64,
) -> Option<Choice> {
    let mut skipped = Vec::new();
    let mut fallback: Option<(usize, usize)> = None;
    for (at, entry) in pool.iter().enumerate().skip(from) {
        let index = u16::try_from(at).unwrap_or(u16::MAX);
        if let Some(max) = entry.max_running {
            let running = entry_load(inner, entry, except);
            if running >= usize::from(max) {
                skipped.push(cide_ipc::PoolSkipped {
                    entry: entry.clone(),
                    index,
                    skip: cide_ipc::PoolSkip::Full {
                        running: u32::try_from(running).unwrap_or(u32::MAX),
                        max,
                    },
                });
                continue;
            }
        }
        if let Some(bench) = bench_on(inner, entry, now) {
            // The skips *before* this one belong to the fallback's story; the ones after it do
            // not, because a run that falls back to here never looked past it.
            fallback.get_or_insert((at, skipped.len()));
            skipped.push(cide_ipc::PoolSkipped {
                entry: entry.clone(),
                index,
                skip: cide_ipc::PoolSkip::Benched {
                    reason: bench.reason.wire(),
                    until_unix_ms: bench.until_unix_ms,
                },
            });
            continue;
        }
        return Some(Choice {
            at,
            skipped,
            benched_fallback: false,
        });
    }
    let (at, before) = fallback?;
    skipped.truncate(before);
    Some(Choice {
        at,
        skipped,
        benched_fallback: true,
    })
}

/// `k3s/qwen benched (unreachable, 2m left), o3/big full 1/1` — the skips as the row says them.
fn skipped_phrase(skipped: &[cide_ipc::PoolSkipped], now: u64) -> String {
    skipped
        .iter()
        .map(|skip| match &skip.skip {
            cide_ipc::PoolSkip::Full { running, max } => {
                format!("{} full {running}/{max}", skip.entry.model_flag())
            }
            cide_ipc::PoolSkip::Benched {
                reason,
                until_unix_ms,
            } => format!(
                "{} benched ({}, {} left)",
                skip.entry.model_flag(),
                refusal_phrase(*reason),
                minutes_left(*until_unix_ms, now)
            ),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// [`cide_agents::FailoverReason::phrase`], from the wire side.
fn refusal_phrase(reason: cide_ipc::PoolRefusal) -> &'static str {
    match reason {
        cide_ipc::PoolRefusal::RateLimited => "rate limited",
        cide_ipc::PoolRefusal::Unreachable => "unreachable",
        cide_ipc::PoolRefusal::Auth => "refused the credential",
    }
}

/// `2m`, rounded up so a bench with seconds to go never reads `0m left`.
fn minutes_left(until: u64, now: u64) -> String {
    let ms = until.saturating_sub(now);
    format!("{}m", ms.div_ceil(60_000).max(1))
}

/// Append to the pool log, dropping the oldest past [`POOL_EVENTS`].
fn log_pool_event(inner: &mut Inner, event: cide_ipc::PoolEvent) {
    if inner.pool_events.len() >= POOL_EVENTS {
        inner.pool_events.pop_front();
    }
    inner.pool_events.push_back(event);
}

/// Bench `entry` for its reason's time, or extend a bench already on it. (M90)
fn bench_entry(
    inner: &mut Inner,
    entry: &cide_ipc::PoolEntry,
    reason: cide_agents::FailoverReason,
    run: RunId,
    agent_label: &str,
    now: u64,
) -> u64 {
    let until = now + reason.bench_ms();
    inner
        .benched
        .retain(|bench| !bench.entry.same_target(entry));
    inner.benched.push(Bench {
        entry: entry.clone(),
        reason,
        since_unix_ms: now,
        until_unix_ms: until,
        run,
        agent_label: agent_label.to_string(),
    });
    until
}

/// The row's sentence for a run waiting on a full pool: each entry it could take, and how full.
fn pool_full_note(inner: &Inner, pool: &[cide_ipc::PoolEntry], from: usize) -> String {
    let each = pool
        .iter()
        .skip(from)
        .map(|entry| {
            format!(
                "{} {}/{}",
                entry.model_flag(),
                entry_load(inner, entry, None),
                entry
                    .max_running
                    .map_or_else(|| "∞".to_string(), |max| max.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("every model in its pool is at its running limit ({each}); starts when one frees")
}

/// One pass of the admission scan: at most one run per agent, oldest dispatch first.
fn admit_a_pass(inner: &mut Inner, admitted: &mut Vec<Admission>, noted: &mut Vec<ProjectId>) {
    let now = now_unix_ms();
    {
        // Dispatch order across the whole registry, so an agent that has been waiting longer
        // wins the project's last slot.
        let mut fronts: Vec<(u64, RunId, AgentKey)> = inner
            .queues
            .iter()
            .filter_map(|(key, queue)| {
                let run = *queue.front()?;
                Some((inner.runs.get(&run)?.seq, run, key.clone()))
            })
            .collect();
        fronts.sort_by_key(|(seq, _, _)| *seq);

        for (_, run, key) in fronts {
            let Some(live) = inner.runs.get(&run) else {
                continue;
            };
            let (project, agent_limit, project_limit) =
                (live.project, live.agent_limit, live.project_limit);
            let checkout = live.checkout.clone();
            // The queue half of a pause. A paused project keeps its queue — nothing is cancelled
            // and the order is preserved — it simply stops being drained, so a resume starts
            // exactly what was waiting, in the order it was waiting in.
            if inner.paused_projects.contains(&project) {
                continue;
            }
            let agent_held = inner.agent_slots.get(&key).copied().unwrap_or(0);
            let project_held = inner.project_slots.get(&project).copied().unwrap_or(0);
            if agent_held >= agent_limit || project_held >= project_limit {
                continue;
            }
            // The pool gate: which entry this run starts on, by which has room. A run with no
            // pool has nothing to gate. A pool whose every remaining entry is at its
            // `max_running` keeps the run queued with a sentence rather than overcommitting one
            // — the provider would only refuse it, and a refusal costs a turn to learn what the
            // settings already said. Counted across projects: see `PoolEntry::max_running`.
            //
            // It also steers around benched targets (M90) — see `choose_entry` for why a bench
            // is a preference and never a reason to wait.
            let chosen = match live.pool.is_empty() {
                true => None,
                false => {
                    let from = live.pool_index.min(live.pool.len() - 1);
                    match choose_entry(inner, &live.pool, from, None, now) {
                        Some(choice) => Some(choice),
                        None => {
                            let note = pool_full_note(inner, &live.pool, from);
                            let live = inner.runs.get_mut(&run).expect("just read above");
                            if live.note.as_deref() != Some(note.as_str()) {
                                live.note = Some(note);
                                if !noted.contains(&project) {
                                    noted.push(project);
                                }
                            }
                            continue;
                        }
                    }
                }
            };
            // The checkout gate — the per-task successor to the worktree clamp that used to
            // pin `agent_limit` at 1. Two children in one checkout is the failure worktree
            // isolation exists to prevent, and with a worktree per task it is no longer a
            // per-role *number* but a per-directory fact: this run stays queued while any run
            // whose child may still be standing in the same checkout is on it. `Idle` does not
            // block — its parked child is wound down by `bring_up` at the moment the checkout
            // is claimed — and `Interrupted` and the terminal states hold no process at all.
            // `None` collides with nothing: N runs in one directory is what shared isolation
            // says on its face, and a run with no task (M40) stands in the project root by
            // design — a gate on the root would be a gate on the user.
            let occupied = checkout.as_deref().is_some_and(|name| {
                inner.runs.values().any(|other| {
                    other.run != run
                        && other.project == project
                        && other.checkout.as_deref() == Some(name)
                        && holds_its_checkout(other)
                })
            });
            if occupied {
                continue;
            }

            inner.agent_slots.insert(key.clone(), agent_held + 1);
            inner.project_slots.insert(project, project_held + 1);
            if let Some(queue) = inner.queues.get_mut(&key) {
                queue.pop_front();
            }
            let live = inner.runs.get_mut(&run).expect("just read above");
            // The edge that opens the worked clock: everything before this was queue, and the
            // queue is not work — a run that sat behind two others did nothing in the wait.
            move_to(live, RunState::Starting, now_unix_ms());
            live.slot = true;
            live.note = None;
            let mut started = None;
            if let Some(choice) = chosen {
                live.pool_index = choice.at;
                // Said on the row only when something was passed over: a run on the first entry
                // it may try has nothing to explain, and its pool position already says where it
                // is. When something was, this is the sentence the user had no way to get before
                // M90 — why a run began at the bottom of its list.
                if !choice.skipped.is_empty() || choice.benched_fallback {
                    let target = &live.pool[choice.at];
                    let passed_over = match choice.skipped.is_empty() {
                        true => String::new(),
                        false => format!("; passed over {}", skipped_phrase(&choice.skipped, now)),
                    };
                    live.pool_note = Some(match choice.benched_fallback {
                        false => format!(
                            "started on {} (entry {} of {}){passed_over}",
                            target.model_flag(),
                            choice.at + 1,
                            live.pool.len(),
                        ),
                        true => format!(
                            "started on {} (entry {} of {}) although it is benched — nothing after it \
                             had room{passed_over}",
                            target.model_flag(),
                            choice.at + 1,
                            live.pool.len(),
                        ),
                    });
                }
                started = Some(cide_ipc::PoolEvent {
                    at_unix_ms: now,
                    pool: live.pool_name.clone(),
                    run: Some(run),
                    agent_label: Some(live.agent_label.clone()),
                    project: Some(project),
                    kind: cide_ipc::PoolEventKind::Started {
                        entry: live.pool[choice.at].clone(),
                        index: u16::try_from(choice.at).unwrap_or(u16::MAX),
                        of: u16::try_from(live.pool.len()).unwrap_or(u16::MAX),
                        skipped: choice.skipped,
                        benched_fallback: choice.benched_fallback,
                    },
                });
            }
            // A resumed interrupted run continues its old conversation; one that never had a
            // conversation to continue (it was still queued when cide quit) starts fresh with
            // its original prompt, which is the honest reading of "nothing happened yet".
            let requeued = live.requeued.take();
            let resume = match (&requeued, live.continuing) {
                (Some(requeued), _) => requeued.resume.clone(),
                (None, true) => resume_point(live),
                (None, false) => None,
            };
            live.continuing = false;
            if let Some(point) = resume.as_ref() {
                // The other half of `requeue_interrupted`'s trace: this is the line that says
                // *which conversation* a continuing child was pointed at, so two of them for one
                // run is visible in the log rather than only in the harness's own store.
                tracing::info!(
                    %run,
                    conversation = %point.conversation,
                    "resume: admitting a continuing child on an existing conversation"
                );
            }
            let prompt = match requeued {
                // Exactly what the lost child was meant to be told: it never read a word of it.
                Some(requeued) => requeued.prompt,
                None => match resume.is_some() {
                    true => continuation_prompt(
                        live.task.as_ref(),
                        live.task_title.as_deref(),
                        // Both facts the sentence needs: whether a bridge will be attached at all,
                        // and what this run's CLI calls the tool. `start_child` refuses a bridgeless
                        // run outright, so the `None` arm is a belt-and-braces answer rather than a
                        // state a user reaches.
                        cide_hook_binary()
                            .and_then(|_| cide_agents::harness::for_kind(live.harness)),
                        Restarted::Cide,
                    ),
                    false => live.prompt.clone(),
                },
            };
            admitted.push(Admission {
                run,
                project,
                agent: key.1,
                task: live.task.clone(),
                task_title: live.task_title.clone(),
                change: live.change.clone(),
                prompt,
                resume,
                purpose: live.purpose.clone(),
            });
            if let Some(event) = started {
                log_pool_event(inner, event);
            }
        }
    }
}

/// See [`ResumePoint`]. `None` when the run never reported a conversation — the caller starts
/// it fresh instead, which is a restart of the work rather than a lie about continuing it.
fn resume_point(live: &LiveRun) -> Option<ResumePoint> {
    match live.harness {
        Harness::Claude | Harness::Qwen => live.session.map(|session| ResumePoint {
            rebind: Some(session),
            conversation: session.to_string(),
        }),
        // Codex is caller-bound for routing and harness-bound for its conversation (M93), and
        // the conversation is the half a resume needs: the successor gets a fresh routing id —
        // so a dying predecessor's last frames and its exit belong to nobody — and
        // `codex resume` is handed the thread, which is the same conversation whatever id cide
        // files the child under.
        Harness::Opencode | Harness::Codex | Harness::Mimo => {
            live.harness_session
                .clone()
                .map(|conversation| ResumePoint {
                    rebind: None,
                    conversation,
                })
        }
    }
}

/// What a child forked now would be forked with, per role — or `None` for a role that cannot be
/// forked now, which a Resume reads as "nothing to compare, thaw in place".
type SettingsNow<'a> = dyn Fn(&AgentId) -> Option<cide_agents::overrides::ChildSettings> + 'a;

/// Read what a fork would resolve to, for every role of `project`, once.
///
/// The same reads a fork makes — [`facts`] and `load_project` — done on the caller's thread
/// before any registry lock, and captured, so the comparison a Resume makes across a whole
/// project's runs is against one snapshot of the settings rather than a file that could move
/// between two rows. A resolution the fork would **refuse** (an override naming a pool that is
/// not configured) answers `None`: a broken setting is not a changed one, and restarting a
/// healthy paused run into a refusal would end it for nothing — the refusal waits for the next
/// dispatch, where it is shown.
fn settings_now(app: &AppHandle, project: ProjectId) -> Box<SettingsNow<'static>> {
    let facts = match facts(app, project) {
        Ok(facts) => facts,
        Err(error) => {
            tracing::warn!(%error, "cannot read the settings a resume compares; thawing every run as it stands");
            return Box::new(|_| None);
        }
    };
    let agents = cide_agents::load_project(&facts.root);
    Box::new(move |id| {
        let agent = agents.get(id)?;
        let resolved = facts.resolve(agent);
        if resolved.refusal.is_some() {
            return None;
        }
        Some(facts.child_settings(&resolved))
    })
}

/// What every role of `project` would run as, right now. (M71)
///
/// The roster's second column, and the answer `cide_agents_config`'s neighbours are read against.
/// It is [`facts`] plus `load_project` — the same two reads a fork makes, so the overrides and the
/// providers it resolves against are the ones a dispatch would resolve against.
///
/// **`overrides::resolve` directly, not [`Facts::resolve`]**, and the difference is one probe:
/// `Facts::resolve` folds opencode's *own* default model in, which runs `opencode debug config`.
/// That is right at a fork, which happens once per run, and wrong here, where the roster is read
/// on most orchestration turns — a subprocess per read for a value the row states as "its default
/// model" either way. The two therefore differ only in the case where nothing in cide names a
/// model at all, and the row says so rather than naming an id that might be another one.
pub(crate) fn resolutions_for(
    app: &AppHandle,
    project: ProjectId,
) -> Result<Vec<(cide_ipc::AgentId, cide_agents::overrides::Resolved)>> {
    let facts = facts(app, project)?;
    let loaded = cide_agents::load_project(&facts.root);
    Ok(loaded
        .catalog
        .agents
        .iter()
        .map(|agent| {
            (
                agent.id().clone(),
                cide_agents::overrides::resolve(agent, &facts.overrides, &facts.llm),
            )
        })
        .collect())
}

/// Put a run on the pool as configured now. The one place the stickiness rule on
/// [`LiveRun::pool`] yields, and only to a person's Resume — never to a failover or a follow-up,
/// which continue on the list the run was arranged on.
///
/// Given the new list rather than clearing the old one for the next fork to restamp, because
/// admission reads the list *before* the fork: a cleared pool is a run whose running limits
/// nobody counts.
fn restamp(live: &mut LiveRun, pool: &[cide_ipc::PoolEntry]) {
    live.pool = pool.to_vec();
    live.pool_index = 0;
    live.spent.clear();
    live.pool_note = None;
    // Re-stamped at the next fork, from the settings that fork resolves.
    live.pool_name = None;
}

/// What a run says while its settings restart is between the old child and the new.
const RESTARTING_NOTE: &str =
    "restarting on the current settings; a new child continues the same conversation";

/// The same, for a run whose frozen child had not begun — nothing to continue, so the dispatch
/// is replayed on the current settings instead.
const RESTARTING_FRESH_NOTE: &str =
    "restarting on the current settings; nothing had happened yet, so the dispatch is replayed";

/// What a run whose settings changed says when a pane holds its conversation and it is thawed
/// as it stands instead. See `plan_restarts`.
const SETTINGS_KEPT_FOR_A_PANE: &str = "settings changed, but this run's conversation is open in a pane, and two harness processes \
     must not write one conversation — it continues on the settings it started with. Close that \
     pane and pause and resume it again to restart it.";

/// What a queued run in a paused project says instead of nothing.
///
/// One sentence, naming the state and the gesture that ends it. It deliberately does *not* say
/// "waiting for a slot": the slots may be entirely free, which is what made this state so hard to
/// read from the panel — a paused project with two idle slots and a queued run looks like a bug in
/// the scheduler until you know the queue is shut.
///
/// A const because three surfaces quote it — the panel through `AgentRun::note`, `cide_agent_runs`
/// through `render_run`, and the tests — and a second wording of one fact reads as a second fact.
pub(crate) const PAUSED_QUEUE_NOTE: &str =
    "this project's agents are paused; nothing starts until Resume";

/// The first line a resumed run is told. One line, for the same `\r` rule as every prompt.
///
/// It does not restate the task body — the conversation being continued already holds it — but
/// it does name the id, because a restart is exactly the moment the tracker may have moved
/// under the run and `cide_task_get` is how it finds out.
///
/// # `tools` carries the same two facts `cmd::agents::opening_prompt`'s does
///
/// And for the same reasons, arrived at later: this function is the *third* place cide names a
/// tracker tool to a run, and it was the one nobody noticed. It hard-coded Claude Code's
/// `mcp__cide__cide_task_get` — uncallable under opencode — and, unlike both preambles, it was
/// gated on nothing at all, so a run resumed without a bridge was told to call a tool that had
/// never been attached. `None` drops the sentence; `Some(harness)` spells it that CLI's way.
fn continuation_prompt(
    task: Option<&TaskId>,
    title: Option<&str>,
    tools: Option<&dyn cide_agents::harness::Harness>,
    why: Restarted,
) -> String {
    let title = title
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty())
        .map(|title| format!(" ({title})"))
        .unwrap_or_default();
    let opening = why.opening();
    match (task, tools) {
        (Some(id), Some(harness)) => format!(
            "{opening} Re-read task {id}{title} with {}, \
             check your worktree with git status, and continue from \
             where the conversation left off.",
            harness.tool_name("cide_task_get")
        ),
        // A task it cannot re-read is a task it can still be reminded of by name; the git half of
        // the sentence is the half that does not need the bridge.
        (Some(id), None) => format!(
            "{opening} You were on task {id}{title}: check your \
             worktree with git status and continue from where the conversation left off."
        ),
        (None, _) => format!(
            "{opening} Check the tree with git status and \
             continue from where the conversation left off."
        ),
    }
}

/// The row's sentence for a run that has been stopped, or `None` for every other run. (M67)
///
/// Only the wind-down says anything: it is the one state that persists long enough for a person
/// to read it and the one where *not knowing* is confusing — a row that says `running` while
/// its agent is writing a farewell is a row nobody can act on. Every terminal spelling is the
/// row's own phase a moment later, and the durable account is the task's comments.
fn stop_note(stop: Option<&StopRecord>) -> Option<String> {
    let stop = stop?;
    if !stop.how.is_winding_down() {
        return None;
    }
    Some(format!(
        "winding down — asked by {} to post a final comment and exit; cide ends it within {}s",
        stop.by.phrase(),
        stop.grace_secs
    ))
}

/// Join the row's three sentence sources, most urgent first, dropping the ones with nothing.
fn compose_note(
    stop: Option<String>,
    pool: &Option<String>,
    note: &Option<String>,
) -> Option<String> {
    let parts: Vec<&str> = [stop.as_deref(), pool.as_deref(), note.as_deref()]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" — "))
}

/// Might this run's child still be standing in its checkout? (M67)
///
/// **One function for two readers** — `admit_a_pass`'s occupancy gate and `idle_children_in`'s
/// reclaim — because they are two spellings of one question, and a run that one of them thinks
/// has left the directory while the other thinks is still in it is two processes in one
/// worktree. They were separate before M67 and agreed by coincidence.
///
/// `Idle` does **not** hold a checkout: the parked child is `bring_up`'s to wind down at the
/// moment the directory is actually claimed, which is the trade `RunState::Idle`'s own doc
/// argues for. The exception is a run that has been **asked to wind down**: its child has been
/// told to write one last comment and will be dead within the grace, so taking the directory
/// out from under it buys a few seconds and costs the whole point of asking. Bounded, unlike
/// the idle `claude` the exclusion exists for.
///
/// The concurrency **slot** is deliberately not held by any of this. Releasing it on the
/// hand-back edge stays right, and a sibling admitted into a *different* checkout is exactly
/// what should happen — holding a checkout and holding a slot are two decisions, which is the
/// distinction `RunState::Idle`'s doc draws.
fn holds_its_checkout(live: &LiveRun) -> bool {
    if live
        .stop
        .as_ref()
        .is_some_and(|stop| stop.how.is_winding_down())
    {
        return true;
    }
    matches!(
        live.state,
        RunState::Starting
            | RunState::Running
            | RunState::AwaitingPermission
            | RunState::Paused { .. }
    )
}

/// What a state change means for a wind-down in flight. See [`wind_down_step`]. (M67)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindDownStep {
    /// Nothing to do — the commonest answer by far.
    Wait,
    /// The wind-down turn has begun. Remember it, so the hand-back that follows is the one.
    Arm,
    /// That hand-back was the wind-down turn's own end. Mark it, then kill the child.
    Over,
    /// The child left on its own, cleanly. Mark it wound down; there is nothing to kill.
    Completed,
}

/// Has the wind-down turn ended? Pure over one transition. (M67)
///
/// # Two edges, never one, and the difference is a lost turn
///
/// The obvious rule — *kill on the next hand-back* — is wrong, and wrong in the direction that
/// destroys the thing the feature exists to save. A stop pressed mid-turn types into a composer
/// whose CLI queues the line and submits it when the **current** turn ends, so the first
/// `→ Idle` after the ask usually belongs to the turn being stopped. Killing there would end
/// the child a beat *before* it was told anything.
///
/// So the run must be seen to start a turn ([`WindDownStep::Arm`]) before a hand-back counts.
/// A run asked while already `Idle` takes the identical path: nothing has been seen, the ask
/// starts a turn, that turn ends. One rule, both cases, and the Esc that precedes the line only
/// makes the first edge arrive sooner.
///
/// If the line is never taken up no edge arrives at all, and the grace is what ends the run —
/// which is exactly what a grace is for.
///
/// Only a `Delivery::Stdin` harness reaches this: opencode's and codex's wind-down is a whole
/// new child that exits when its turn is done, and `Observation::Exit` ends those runs the way
/// it always did.
pub(crate) fn wind_down_step(how: &StopHow, handed_back: bool, next: &RunState) -> WindDownStep {
    let StopHow::WindingDown { saw_a_turn, .. } = how else {
        return WindDownStep::Wait;
    };
    // **The `Delivery::Respawn` ending, and it is not the same shape as the other one.** An
    // opencode or codex wind-down is a whole new child: it answers the instruction, writes its
    // comment and *exits*. There is no hand-back, so the `Over` road below never fires and
    // nothing would ever mark the stop complete — the run would die still flagged
    // `WindingDown`, and its epitaph would say it *died before it could wind down* about the
    // one case where everything worked.
    //
    // Clean exits only. A child that crashed or was killed mid-wind-down really did die before
    // it could finish, and the sentence for that is already correct.
    if let RunState::Finished { code } = next {
        return if *code == 0 {
            WindDownStep::Completed
        } else {
            WindDownStep::Wait
        };
    }
    if handed_back {
        return if *saw_a_turn {
            WindDownStep::Over
        } else {
            // The interrupted turn handing back. Not ours, and not a reason to do anything:
            // `Arm` below fires when the wind-down turn actually starts.
            WindDownStep::Wait
        };
    }
    // `AwaitingPermission` counts as a turn having started, and must: a wind-down turn that
    // stops to ask permission has plainly begun, and refusing to arm on it would mean the
    // hand-back that follows is read as the interrupted turn's and the run is never ended by
    // anything but the grace.
    if matches!(next, RunState::Running | RunState::AwaitingPermission) && !*saw_a_turn {
        return WindDownStep::Arm;
    }
    WindDownStep::Wait
}

/// Which road a stop takes. See [`stop_route`]. (M67)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StopRoute {
    /// Off the queue, and failed. Nothing was ever forked.
    Cancel,
    /// An interrupted row discarded; the conversation stays on disk.
    Discard,
    /// It had already ended. Idempotent, deliberately.
    Nothing,
    /// Kill the child now, saying why it was not asked first.
    Kill { why: Option<&'static str> },
    /// Ask it to wind down, then watch.
    Ask,
}

/// Obstacles to asking, spelled once so the route and the record cannot word them differently.
///
/// Each is a full sentence fragment in the past tense, because both readers splice it after
/// "It was not asked to wind down first:" — the tool's answer to the caller, and the epitaph on
/// the task. One string, two readers, no chance of the board and the orchestrator disagreeing
/// about why a stop was not the polite kind.
const PAUSED: &str = "it was paused, and a stopped process neither answers a signal nor reads \
                      its input, so the line would have been read on resume, out of order with \
                      whatever it was doing";
const AWAITING_PERMISSION: &str = "it was waiting on a permission prompt, where a typed line and its Enter would have \
     answered that prompt instead, approving the very tool call the stop was meant to prevent";
const RESTARTING: &str = "its child was already being wound down for a settings restart, so \
                          there was nothing to speak to";
const NO_CHILD: &str = "it had no child to speak to";
const SEALED: &str =
    "cide is shutting down, and there would have been nobody left to read what it wrote";
const ALREADY_ASKED: &str = "it had already been asked once and had not gone";
const NO_CONVERSATION: &str = "it never reported a conversation id, so there was nothing to \
                               continue it from";
/// Not an obstacle the route saw coming: the ask itself was refused at delivery. See
/// `AgentRegistry::stop`'s `Err` arm.
const COULD_NOT_ASK: &str = "its harness refused the message";
/// The grace ran out. The only `StopHow::Forced` reason that is not a surprise.
const RAN_OUT_OF_TIME: &str = "it did not wind down in time";

/// What a stop should do, decided from facts alone. (M67)
///
/// Pure, and pure *of the registry* rather than merely of an `AppHandle`: every input is a value
/// the caller resolved, so the whole table below is a test row rather than a claim about a live
/// lock. `plan_respawn`'s and `plan_failover`'s split, taken one step further because this
/// decision has more arms than either.
///
/// # The two arms nobody would guess, and both are real bugs
///
/// **`AwaitingPermission` must never be asked.** The permission prompt is a selection list, and
/// a wind-down is delivered as text followed by a lone `\r` (`type_submitted_line`) — the `\r`
/// is what the dialog reads. A graceful stop there would *approve the tool call it was meant to
/// prevent*, which is the single worst thing in this milestone's blast radius.
///
/// **`Paused` must never be asked**, for the rule `AgentRegistry::resume` already states: a
/// `SIGSTOP`ped process does not act on a signal, and the bytes sit in the kernel buffer to be
/// read on resume, out of order with whatever it was doing.
///
/// And **`sealed`** — `lifecycle::shutdown` sets `snapshot_sealed` before the ladder. A grace
/// per run would turn quitting cide into a minutes-long wait, and there would be nobody left to
/// read the comment it bought.
///
/// A **second** stop on a run already winding down is the escalation: it is how `force` is
/// reached without a second argument, and it is why the tool's answer says so.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stop_route(
    state: &RunState,
    session: Option<SessionId>,
    already_asked: bool,
    restart_pending: bool,
    sealed: bool,
    can_continue: bool,
    grace: Duration,
    force: bool,
) -> StopRoute {
    match state {
        RunState::Queued => StopRoute::Cancel,
        RunState::Finished { .. } | RunState::Failed { .. } => StopRoute::Nothing,
        RunState::Interrupted => StopRoute::Discard,
        _ => {
            let Some(_session) = session else {
                return StopRoute::Kill {
                    why: Some(NO_CHILD),
                };
            };
            let why = if force || grace.is_zero() {
                // The caller asked for this, or the project did. Not an obstacle, so no
                // sentence: `Immediate { why: None }` is what "we never asked, on purpose"
                // looks like, and inventing a reason for it would read as an apology.
                None
            } else if already_asked {
                Some(ALREADY_ASKED)
            } else if sealed {
                Some(SEALED)
            } else if restart_pending {
                Some(RESTARTING)
            } else if matches!(state, RunState::Paused { .. }) {
                Some(PAUSED)
            } else if matches!(state, RunState::AwaitingPermission) {
                Some(AWAITING_PERMISSION)
            } else if !can_continue {
                Some(NO_CONVERSATION)
            } else {
                return StopRoute::Ask;
            };
            StopRoute::Kill { why }
        }
    }
}

/// The line a run is given when it is stopped and asked to wind down. (M67)
///
/// # One line, and the harness's own dialect — both scars, both [`continuation_prompt`]'s
///
/// **One line**, because this is typed into a TUI and the harness ends it with `\r`: an embedded
/// newline is another Enter, so a two-paragraph instruction would submit its first line as a
/// turn and feed the rest in as further turns. Everything that can carry a newline — the
/// caller's `reason`, the task title — goes through `one_line` before it gets here.
///
/// **The comment tool is spelled by the harness** (`mcp__cide__cide_task_comment` under Claude
/// Code, `cide_cide_task_comment` under opencode). [`continuation_prompt`] carries the scar: it
/// hard-coded Claude's spelling and was gated on nothing, so every opencode run was told to
/// call a tool it did not have. `tools: None` — a run with no bridge — drops the clause rather
/// than naming a tool that is not attached.
///
/// **A run with no task is asked for no comment.** M40's task-less dispatch works in the
/// project root and has nowhere to report; telling it to comment on a task it has not got would
/// send it looking for one. It is asked to stop and to say where it got to, and the tree is the
/// record.
///
/// The `reason` is handed over **because a run told why writes a better handover**. That is the
/// one place in this feature where prose written by one model becomes prose read by another, so
/// it is quoted as the stopper's words rather than restated as cide's.
fn wind_down_prompt(
    task: Option<&TaskId>,
    tools: Option<&dyn cide_agents::harness::Harness>,
    reason: Option<&str>,
) -> String {
    let because = match reason
        .map(crate::cmd::agents::one_line)
        .filter(|why| !why.is_empty())
    {
        Some(why) => format!(" The reason given: {why}"),
        None => String::new(),
    };
    let report = match (task, tools) {
        (Some(id), Some(harness)) => format!(
            " Post one final comment on task {id} with {} saying what you finished, what you \
             were part-way through, and anything you learned that is not written down anywhere \
             yet — then exit.",
            harness.tool_name("cide_task_comment")
        ),
        // A task it cannot comment on is a task it can still be told about; the useful half of
        // the instruction — say where you got to, then exit — needs no bridge.
        (Some(id), None) => format!(
            " You were on task {id}. Say what you finished, what you were part-way through, and \
             anything you learned that is not written down anywhere yet — then exit."
        ),
        (None, _) => " Say what you finished, what you were part-way through, and anything you \
             learned that is not written down anywhere yet — then exit."
            .to_string(),
    };
    format!("cide is stopping this run.{because} Stop work now and start nothing new.{report}")
}

/// Why a run is being told to pick up where it left off — the first clause of
/// [`continuation_prompt`]. Two sentences rather than one vague one, because the model is
/// about to be asked to reconcile a conversation with a working tree, and "cide restarted" on
/// a machine that never went down is a false lead about what may have changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Restarted {
    /// The cide process went down and came back; the child died with it.
    Cide,
    /// A person changed the model settings under a pause and resumed; cide restarted the run.
    Settings,
}

impl Restarted {
    fn opening(self) -> &'static str {
        match self {
            Self::Cide => "cide restarted while you were working.",
            Self::Settings => {
                "cide restarted you on new model settings while you were paused; the \
                 conversation so far is yours, the process is new."
            }
        }
    }
}

impl AgentRegistry {
    /// Record the session a run's child is about to be filed under.
    ///
    /// Called **before** the fork, deliberately: a hook frame is applied on another thread, and
    /// binding after the insert would leave a window in which the child's first `SessionStart`
    /// reached a registry that could not name the run it belonged to.
    fn bind_session(&self, run: RunId, session: SessionId) {
        if let Some(live) = self.inner.lock().runs.get_mut(&run) {
            live.session = Some(session);
        }
    }

    /// Note which directory a run is working in: for the row, and for the agent socket. (M39)
    fn note_cwd(&self, run: RunId, cwd: &std::path::Path) {
        if let Some(live) = self.inner.lock().runs.get_mut(&run) {
            live.note = Some(cwd.display().to_string());
            live.cwd = Some(cwd.to_path_buf());
            // A `claude` child files its transcript under this directory the moment it starts,
            // so from here the conversation can be re-opened there. An opencode run's answer
            // waits for its output to name the `ses_…` — see `note_harness_session`.
            if matches!(live.harness, Harness::Claude | Harness::Qwen) {
                live.reopenable = true;
            }
        }
    }

    /// What `agent_rpc` needs to scope a connection that names one of this project's runs:
    /// the role to sign as, and the directory a relative path is read from. (M39)
    ///
    /// **A run without a cwd is not in the answer.** It is queued or restored-and-not-resumed,
    /// so no child of it exists and nothing can have connected as it; answering the project
    /// root for it would be answering a question that cannot legitimately be asked yet, and a
    /// connection that *did* name such a run is one nothing this process started — refused,
    /// like an unknown run id.
    /// Every unfinished run reviewing `review`, in any project. (M85) Closing a review deletes
    /// the checkout these stand in, so the close stops them first.
    /// The conversation `run` is in, as its harness would resume it: claude's session uuid,
    /// opencode's `ses_…`. `None` for a run that has not named one yet. Recorded on each draft
    /// a reviewer writes, so a draft's Discuss can revive the conversation that made it. (M101)
    pub fn conversation_id(&self, run: RunId) -> Option<String> {
        let inner = self.inner.lock();
        let live = inner.runs.get(&run)?;
        match live.harness {
            Harness::Claude | Harness::Qwen => live.session.map(|session| session.to_string()),
            Harness::Opencode | Harness::Codex | Harness::Mimo => live.harness_session.clone(),
        }
    }

    /// Whether `run` still has a conversation cide can type into — alive, not over, not
    /// stopping. A Discuss on a draft whose run fails this revives the conversation instead.
    pub fn follow_up_reachable(&self, project: ProjectId, run: RunId) -> bool {
        let inner = self.inner.lock();
        inner.runs.get(&run).is_some_and(|live| {
            live.project == project
                && live.stop.is_none()
                && matches!(
                    live.state,
                    RunState::Queued
                        | RunState::Starting
                        | RunState::Running
                        | RunState::AwaitingPermission
                        | RunState::Idle
                )
        })
    }

    /// Say one more thing to a live run's conversation: now if its turn is over, else at its
    /// next hand-back (`LiveRun::follow_ups`). (M101)
    ///
    /// The delivery is the harness's (`send_prompt`): typed into claude/qwen/codex's TUI, a
    /// `--session` respawn for opencode — the path `retry_turn` already takes. `line` must be one
    /// line; every newline in a TUI is an Enter.
    pub fn follow_up(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: RunId,
        line: &str,
    ) -> Result<()> {
        let Some((session, kind, line)) = self.plan_follow_up(project, run, line)? else {
            return Ok(());
        };
        self.send_prompt(app, project, run, session, &line, kind)?;
        self.mark_changed(app, project);
        Ok(())
    }

    /// The half of [`Self::follow_up`] under the lock, for `plan_respawn`'s reason: a test can
    /// drive it without an `AppHandle`. `Some` is "type this now, into this session" — the line,
    /// after anything still held, joined, since two typed lines would be two turns; `None` is
    /// "held for the next hand-back"; an error is a run nothing can be said to.
    fn plan_follow_up(
        &self,
        project: ProjectId,
        run: RunId,
        line: &str,
    ) -> Result<Option<(SessionId, Harness, String)>> {
        let mut inner = self.inner.lock();
        let live = inner
            .runs
            .get_mut(&run)
            .filter(|live| live.project == project)
            .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
        if live.stop.is_some() {
            return Err(CoreError::Io("this run is being stopped".into()));
        }
        match (&live.state, live.session) {
            // Idle and not paused: its turn is over, so type now — and anything held for it too,
            // which would otherwise wait for a hand-back an idle run is not going to make.
            (RunState::Idle, Some(session)) if live.frozen.is_none() => {
                let mut lines = std::mem::take(&mut live.follow_ups);
                lines.push(line.to_string());
                Ok(Some((session, live.harness, lines.join(" — then: "))))
            }
            (
                RunState::Queued
                | RunState::Starting
                | RunState::Running
                | RunState::AwaitingPermission
                | RunState::Idle,
                _,
            ) => {
                live.follow_ups.push(line.to_string());
                Ok(None)
            }
            (state, _) => Err(CoreError::Io(format!(
                "this run cannot be spoken to ({state:?})"
            ))),
        }
    }

    /// Enqueue a review run that **continues** `conversation` rather than starting a fresh one,
    /// with the spec's prompt as the first thing it reads — a draft's Discuss reviving a
    /// reviewer that is gone. (M101) The admission's `requeued` road, which already forks a
    /// child onto an existing conversation with a prompt of its own: as a **fork**
    /// (`rebind: None`), so a predecessor still filed under the old id cannot have its exit end
    /// this run.
    pub fn enqueue_continuing(
        &self,
        spec: DispatchSpec,
        conversation: String,
    ) -> std::result::Result<RunId, HeldPair> {
        let prompt = spec.prompt.clone();
        let mut inner = self.inner.lock();
        if let Some(task) = spec.task.as_ref()
            && let Some(held) = holder_in(&inner, spec.project, &spec.agent, task)
        {
            return Err(held);
        }
        let run = insert_run(&mut inner, spec);
        if let Some(live) = inner.runs.get_mut(&run) {
            live.requeued = Some(Requeued {
                resume: Some(ResumePoint {
                    rebind: None,
                    conversation,
                }),
                prompt,
            });
        }
        Ok(run)
    }

    pub fn review_runs(&self, review: &str) -> Vec<(ProjectId, RunId)> {
        self.inner
            .lock()
            .runs
            .values()
            .filter(|run| {
                matches!(&run.purpose, RunPurpose::MrReview { review: r, .. } if r == review)
                    && !matches!(
                        run.state,
                        RunState::Finished { .. } | RunState::Failed { .. }
                    )
            })
            .map(|run| (run.project, run.run))
            .collect()
    }

    pub fn run_scopes_for(&self, project: ProjectId) -> Vec<RunScope> {
        let inner = self.inner.lock();
        inner
            .runs
            .values()
            .filter(|run| run.project == project)
            .filter_map(|run| {
                Some(RunScope {
                    run: run.run,
                    project: run.project,
                    agent: run.agent.clone(),
                    label: run.agent_label.clone(),
                    cwd: run.cwd.clone()?,
                    purpose: run.purpose.clone(),
                    harness: run.harness,
                })
            })
            .collect()
    }

    /// Move a run to a new state, releasing its slot if this is the transition that ends its
    /// claim on the agent's worktree. Answers whether anything changed.
    ///
    /// The release rule is the module header's, and the *edge* is the whole of it: only a
    /// transition out of `Running`/`AwaitingPermission` into `Idle` counts as "the turn was
    /// handed back". `Starting → Idle` is the `SessionStart` window and releases nothing.
    ///
    /// # These edges are the product-owner loop's only triggers
    ///
    /// A run reports back through `.cide/tasks.json` and nothing watches that file on the
    /// orchestrator's behalf, so the moment a turn is handed back — or a run finishes or fails —
    /// is the moment the project's primary session has something new to read.
    /// [`crate::agent_rpc::note_run_over`] is called from **here** (and from the one wrapper
    /// below), precisely because this is the only place in the process that can see a
    /// *transition* rather than a state: an idle run is observed over and over — every
    /// statusline frame is an observation — and a nudge per observation would be a typed prompt
    /// per observation. Each edge happens once.
    ///
    /// # `app: Option<&AppHandle>`, and what the `None` arm costs
    ///
    /// `None` means *this caller cannot nudge*, and there is exactly one such caller:
    /// [`Self::watch_exit_with`], which is deliberately app-free so that a real child's exit can
    /// be driven by a test — `tauri`'s mock app is behind a feature this build does not enable.
    /// It still costs nothing, because the exit's nudge is delivered one layer up instead: the
    /// production wrapper [`Self::watch_exit`] holds the app and calls `note_run_over` after the
    /// transition, so the app-free arm stays drivable by
    /// `a_real_childs_exit_finishes_its_run` and a reaped child's ending is still announced.
    /// Every *hook*-reached edge — through [`note_hook`] — carries an app of its own.
    fn set_state(&self, app: Option<&AppHandle>, run: RunId, next: RunState) -> bool {
        let mut inner = self.inner.lock();
        let Some(live) = inner.runs.get_mut(&run) else {
            return false;
        };
        if live.state == next {
            return false;
        }
        let handed_back = matches!(
            (&live.state, &next),
            (
                RunState::Running | RunState::AwaitingPermission,
                RunState::Idle
            )
        );
        let over = matches!(next, RunState::Finished { .. } | RunState::Failed { .. });
        move_to(live, next, now_unix_ms());
        if over {
            // A frozen child that died under the freeze is not frozen any more, and leaving the
            // record behind would put a dead session on `thaw_for_shutdown`'s list and leave the
            // row restorable to a state its process no longer has. `Observation::Exit` is the one
            // observation that answers from `Paused`, which is exactly how this is reached.
            live.frozen = None;
            live.stale_turn = false;
            inner
                .frozen_sessions
                .retain(|_, frozen| frozen.run != Some(run));
        }
        let Some(live) = inner.runs.get_mut(&run) else {
            return true;
        };
        // Decided under the lock, acted on after it — the discipline the module header states,
        // and the nudge is the newest reason for it: `note_run_idle` reads `.cide/config.json`,
        // takes the session registry and writes into a PTY, none of which may happen with this
        // mutex held while the hook applier thread is the one holding it.
        let mut nudge = None;
        // A wind-down in flight ends its run on the hand-back that belongs to *its own* turn,
        // and `wind_down_step` is the whole of that rule. Decided here, where the edge is
        // already computed, and acted on after `drop(inner)` beside the nudge — this method is
        // reached on the hook applier thread, whose ordering is a correctness requirement, and
        // reaching into the session registry with this mutex held is what the module header
        // forbids.
        //
        // The app-free caller (`watch_exit_with`'s reaper) can never be the hand-back edge — a
        // harness answers `Finished` and nothing else to an `Exit` — so no wind-down end is
        // ever lost to it.
        let mut wind_down_kill = None;
        if let Some(stop) = live.stop.as_mut() {
            match wind_down_step(&stop.how, handed_back, &live.state) {
                WindDownStep::Wait => {}
                WindDownStep::Arm => {
                    if let StopHow::WindingDown { saw_a_turn, .. } = &mut stop.how {
                        *saw_a_turn = true;
                    }
                }
                WindDownStep::Over => {
                    stop.how = StopHow::WoundDown;
                    wind_down_kill = live.session;
                }
                // Already gone: the child that answered is the child that exited. Nothing to
                // kill, and `wind_down_kill` stays `None`.
                WindDownStep::Completed => stop.how = StopHow::WoundDown,
            }
        }
        // A hand-back is when a held Discuss line may be typed (see `LiveRun::follow_ups`). Taken
        // under the lock so two edges cannot both deliver it; typed after it, like the nudge.
        let follow_ups = match handed_back && live.stop.is_none() {
            true => std::mem::take(&mut live.follow_ups),
            false => Vec::new(),
        };
        let follow_project = live.project;
        if handed_back || over {
            let key = live.key();
            let project = live.project;
            let held = live.slot;
            live.slot = false;
            // Both edge kinds nudge — `note_run_over` reads the run's state back and renders
            // "handed back" / "finished (exit n)" / "failed" accordingly. An `over` reached
            // app-less (the reaper) computes this and drops it; `watch_exit` re-delivers it.
            nudge = Some(project);
            if held {
                release(&mut inner, &key, project);
            }
        }
        drop(inner);
        self.forget_old();
        if let Some(app) = app
            && !follow_ups.is_empty()
            && let Some(registry) = app.try_state::<Arc<AgentRegistry>>()
        {
            let registry = Arc::clone(&registry);
            let app = app.clone();
            // Off the hook applier thread: delivery reaches the session registry and may fork
            // an opencode child, and this method runs on the thread whose ordering is a
            // correctness requirement. Joined into one line: two typed lines are two turns, and
            // the second would land while the first is being answered.
            let line = follow_ups.join(" — then: ");
            tauri::async_runtime::spawn(async move {
                if let Err(error) = registry.follow_up(&app, follow_project, run, &line) {
                    tracing::warn!(%run, %error, "a held follow-up could not be delivered");
                }
            });
        }
        // Before the nudge, because this is what *makes* the run over: the child is still alive
        // at this point — it handed its turn back and is sitting at its prompt — and the run's
        // end is the exit that follows.
        if let (Some(app), Some(session)) = (app, wind_down_kill) {
            tracing::info!(%run, "a run wound down as asked; ending it");
            self.kill_child(app, Some(session));
        }
        if let (Some(app), Some(project)) = (app, nudge) {
            crate::agent_rpc::note_run_over(app, project, run);
        }
        true
    }

    /// Fail a run that never reached a child, with the sentence its row will show.
    fn fail(&self, app: Option<&AppHandle>, run: RunId, reason: impl Into<String>) -> bool {
        self.set_state(
            app,
            run,
            RunState::Failed {
                reason: reason.into(),
            },
        )
    }

    /// What one hook frame or one exit means for the run that owns `session`.
    ///
    /// The mapping is [`cide_agents::Harness::observe`]'s and not this module's, which is what
    /// keeps the two refusals it documents in one place: a `Paused` run is never moved by a frame
    /// that was already in flight, and a late `PostToolUse` never resurrects a finished run.
    pub(crate) fn observe(
        &self,
        app: Option<&AppHandle>,
        session: SessionId,
        ob: Observation<'_>,
    ) -> Option<(RunId, RunState)> {
        let (run, kind, current) = {
            let inner = self.inner.lock();
            let live = inner.runs.values().find(|r| r.session == Some(session))?;
            (live.run, live.harness, live.state.clone())
        };
        let harness = cide_agents::for_kind(kind)?;
        // A caller-bound harness whose *conversation* the CLI still mints announces it in a hook
        // (codex's `SessionStart`, M93). Recorded before the state moves, so a run that ends on
        // this very frame is already reopenable. `note_harness_session` never overwrites.
        if let Observation::Hook(frame) = ob
            && let Some(conversation) = harness.capture_hook(frame)
        {
            self.note_harness_session(run, conversation);
        }
        let next = harness.observe(current, ob)?;
        self.set_state(app, run, next.clone())
            .then_some((run, next))
    }

    /// Every run this project has, in the order the panel's groups read them.
    ///
    /// # Why the pause note is derived here rather than written where the pause bites
    ///
    /// `admit_a_pass` skips a paused project's queue with a bare `continue`, and a run held there
    /// looked exactly like a run waiting behind its role's concurrency: `[queued]`, no reason, in
    /// the panel and in `cide_agent_runs` alike, while the roster went on saying `ready` and
    /// dispatch went on answering "Dispatched". A terrastrike probe sat in that state for hours
    /// with both project slots free.
    ///
    /// Writing the note at that `continue` was the obvious fix and is the wrong one twice over:
    /// `live` is borrowed immutably there, and — the real reason — a run restored from the
    /// snapshot into a paused project has never passed through an admission pass at all, so the
    /// note would be missing on exactly the runs a restart leaves behind. Deriving it at wire
    /// time covers both, needs no second pass, and cannot go stale: this function already holds
    /// the lock that owns `paused_projects`.
    ///
    /// It rides [`AgentRun::note`] — the DTO's existing "what the queue is waiting on" slot,
    /// already carrying `enqueue`'s *N ahead of it* — so the panel renders it with no change at
    /// all, and `tools::render_run` passes it through to a model for free.
    pub fn runs_for(&self, project: ProjectId) -> Vec<AgentRun> {
        let inner = self.inner.lock();
        let paused = inner.paused_projects.contains(&project);
        let mut runs: Vec<&LiveRun> = inner
            .runs
            .values()
            .filter(|run| run.project == project)
            .collect();
        runs.sort_by_key(|run| (group_rank(&run.state), run.seq));
        runs.iter()
            .map(|run| {
                let mut wire = run.wire();
                // Only `Queued`. A `Paused` run says it in its own state, and a run in any other
                // state is not being held by the queue — overriding its note would replace a fact
                // about *that run* with one about the project.
                if paused && matches!(wire.state, RunState::Queued) {
                    wire.note = Some(PAUSED_QUEUE_NOTE.to_string());
                }
                wire
            })
            .collect()
    }

    /// Whether the queue will start anything new for this project.
    ///
    /// The flag [`Self::pause`] flips, and deliberately *not* derived from "every run is paused":
    /// `AgentRoster::Ready::dispatching`'s doc says why the two facts have to stay
    /// distinguishable on the wire — a project with the queue shut and nothing running looks
    /// exactly like an idle one otherwise, and Resume is the button that turns on which it is.
    pub fn dispatching(&self, project: ProjectId) -> bool {
        !self.inner.lock().paused_projects.contains(&project)
    }

    /// The run of `agent` on `task` that would be doubled by a new dispatch, if there is one.
    ///
    /// **The duplicate rule, and it is narrower than "has not ended".** (M66) A pair is held by
    /// a run that has a child or is about to get one — `Queued`, `Starting`, `Running`,
    /// `AwaitingPermission`, `Idle`, `Paused` — and by nothing else. `Idle` holds on purpose: an
    /// idle run still owns a live child in the role's only worktree, and the way to re-engage it
    /// is a retry or a prompt, not a second run queued behind it. `Paused` holds because a
    /// frozen child has not let go of the checkout.
    ///
    /// `Interrupted` deliberately does **not** hold, and that is a change from the predicate this
    /// replaced. Its child died with a previous cide; nothing is running, so nothing would be
    /// doubled — and while it counted, a row left behind by a restart silently swallowed every
    /// re-assignment of that role to that task with a debug line and no way to see why. The cost
    /// of excluding it is that Resume could requeue such a row beside a run dispatched in the
    /// meantime, which is why [`AgentRegistry::requeue_interrupted`] asks this question too.
    ///
    /// Not spelled `is_live`: [`cide_agents::tools`] uses that word for "not `Finished`/`Failed`"
    /// on the run list, which is a different set, and two notions of *open* under one word is how
    /// a rule like this rots.
    pub fn run_holding(
        &self,
        project: ProjectId,
        agent: &AgentId,
        task: &TaskId,
    ) -> Option<HeldPair> {
        holder_in(&self.inner.lock(), project, agent, task)
    }

    /// Whether `session` belongs to a run that has not ended — the guard `session_kill` asks.
    ///
    /// This is the domain-side answer to the debug report's single-pane deaths (e83a75fe:
    /// exit 129 after nine minutes while its sibling lived). A mirror pane's spawn plan lives
    /// in the *clicking* window's JS realm, so in a multi-window layout the pane can render in
    /// a window that never heard of the plan, adopt the run's session without the `mirrored`
    /// flag — and `closePane`'s kill then SIGHUPed the agent mid-turn, silently. Rather than
    /// widening `Pane` with a second ownership signal (the no-`agent_open_pane` note says why
    /// that road is closed), the kill *command* refuses sessions the registry owns: a pane can
    /// close its view of a run, and only a run's owners — Stop, the idle wind-down, a respawn,
    /// the shutdown ladder — end its child, all of which call `pty.kill()` directly and never
    /// pass through `session_kill`.
    ///
    /// `Interrupted` answers **false**, and it used to answer true "harmlessly, there is no
    /// process behind it". There can be one now: a person can open an interrupted run's
    /// conversation in a pane — `claude --resume` under the run's own session id (M42) — and
    /// that child is the pane's, spawned by `session_spawn`, ended by closing the pane. A
    /// refusal here would leak it until the app quit. The registry's own continuation of that
    /// run moves it `Queued → Starting` before any child of *its* exists, so a registry child
    /// is never behind an `Interrupted` row and nothing is lost by the answer.
    /// Which harness the run filed under `session` runs, whatever its state. (M93)
    pub(crate) fn harness_of_session(&self, session: SessionId) -> Option<Harness> {
        self.inner
            .lock()
            .runs
            .values()
            .find(|run| run.session == Some(session))
            .map(|run| run.harness)
    }

    pub fn owns_session(&self, session: SessionId) -> bool {
        self.inner.lock().runs.values().any(|run| {
            run.session == Some(session)
                && !matches!(
                    run.state,
                    RunState::Finished { .. } | RunState::Failed { .. } | RunState::Interrupted
                )
        })
    }

    /// The facts an abnormal end leaves for its task, or `None` when there is nothing to say.
    ///
    /// P2 of the debug-report plan: a run that died mid-task left the task sitting in `doing`
    /// with no trace on the board — the report's terrastrike runs were only diagnosable from
    /// the Agents panel, which the orchestrator never reads. The answer is one comment on the
    /// task, and this method is the decision half: it says *whether* to comment and hands back
    /// what the sentence needs, in one lock pass, so the caller
    /// ([`crate::agent_rpc::note_run_over`]'s death arm) does no registry reasoning of its own.
    ///
    /// `Some` requires all of: the run ended abnormally (`Failed`, or `Finished` with a nonzero
    /// code — exit 0 is a run that believes it finished, and second-guessing it belongs to the
    /// orchestrator's own review, not to an automatic epitaph); it carried a task; its
    /// `death_noted` latch was clear; and **no other open run** of the same role holds the same
    /// task — a respawn's predecessor stays quiet because the successor picks the work up and
    /// its own end will speak if it also dies.
    ///
    /// The latch is set on *every* abnormal-end pass, including the ones a surviving sibling
    /// silences: the question it records is "has this run's end been dealt with", and it has.
    /// It is not persisted — a restored row starts `false`, which is safe because a snapshot
    /// only restores ended runs as history and interrupted ones as resumable, and the end edge
    /// that reaches this method fired (or will fire) in the process that watches the child.
    pub fn death_facts(&self, run: RunId) -> Option<DeathFacts> {
        let mut inner = self.inner.lock();
        let live = inner.runs.get(&run)?;
        if live.death_noted {
            return None;
        }
        // **The arm M67 added, and the feature does not work without it.** A run that wound
        // down as it was asked to exits **0** — an opencode or codex wind-down turn is a whole
        // process that finishes and leaves — and the rule above read that as "it believes it
        // finished" and said nothing at all. So the best outcome of a graceful stop would have
        // left no trace on the board whatsoever.
        //
        // Widened for a *stopped* run only, deliberately. The exit-0 refusal is about a run that
        // decided for itself that it was done, and second-guessing that judgement belongs to the
        // orchestrator's own review; a run cide stopped decided nothing, and its clean exit is
        // the success of the wind-down rather than a verdict to leave unremarked. Widening the
        // gate generally would put an automatic line on every clean run, and a board where every
        // run carries one is a board where the lines that matter are buried.
        let stopped = live.stop.clone();
        // A retired run's task is `done`: there is nothing left to report to it. See
        // `StopHow::Retired`.
        if stopped
            .as_ref()
            .is_some_and(|stop| stop.how == StopHow::Retired)
        {
            return None;
        }
        let code = match live.state {
            RunState::Failed { .. } => None,
            RunState::Finished { code } if code != 0 => Some(code),
            RunState::Finished { code } if stopped.is_some() => Some(code),
            _ => return None,
        };
        let task = live.task.clone()?;
        let project = live.project;
        let agent = live.agent.clone();
        let agent_label = live.agent_label.clone();
        let survived = inner.runs.values().any(|other| {
            other.run != run
                && other.project == project
                && other.agent == agent
                && other.task.as_ref() == Some(&task)
                && !matches!(
                    other.state,
                    RunState::Finished { .. } | RunState::Failed { .. }
                )
        });
        inner
            .runs
            .get_mut(&run)
            .expect("present four lines up, and the lock never left this frame")
            .death_noted = true;
        if survived {
            return None;
        }
        Some(DeathFacts {
            task,
            agent_label,
            code,
            stop: stopped,
        })
    }

    /// What the stop-time comment needs: the run's task, its label, and what was asked. (M67)
    ///
    /// `death_facts`' shape — the decision half here, the sentence and the write in
    /// `agent_rpc` — and the same reason: the caller should do no registry reasoning of its own.
    ///
    /// `None` for a run with no task (there is nowhere to write) and for any stop that is not a
    /// wind-down, because every other road is over by the time it returns and says everything it
    /// has to say in one epitaph. This comment exists **only** for the road with a window: up to
    /// the whole grace passes before anything else is written, and a cide that quits inside it
    /// would otherwise take the caller's reason with it.
    pub fn stop_asked_facts(&self, run: RunId) -> Option<StopAsked> {
        let inner = self.inner.lock();
        let live = inner.runs.get(&run)?;
        let stop = live.stop.as_ref()?;
        if !stop.how.is_winding_down() {
            return None;
        }
        Some(StopAsked {
            task: live.task.clone()?,
            agent_label: live.agent_label.clone(),
            by: stop.by,
            reason: stop.reason.clone(),
            grace_secs: stop.grace_secs,
        })
    }

    /// What pressing **Open** on `run` should do. (M42)
    ///
    /// The ladder, in order, and the order is the design:
    ///
    /// 1. **The child is alive** in `sessions` — mirror it. For `claude` that is the real TUI
    ///    mid-turn; for `opencode` the rendered event stream of a turn in flight. A second
    ///    harness process on a live conversation is not a thing either CLI supports, so this
    ///    arm is never skipped in favour of the next.
    /// 2. **The conversation can be re-opened** — the real harness, spawned by the pane:
    ///    `claude --resume <session>` from the run's cwd when Claude Code still holds the
    ///    transcript there ([`crate::lifecycle::resumable`], asked now rather than trusted from
    ///    `reopenable`, because a worktree can be deleted between the two); `opencode --session
    ///    <ses_…>` whenever the run named one — that CLI's store is not cide's to check.
    /// 3. **The registry still retains the exited session** — mirror what it kept, which is
    ///    what Open showed before this ladder existed and is still right for an opencode run
    ///    whose stream is the only record, or a claude transcript that vanished.
    /// 4. **Unavailable**, with the reason in the sentence: queued, never named a conversation,
    ///    `--resume` injected off, or the transcript gone with its worktree.
    ///
    /// `root` is the project's, for a restored row whose `cwd` was never recorded in this
    /// process; the worktree name it stamped at dispatch is enough to find the directory.
    /// `CoreError::Io` for a run this project does not have, on `worktree_refusal`'s trade:
    /// nothing branches on the tag, the sentence is the answer.
    pub fn open_plan(
        &self,
        project: ProjectId,
        run: RunId,
        sessions: &SessionRegistry,
        root: &std::path::Path,
        resume_enabled: bool,
    ) -> Result<RunOpen> {
        self.open_plan_with(
            project,
            run,
            sessions,
            root,
            resume_enabled,
            |harness, cwd, session| {
                crate::lifecycle::resumable_on(harness, cwd, session, resume_enabled)
            },
        )
    }

    /// [`Self::open_plan`] with the transcript question injected, so a test can answer it
    /// without a `~/.claude` — the same split `lifecycle::plan_restore_in` makes for the same
    /// reason. `resume_enabled` still travels separately: it also decides the *sentence* for a
    /// claude row that cannot be re-opened.
    fn open_plan_with(
        &self,
        project: ProjectId,
        run: RunId,
        sessions: &SessionRegistry,
        root: &std::path::Path,
        resume_enabled: bool,
        transcript: impl Fn(Harness, &std::path::Path, SessionId) -> bool,
    ) -> Result<RunOpen> {
        let inner = self.inner.lock();
        let live = inner
            .runs
            .get(&run)
            .filter(|live| live.project == project)
            .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
        let cwd = run_cwd(root, live);
        let conversation = conversation_of(live, &cwd);

        if let Some(session) = live.session
            && let Some(pty) = sessions.get(session)
            && !pty.has_exited()
        {
            return Ok(RunOpen::Mirror {
                session,
                continues: conversation,
            });
        }

        if let Some(conversation) = conversation.clone() {
            let can = match live.harness {
                Harness::Claude | Harness::Qwen => live
                    .session
                    .is_some_and(|session| transcript(live.harness, &cwd, session)),
                Harness::Opencode | Harness::Codex | Harness::Mimo => true,
            };
            if can {
                return Ok(RunOpen::Continue { conversation });
            }
        }

        if let Some(session) = live.session
            && sessions.get(session).is_some()
        {
            return Ok(RunOpen::Mirror {
                session,
                continues: conversation,
            });
        }

        let reason = if live.state == RunState::Queued {
            "this run has not started yet, so there is no conversation to open. A queued run has \
             no child — it is waiting for a slot, or for its checkout to free."
                .to_string()
        } else {
            match live.harness {
                Harness::Opencode | Harness::Codex | Harness::Mimo => {
                    "this run never reported a conversation id, so there is \
                                      nothing to open. Its harness mints its own id and prints \
                                      it, and this child ended before printing one."
                        .to_string()
                }
                Harness::Claude if !resume_enabled => "Settings → Claude sessions has the \
                                                       `--resume` injection switched off, so \
                                                       cide will not name a conversation on the \
                                                       command line and cannot re-open this one."
                    .to_string(),
                Harness::Claude | Harness::Qwen => format!(
                    "its transcript is gone. {} kept it under {}, and nothing is there now — \
                     the worktree may have been removed.",
                    harness_label(live.harness),
                    cwd.display()
                ),
            }
        };
        Ok(RunOpen::Unavailable { reason })
    }

    /// Record that `session` — a pane's child — is the real harness on `conversation`. (M42)
    ///
    /// Called by `cmd::session::session_spawn` once the child is in the session registry, so
    /// a respawn that races the spawn finds the row. See [`Inner::viewers`].
    pub fn note_viewer(&self, conversation: &HarnessSession, session: SessionId) {
        self.inner
            .lock()
            .viewers
            .insert((conversation.harness, conversation.id.clone()), session);
    }

    /// Drop every viewer row naming `session`. Called from `lifecycle::report_exit` when the
    /// pane's child is reaped — the earliest moment the conversation is free again.
    pub fn forget_viewer(&self, session: SessionId) {
        self.inner
            .lock()
            .viewers
            .retain(|_, viewer| *viewer != session);
    }

    /// Sessions of runs that are idle with a child still alive **in this checkout**.
    ///
    /// The queue's answer to a real hazard: an `Idle` run has released its slot but its `claude`
    /// is still sitting in its worktree, so starting the next run there would put two
    /// processes in one checkout — exactly what the isolation exists to prevent.
    /// [`cide_ipc::RunState::Idle`]'s doc names the two ways out ("delivers the next turn into it
    /// or winds it down first"); this is the wind-down, taken only at the moment the checkout is
    /// actually needed, so an idle run that nothing is queued behind stays open to be read.
    ///
    /// Scoped by checkout **name**, not by role, since worktrees went per-task: an idle claude
    /// of the same role parked in a *different* task's checkout is in nobody's way, and killing
    /// it would repeat run 06202dd6's death with a different murder weapon. The name compared
    /// is the stamped [`LiveRun::checkout`], which is the directory that run's child actually
    /// stands in — computed by the same `checkout_name` at its own dispatch.
    fn idle_children_in(
        &self,
        project: ProjectId,
        checkout: &str,
        except: RunId,
    ) -> Vec<SessionId> {
        let inner = self.inner.lock();
        inner
            .runs
            .values()
            .filter(|run| {
                run.project == project
                    && run.checkout.as_deref() == Some(checkout)
                    && run.run != except
                    && run.state == RunState::Idle
                    // The same predicate the admission gate uses, negated: a run that still
                    // holds its checkout is not one to reclaim it from. Without this, a run
                    // stopped mid-turn is killed here in the window between the interrupted
                    // turn handing back and its wind-down turn starting — the graceful stop
                    // becomes a hard kill, the final comment is never written, and nothing
                    // anywhere says why.
                    && !holds_its_checkout(run)
            })
            .filter_map(|run| run.session)
            .collect()
    }

    /// Drop the oldest finished runs of every project past [`RECENT_KEPT`].
    fn forget_old(&self) {
        let mut inner = self.inner.lock();
        let mut done: HashMap<ProjectId, Vec<(u64, RunId)>> = HashMap::new();
        for run in inner.runs.values() {
            if matches!(
                run.state,
                RunState::Finished { .. } | RunState::Failed { .. }
            ) {
                done.entry(run.project)
                    .or_default()
                    .push((run.seq, run.run));
            }
        }
        for (_, mut runs) in done {
            if runs.len() <= RECENT_KEPT {
                continue;
            }
            runs.sort_unstable();
            for (_, run) in runs.drain(..runs.len() - RECENT_KEPT) {
                inner.runs.remove(&run);
            }
        }
    }
}

// ==========================================================================================
// Pause and resume. (M18)
// ==========================================================================================

/// The two signals a pause is made of.
///
/// **Deliberately not [`crate::lifecycle::Rung`]**, whose own doc refuses to grow these: every
/// value of that enum is something the shutdown ladder may hand a child on the way out, and a
/// `Rung::Stop` there would make the ladder spend every grace it has waiting for a process it had
/// just rendered incapable of exiting. These two are not rungs of anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PauseSignal {
    Stop,
    Cont,
}

/// One session to thaw, and what the registry knows about it.
#[derive(Debug, Clone, Copy)]
struct Thawing {
    session: SessionId,
    /// `None` for a project's primary console session, which has no row.
    run: Option<RunId>,
}

/// Whether a thaw is worth watching for a turn that died under the freeze.
///
/// A free function over injected clock values, because it is the whole of the stale-turn rule and
/// the only part of it a test can pin without freezing a real process for a minute.
///
/// Two conditions, and both are needed:
///
/// * **the pre-freeze state was live.** `RunState::Running | AwaitingPermission` — deliberately
///   the same set `cide_ipc::SessionState::is_live` names, because the two enums keep their
///   vocabulary aligned on purpose and a second definition of "live" here would be a second thing
///   to get wrong. `AwaitingPermission` is included even though a CLI blocked on a permission
///   prompt has no request outstanding at that instant: the turn around it is unfinished, and
///   being over-inclusive costs at most an offer the user declines, while being under-inclusive
///   costs a turn that dies in silence.
/// * **the freeze outlasted [`STALE_FREEZE_MS`].** Nothing shorter can have timed anything out.
///
/// `saturating_sub`, because the clock is wall clock and can go backwards — an NTP step or a
/// suspend/resume during the freeze — and a backwards clock must read as "no time passed" rather
/// than as an enormous freeze that raises a warning on every row.
fn freeze_may_have_killed_the_turn(frozen: &RunState, paused_at_ms: u64, now_ms: u64) -> bool {
    matches!(frozen, RunState::Running | RunState::AwaitingPermission)
        && now_ms.saturating_sub(paused_at_ms) >= STALE_FREEZE_MS
}

/// Move a run to `next`, keeping its worked clock. **The one place `LiveRun::state` is assigned.**
///
/// A run's worked clock runs only while [`RunState::counts_as_work`] admits its state, and the
/// only moment that clock can be started or stopped is the *transition* — a state observed over
/// and over says nothing about how long it has been held. [`AgentRegistry::set_state`] already
/// makes that argument for the slot release and the orchestrator nudge; this is the same
/// argument for the clock, factored out because six callers write the state inline, under the
/// lock, without going through that method.
///
/// Takes `&mut LiveRun` and no lock, so it is callable from every one of them — including
/// `plan_failover`, whose comment records that it may not call `set_state` because that method
/// takes the mutex it is already holding.
///
/// # Both edges, never one
///
/// Accumulating on the way out without stamping on the way in loses every interval, and
/// stamping without accumulating loses every interval but the last. Neither has a symptom: the
/// figure is simply a smaller wrong number than the one this replaces, on a row nobody
/// cross-checks against a stopwatch. A `_ => {}` arm for work→work and wait→wait is correct and
/// is written as one, because a transition between two working states (`Running` → `Starting`,
/// which is what a failover respawn does) must leave the open interval alone: closing and
/// reopening it would be the same total, but only by accident of the two happening at one
/// instant.
fn move_to(live: &mut LiveRun, next: RunState, now_ms: u64) {
    match (live.state.counts_as_work(), next.counts_as_work()) {
        (true, false) => {
            if let Some(since) = live.working_since_unix_ms.take() {
                // `saturating_sub`, for `freeze_may_have_killed_the_turn`'s stated reason in a
                // second place: this is wall clock and it can go backwards — an NTP step, a
                // suspend and resume — and a backwards clock must read as no time passed rather
                // than as an enormous interval that lands permanently in a run's total.
                live.worked_ms = live.worked_ms.saturating_add(now_ms.saturating_sub(since));
            }
        }
        (false, true) => live.working_since_unix_ms = Some(now_ms),
        (true, true) | (false, false) => {}
    }
    live.state = next;
}

/// What this run's worked figure is **right now**, closing the open interval if there is one.
///
/// One producer for the sum, because two places computing a number give two plausible answers —
/// the rule `cide_git::push::preview` states. The wire deliberately carries the two halves
/// rather than this, so a live row can tick between broadcasts; this is for the callers that
/// need a single closed number, which today is the snapshot.
fn worked_now(live: &LiveRun, now_ms: u64) -> u64 {
    live.worked_ms.saturating_add(
        live.working_since_unix_ms
            .map_or(0, |since| now_ms.saturating_sub(since)),
    )
}

/// Freeze one run, under the lock. `None` when there is nothing to freeze.
///
/// The write half of the ordering the module header describes: by the time this returns, the run
/// is `Paused` in a registry nothing has signalled yet.
fn freeze_run(live: &mut LiveRun, now_ms: u64) -> Option<SessionId> {
    // A queued run has no child. Nothing to stop, and marking it `Paused` would take it out of
    // the queue's vocabulary for a state it can return to only through a spawn.
    let session = live.session?;
    if live.frozen.is_some()
        || matches!(
            live.state,
            RunState::Finished { .. } | RunState::Failed { .. }
        )
    {
        return None;
    }
    live.frozen = Some(Frozen {
        state: live.state.clone(),
        at_unix_ms: now_ms,
    });
    move_to(
        live,
        RunState::Paused {
            since_unix_ms: now_ms,
        },
        now_ms,
    );
    Some(session)
}

impl AgentRegistry {
    /// Close this project's queue and freeze its children — or freeze one run.
    ///
    /// `run: None` is the **project scope**, and it is one command with a nullable argument
    /// rather than two, because the two scopes share a queue, a signal and a refusal path;
    /// splitting them would be two implementations of one refusal.
    ///
    /// # The order, which is the whole of the correctness here
    ///
    /// Mark, then signal, then emit. See the module header for the bug the other order produces.
    ///
    /// # Whether pause-all also freezes the project's primary session
    ///
    /// **It does**, and the argument is worth having in both directions because it is the one
    /// judgement call in this slice.
    ///
    /// *For*, which is why it is the behaviour: the primary session is the **orchestrator**. It
    /// is the process that decomposes goals, dispatches runs and nudges itself when one finishes,
    /// so freezing the runs and leaving it alive freezes the workers and leaves the foreman
    /// going — still spending a turn, still holding a model request open, and still trying to
    /// dispatch into a queue that has just been shut, where every attempt comes back as a refusal
    /// it will spend more tokens reasoning about. "A pause that survives lunch" is not delivered
    /// by a pause that leaves the most expensive process in the project running.
    ///
    /// *Against*, which is the cost taken knowingly: this is the pane the user types into. It is
    /// their console, not a worker, and freezing it means keystrokes stop having an effect in the
    /// window they are looking at. Two consequences follow and both are load-bearing —
    ///
    /// * **Resume must be reachable from a control that is not the frozen pane.** That control is
    ///   the Agents panel header, which is why `AgentRoster::Ready::dispatching` is on the wire:
    ///   the header has to be able to draw Resume for a project whose runs are all finished.
    ///   Without that, a user can freeze their own console with no way back.
    /// * **A session frozen while `AwaitingPermission` is indistinguishable on screen from a live
    ///   one**, because the modal is still painted over a child that can no longer read the
    ///   answer. The `SessionState::Paused` this emits is the only thing that separates them, and
    ///   it is why the badge is not optional.
    ///
    /// Freezing the primary is also safe in the one place it looks dangerous: its `cide-hook`
    /// children are in the same process group and freeze with it, and `hooks::handle` gives every
    /// connection its own reader thread precisely so that "a hook that connects and never writes
    /// blocks only itself".
    pub fn pause(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: Option<RunId>,
    ) -> Result<()> {
        // Resolved before the lock is taken, because it reads the workspace — and this lock is
        // taken from the hook applier thread, so holding it across another lock is how two
        // orderings become a deadlock.
        let primary = match run {
            Some(_) => None,
            None => primary_session(app, project),
        };

        let frozen = self.take_freezes(project, run, primary, now_unix_ms())?;

        // Only now. Everything above is a registry that has already stopped dispatching.
        for session in &frozen {
            signal_session(app, *session, PauseSignal::Stop);
            crate::emit::session_state(app, &session.to_string(), SessionState::Paused);
        }
        self.mark_changed(app, project);
        Ok(())
    }

    /// Mark, and answer with the sessions to freeze. **The whole of the mark happens here.**
    ///
    /// Separate from [`Self::pause`] so the ordering is testable: a test can call this and assert
    /// that the queue is already shut and the rows already say `Paused` at a point where no
    /// signal has been sent, which is the property the module header calls load-bearing.
    fn take_freezes(
        &self,
        project: ProjectId,
        run: Option<RunId>,
        primary: Option<SessionId>,
        now_ms: u64,
    ) -> Result<Vec<SessionId>> {
        let mut inner = self.inner.lock();
        let mut frozen: Vec<(SessionId, Option<RunId>)> = Vec::new();

        match run {
            Some(run) => {
                let live = inner
                    .runs
                    .get_mut(&run)
                    .filter(|live| live.project == project)
                    .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
                // Refused rather than shrugged off, because the two ways this can be nothing to
                // do want different sentences and the row that offered the button is drawn from
                // one of them.
                if live.session.is_none() {
                    return Err(CoreError::Io(
                        "this run has not started yet, so there is no child to freeze. Pause the \
                         project to hold it in the queue, or stop it."
                            .into(),
                    ));
                }
                if matches!(
                    live.state,
                    RunState::Finished { .. } | RunState::Failed { .. }
                ) {
                    return Err(CoreError::Io("this run has already ended".into()));
                }
                if let Some(session) = freeze_run(live, now_ms) {
                    frozen.push((session, Some(run)));
                }
            }
            None => {
                // **First**, and before any run is touched: from here on nothing new is admitted,
                // whatever else happens below or on another thread.
                inner.paused_projects.insert(project);

                let runs: Vec<RunId> = inner
                    .runs
                    .values()
                    .filter(|live| live.project == project)
                    .map(|live| live.run)
                    .collect();
                for run in runs {
                    let Some(live) = inner.runs.get_mut(&run) else {
                        continue;
                    };
                    if let Some(session) = freeze_run(live, now_ms) {
                        frozen.push((session, Some(run)));
                    }
                }

                // And the orchestrator. See [`Self::pause`] for the argument.
                if let Some(primary) = primary
                    && !inner.frozen_sessions.contains_key(&primary)
                {
                    frozen.push((primary, None));
                }
            }
        }

        for (session, run) in &frozen {
            inner
                .frozen_sessions
                .insert(*session, FrozenSession { project, run: *run });
        }
        Ok(frozen.into_iter().map(|(session, _)| session).collect())
    }

    /// `SIGCONT`, reopen the queue, drain it, and tell the windows.
    ///
    /// The mirror of [`Self::pause`], including the order: the children are thawed **before** the
    /// marks are cleared, so no admission can reach a child that is still frozen — which is the
    /// same hazard the mark-first rule closes from the other side.
    ///
    /// # Resume is on the settings as they stand now
    ///
    /// A `SIGCONT` changes nothing about a process: the model, provider and limits a child was
    /// forked with are on its argv and in its environment, fixed at the fork. So a run whose
    /// settings moved while it was paused is not thawed — it is **restarted**: rebound to a
    /// fresh session, its old child wound down, and a new child forked on the current settings
    /// continuing the same conversation, through the same road a pool failover takes
    /// ([`Self::fork_failover`]), which keeps the slot, the worktree and the screen. A run whose
    /// settings did not move is thawed in place, and that distinction is the whole reason the
    /// comparison exists: restarting every resumed run would cost every short pause its
    /// in-flight turn. [`Self::plan_restarts`] is the decision; the ordering it needs is written
    /// there.
    pub fn resume(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: Option<RunId>,
    ) -> Result<()> {
        let thaws = self.plan_thaws(project, run)?;

        // What a child forked *now* would be forked with, per role. Read off the workspace and
        // the project's `.cide/` before any registry lock, for `pause`'s reason — this lock is
        // also taken from the hook applier thread — and once, because both halves of Resume
        // below compare against it.
        let settings_now = settings_now(app, project);
        let sessions = app.try_state::<SessionRegistry>();

        // Decided **before** the `SIGCONT`, and the order is load-bearing: a run being restarted
        // is rebound to its successor's session under the lock, so nothing the old child prints
        // or reports between the thaw and its death can reach the run — `respawn`'s
        // rebind-before-kill rule, applied here.
        let restarts =
            self.plan_restarts(project, sessions.as_deref(), &thaws, settings_now.as_ref());

        for thaw in &thaws {
            signal_session(app, thaw.session, PauseSignal::Cont);
        }

        let mut watch = self.finish_thaws(project, run.is_none(), &thaws, now_unix_ms());
        // A restarted run's old child is about to die on purpose; watching it for a sign of life
        // would raise a stale-turn offer on a run whose successor is already being forked.
        watch.retain(|(session, _)| !restarts.contains(session));

        // Only now: thawed, so the signal lands — a stopped process does not act on SIGHUP or
        // SIGTERM — and rebound, so the exit that follows belongs to nobody but `plan_restart`,
        // which forks the successor from the reaper thread once the old child is really gone.
        if let Some(sessions) = sessions.as_deref() {
            for old in &restarts {
                if let Some(pty) = sessions.get(*old) {
                    tracing::info!(session = %old, "winding down a paused run's child to restart it on the current settings");
                    pty.kill();
                }
            }
        }

        // The other thing Resume can mean since the snapshot landed: runs whose children died
        // with a cide restart go back through the queue, continuing their conversations. Before
        // the pump, so this press is what starts them; through the queue, because an
        // interrupted run holds no slot and its role's worktree may meanwhile belong to a
        // newer run — admission is the arbiter, exactly as for a dispatch.
        self.requeue_interrupted(project, run, sessions.as_deref(), settings_now.as_ref())?;

        // The queue is open again, so whatever was waiting may start.
        self.pump(app);

        for thaw in &thaws {
            // What cide last heard this session say, which is still the pre-freeze state: the
            // hook map is written only by `hooks::decide` from frames, and a frozen child's
            // `cide-hook` children are frozen with it, so nothing wrote to it under the freeze.
            // That makes it the honest post-thaw answer as well as the pre-freeze one.
            let state = app
                .try_state::<crate::hooks::HookServer>()
                .map_or(SessionState::Idle, |hooks| hooks.state(thaw.session));
            crate::emit::session_state(app, &thaw.session.to_string(), state);
        }
        for (session, run) in watch {
            self.watch_thaw(app, project, session, run);
        }
        self.mark_changed(app, project);
        Ok(())
    }

    /// Which sessions a resume would thaw. **Reads only** — see [`Self::resume`] for why.
    fn plan_thaws(&self, project: ProjectId, run: Option<RunId>) -> Result<Vec<Thawing>> {
        let inner = self.inner.lock();
        match run {
            Some(run) => {
                let live = inner
                    .runs
                    .get(&run)
                    .filter(|live| live.project == project)
                    .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
                Ok(match (live.frozen.as_ref(), live.session) {
                    (Some(_), Some(session)) => vec![Thawing {
                        session,
                        run: Some(run),
                    }],
                    // Not an error. A row whose run finished under the freeze, or a second press
                    // of Resume while the first was in flight, is not something to show anybody.
                    _ => Vec::new(),
                })
            }
            None => Ok(inner
                .frozen_sessions
                .iter()
                .filter(|(_, frozen)| frozen.project == project)
                .map(|(session, frozen)| Thawing {
                    session: *session,
                    run: frozen.run,
                })
                .collect()),
        }
    }

    /// Clear the marks, reopen the queue, and answer with the thaws worth watching.
    ///
    /// Runs **after** the `SIGCONT`s. Returns the `(session, run)` pairs whose pre-freeze state
    /// was live and whose freeze outlasted [`STALE_FREEZE_MS`]; see
    /// [`freeze_may_have_killed_the_turn`].
    fn finish_thaws(
        &self,
        project: ProjectId,
        whole_project: bool,
        thaws: &[Thawing],
        now_ms: u64,
    ) -> Vec<(SessionId, RunId)> {
        let mut inner = self.inner.lock();
        if whole_project {
            inner.paused_projects.remove(&project);
        }

        let mut watch = Vec::new();
        for thaw in thaws {
            inner.frozen_sessions.remove(&thaw.session);
            let Some(run) = thaw.run else {
                // The primary console session. It has no row, so there is nothing to restore and
                // nothing to offer a retry on; the pane hears the state event and redraws.
                continue;
            };
            let Some(live) = inner.runs.get_mut(&run) else {
                continue;
            };
            let Some(frozen) = live.frozen.take() else {
                continue;
            };
            // Restored verbatim rather than re-derived: the run is exactly where it was, and
            // asking the harness would answer from a hook frame that never arrived.
            //
            // Through `move_to`, so a run that was working when it was frozen opens a *fresh*
            // interval here — which is what excludes the freeze. Note that nothing subtracts a
            // pause: the clock simply did not run, so `Frozen::at_unix_ms` keeps its one
            // existing purpose and this needs no arithmetic of its own.
            move_to(live, frozen.state.clone(), now_ms);
            if freeze_may_have_killed_the_turn(&frozen.state, frozen.at_unix_ms, now_ms) {
                watch.push((thaw.session, run));
            }
        }
        watch
    }

    /// Which of the sessions a Resume is about to thaw belong to runs whose settings have moved
    /// since their fork — and, for each, the whole of the mark. **Reads the settings through
    /// `settings_now` and nothing else off disk**, so a test drives it with a closure.
    ///
    /// Answers the sessions of the *old* children, which the caller winds down after the
    /// `SIGCONT`; the successor of each is forked by [`Self::plan_restart`] from that child's
    /// exit, through the failover road. Everything here happens under the lock, before any
    /// signal is sent, and in this order for these reasons:
    ///
    /// * **The comparison is against what the standing child got** (`LiveRun::forked_with`),
    ///   never a re-derivation: the question is whether the fork would differ, and only the
    ///   record of the fork can answer it.
    /// * **The run is rebound to a fresh session first.** From here the dying child's hook
    ///   frames, its output and its exit find no run that owns their session — which is what
    ///   makes it safe to `SIGCONT` a process cide is about to kill. A claude successor is
    ///   therefore a *fork* of the conversation (`--resume <old> --fork-session --session-id
    ///   <new>`, `harness::claude::respawn_spec`), which is also what keeps the old transcript
    ///   intact; opencode's and codex's carry the conversation in their own flags and mint cide
    ///   sessions freely anyway.
    /// * **The freeze is taken here**, so `finish_thaws` has nothing to restore on this run and
    ///   the row reads `Starting` — a working phase, so the slot and the checkout stay this run's
    ///   across the swap, exactly as `plan_failover` arranges it.
    /// * **A frozen child that had not begun its turn** (`Starting`, on its first prompt) has no
    ///   conversation worth a continuation line: the dispatch is replayed fresh, on the new
    ///   settings, and a claude run gets a new conversation rather than a fork of an empty one.
    ///   A `Starting` child on a later turn is an opencode respawn that never reported in; its
    ///   follow-up is re-sent into the conversation it was meant for.
    /// * **A changed pool is re-stamped** (`restamp`), so the successor starts at the top of the
    ///   list the person arranged; an unchanged pool keeps the run's place in it. This is the
    ///   one place the stickiness rule yields, and it yields to a person and never to a failover.
    /// * **A conversation a pane is driving is not restarted** (M42's rule, `viewed_by`): the row
    ///   says so and the run is thawed as it stands. Two harness processes must not write one
    ///   transcript, and the person typing into that pane outranks the settings screen.
    fn plan_restarts(
        &self,
        project: ProjectId,
        sessions: Option<&SessionRegistry>,
        thaws: &[Thawing],
        settings_now: &SettingsNow<'_>,
    ) -> Vec<SessionId> {
        let mut inner = self.inner.lock();
        let mut restarting = Vec::new();
        for thaw in thaws {
            let Some(run) = thaw.run else {
                continue;
            };
            let viewed = sessions.is_some_and(|sessions| {
                inner
                    .runs
                    .get(&run)
                    .is_some_and(|live| viewed_by(&inner, sessions, live).is_some())
            });
            let Some(live) = inner
                .runs
                .get_mut(&run)
                .filter(|live| live.project == project)
            else {
                continue;
            };
            // A frozen run with a child, and only that: a run that ended under the freeze has no
            // freeze left (`set_state`'s `over` arm), and a queued one never had a child.
            let (Some(frozen), Some(old)) = (live.frozen.as_ref(), live.session) else {
                continue;
            };
            if old != thaw.session {
                continue;
            }
            // Each outcome below is said once in the log, at info, because the one report this
            // was written from read "resuming, but nothing in logs": a Resume that decides to
            // change nothing must still be able to say so, and one that restarts must say what
            // moved.
            let Some(was) = live.forked_with.as_ref() else {
                tracing::info!(%run, "resume: this run's fork recorded no settings; thawing in place");
                continue;
            };
            let Some(now) = settings_now(&live.agent) else {
                tracing::info!(%run, agent = %live.agent, "resume: this role cannot be resolved now; thawing in place");
                continue;
            };
            let changed = was.changed_fields(&now);
            if changed.is_empty() {
                tracing::info!(%run, "resume: settings unchanged since this run's fork; thawing in place");
                continue;
            }
            if viewed {
                tracing::info!(%run, ?changed, "resume: settings changed, but a pane holds this run's conversation; thawing in place");
                live.note = Some(SETTINGS_KEPT_FOR_A_PANE.to_string());
                continue;
            }
            tracing::info!(%run, ?changed, "resume: settings changed since this run's fork; restarting it on the current ones");

            let pre = frozen.state.clone();
            let pool_changed = was.pool != now.pool;
            let never_began = matches!(pre, RunState::Starting);
            let resume = match never_began && live.turns <= 1 {
                true => None,
                false => resume_point(live).map(|point| point.conversation),
            };
            let prompt = match (&resume, never_began) {
                (Some(_), false) => continuation_prompt(
                    live.task.as_ref(),
                    live.task_title.as_deref(),
                    cide_hook_binary().and_then(|_| cide_agents::harness::for_kind(live.harness)),
                    Restarted::Settings,
                ),
                // The turn never began, or there is no conversation to continue: the prompt the
                // dying child was given is the prompt its successor gets.
                (Some(_), true) | (None, _) => live.prompt.clone(),
            };
            if resume.is_none() {
                // Nothing to continue, so nothing to go on naming — `plan_failover`'s clear, for
                // the same reason: left set, Open would show a conversation this run walked
                // away from.
                live.harness_session = None;
            }
            let next = SessionId::new();
            // `bind_session`, inline: the lock is held, and it must be — the rebind and the
            // decision are one atomic step or a frame could land between them.
            live.session = Some(next);
            live.frozen = None;
            live.stale_turn = false;
            move_to(live, RunState::Starting, now_unix_ms());
            if pool_changed {
                // Onto the new list's first entry without asking whether it has room: this is a
                // run that already holds its slot swapping its own child, not a new one taking a
                // slot, and it reaches the fork without passing admission. The overshoot is at
                // most one run for as long as this child lives, and only after a person edited
                // the pool under a paused run — cheaper than a restart that could refuse itself.
                restamp(live, &now.pool);
            }
            if resume.is_some() && !never_began {
                // A continuation line is a prompt into a conversation that carries real work,
                // so a later provider failure must continue it rather than abandon it — the
                // distinction `plan_failover` draws on this counter.
                live.turns = live.turns.saturating_add(1);
            }
            live.note = Some(
                match resume.is_some() {
                    true => RESTARTING_NOTE,
                    false => RESTARTING_FRESH_NOTE,
                }
                .to_string(),
            );
            let failover = Failover {
                run,
                project,
                agent: live.agent.clone(),
                task: live.task.clone(),
                task_title: live.task_title.clone(),
                change: live.change.clone(),
                prompt,
                resume,
                session: Some(next),
                previous: Some(old),
                why: Carried::Settings,
                purpose: live.purpose.clone(),
            };
            inner.frozen_sessions.remove(&old);
            inner.restarts.insert(old, failover);
            restarting.push(old);
        }
        restarting
    }

    /// The restart `plan_restarts` decided for the run whose old child was filed under
    /// `session`, taken once, from that child's exit — or `None`, which is every other exit.
    ///
    /// Taken and not read, like `plan_failover`'s latch: an exit forks one successor. Two
    /// refusals, both about a fork that must not happen after all. **Stop** was pressed while
    /// the old child was being wound down: `stop` latched `stopping` and signalled a session
    /// that has no child yet, so nothing else will end this run — it is ended here, as `stop`
    /// ends a run that never reached a child, or it would hold its slot for ever under a row
    /// nothing can press. And the process is **going down** (`snapshot_sealed`): the snapshot
    /// already carries this run as interrupted under its old session, and a successor forked
    /// into a teardown would be killed by the ladder it was born into.
    fn plan_restart(&self, session: SessionId) -> Option<Failover> {
        let mut inner = self.inner.lock();
        let failover = inner.restarts.remove(&session)?;
        let stopping = inner
            .runs
            .get(&failover.run)
            .is_none_or(|live| live.stopping);
        if stopping {
            drop(inner);
            self.fail(
                None,
                failover.run,
                "stopped while restarting on the current settings",
            );
            return None;
        }
        if self.snapshot_sealed.load(Ordering::SeqCst) {
            return None;
        }
        Some(failover)
    }

    /// Put [`RunState::Interrupted`] runs back on their roles' queues, marked continuing.
    ///
    /// `run: None` is the project scope — every interrupted run the project has — matching
    /// [`Self::resume`]'s own contract. A run id that names a non-interrupted run is a no-op
    /// rather than an error: Resume's other half (the thaw) may own it.
    /// Refuses — one run — or skips — the project scope — an interrupted run whose conversation
    /// a pane is driving right now (M42): the registry's child would be a second CLI on a
    /// transcript a person is typing into. The skipped row says so in its note; the refused
    /// press gets the sentence. `sessions` is what "right now" is asked of; `None` (a test
    /// without a registry) means nobody is viewing anything.
    ///
    /// `settings_now` is what the resume will fork with, per role, for one comparison: a run
    /// restored **on a pool** comes back at the position it had settled on (`SavedRun::pool`),
    /// which is right while the pool is the one it settled into and wrong once a person has
    /// edited it while cide was down — a position into a list that no longer exists. Such a
    /// run has its stamp cleared here, so the fork re-stamps from the top of the current pool.
    fn requeue_interrupted(
        &self,
        project: ProjectId,
        run: Option<RunId>,
        sessions: Option<&SessionRegistry>,
        settings_now: &SettingsNow<'_>,
    ) -> Result<()> {
        let one_run = run.is_some();
        let mut inner = self.inner.lock();
        let matching: Vec<RunId> = inner
            .runs
            .values()
            .filter(|live| live.project == project && live.state == RunState::Interrupted)
            .filter(|live| run.is_none_or(|one| one == live.run))
            .map(|live| live.run)
            .collect();
        for run in matching {
            let viewed = sessions.is_some_and(|sessions| {
                inner
                    .runs
                    .get(&run)
                    .is_some_and(|live| viewed_by(&inner, sessions, live).is_some())
            });
            /*
             * The second skip, and it exists because `Interrupted` stopped holding its pair in
             * M66. A run whose child died with a previous cide no longer blocks a dispatch — so
             * the user can perfectly well have started this role on this task again in the
             * meantime, and requeueing the old row would put the second run on the board that
             * the whole of M66 is about. The live one is the one that is actually going; this
             * one is history, and says so.
             */
            let held = inner.runs.get(&run).and_then(|live| {
                let task = live.task.as_ref()?;
                holder_in(&inner, live.project, &live.agent, task)
            });
            let Some(live) = inner.runs.get_mut(&run) else {
                continue;
            };
            if viewed {
                if one_run {
                    return Err(CoreError::Io(VIEWED_IN_A_PANE.into()));
                }
                live.note = Some(VIEWED_IN_A_PANE.to_string());
                continue;
            }
            if let Some(held) = held {
                let why = format!(
                    "{} is already going on this task again (run {}); this row is the \
                     conversation the restart ended, and resuming it would put two runs on one \
                     task.",
                    live.agent, held.run
                );
                if one_run {
                    return Err(CoreError::Io(why));
                }
                live.note = Some(why);
                continue;
            }
            // `Interrupted` → `Queued`: both are waiting, so this is a clock no-op today. It
            // goes through `move_to` anyway, because the invariant is that *every* assignment
            // does — a site that writes the state inline is the one that silently stops being a
            // no-op the day `counts_as_work` moves.
            move_to(live, RunState::Queued, now_unix_ms());
            live.continuing = true;
            live.note = Some(
                "resuming after a cide restart; a new child continues the same conversation"
                    .to_string(),
            );
            // Logged because one restart on the reporting machine put the continuation line
            // into three conversations **twice**, 17.6 seconds apart, with nothing between the
            // two in any of them — two children forked for one run. It cannot come from a
            // double press through here (this arm matches `Interrupted` only and leaves the run
            // `Queued` under the lock), and `restore_snapshot_from` is the only other writer of
            // `Interrupted`, so the second one arrived by a road nobody has named yet. These
            // two lines and `admit_a_pass`'s are what will name it; the guard comes after the
            // trace, not before it.
            tracing::info!(%run, agent = %live.agent, "resume: requeued an interrupted run to continue its conversation");
            if let Some(now) = settings_now(&live.agent)
                && now.pool != live.pool
                && !live.pool.is_empty()
            {
                restamp(live, &now.pool);
            }
            let key = live.key();
            inner.queues.entry(key).or_default().push_back(run);
        }
        Ok(())
    }

    /// Watch one thawed run for [`THAW_WATCH`], and raise `stale_turn` if nothing stirs.
    ///
    /// Two evidence channels, because neither is sufficient alone:
    ///
    /// * **a hook frame** keyed on this session, through [`AgentRegistry::saw_life`]. Precise, and
    ///   it only fires for frames that produce a state effect — a statusline frame does not reach
    ///   [`note_hook`] at all.
    /// * **a byte of PTY output**, through a sink attached for the window. Coarse and much more
    ///   likely to fire first: a `claude` whose request survived repaints its spinner within a
    ///   frame or two of `SIGCONT`, and one whose request died prints its error just as fast. The
    ///   sink returns `false` on its first delivery, so it removes itself (`broadcast` retains on
    ///   the return value) and can never accumulate unacknowledged credit or choke the session.
    ///
    /// A short-lived thread rather than a timer wheel, `mark_changed`'s shape: this happens at the
    /// rate a human presses Resume.
    fn watch_thaw(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        session: SessionId,
        run: RunId,
    ) {
        let alive = Arc::new(AtomicBool::new(false));
        self.inner.lock().life.insert(session, Arc::clone(&alive));

        // Attached after the `SIGCONT` rather than before it, which is safe because the freeze
        // that got us here lasted at least a minute: `cide-pty`'s coalescer has long since
        // flushed everything the child produced before the freeze, so the first byte this sink
        // sees is a byte the thawed child produced.
        let sink = app
            .try_state::<SessionRegistry>()
            .and_then(|sessions| sessions.get(session))
            .map(|pty| {
                let flag = Arc::clone(&alive);
                let id = pty.attach(Arc::new(move |_bytes: &[u8]| {
                    flag.store(true, Ordering::Release);
                    false
                }));
                (pty, id)
            });

        let registry = Arc::clone(self);
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("cide-agents-thaw".into())
            .spawn(move || {
                std::thread::sleep(THAW_WATCH);
                if let Some((pty, id)) = sink {
                    pty.detach(id);
                }
                registry.inner.lock().life.remove(&session);
                if alive.load(Ordering::Acquire) {
                    return;
                }
                if registry.mark_stale(run) {
                    registry.mark_changed(&app, project);
                }
            });
        if let Err(error) = spawned {
            // The offer is lost; nothing else is. Worth a line because the user then has a run
            // that may be sitting on a dead turn with nothing on the row to say so.
            tracing::warn!(%error, %run, "no thread to watch a thawed run; no stale-turn offer for it");
            self.inner.lock().life.remove(&session);
        }
    }

    /// Something happened on this session. Used by the thaw watch; a no-op the rest of the time.
    ///
    /// Deliberately cheap enough to sit on the hook applier thread, whose ordering guarantee is a
    /// correctness requirement: one lock, one map lookup, one atomic store.
    fn saw_life(&self, session: SessionId) {
        if let Some(alive) = self.inner.lock().life.get(&session) {
            alive.store(true, Ordering::Release);
        }
    }

    /// Raise the stale-turn offer on a run. Answers whether it moved.
    fn mark_stale(&self, run: RunId) -> bool {
        let mut inner = self.inner.lock();
        let Some(live) = inner.runs.get_mut(&run) else {
            return false;
        };
        // Every one of these means there is no suspicion left to raise: the flag is already up,
        // the run was frozen again while the watch ran, its turn demonstrably ended, or it is
        // over. `Idle` in particular is the good outcome — the turn came back.
        if live.stale_turn
            || live.frozen.is_some()
            || matches!(
                live.state,
                RunState::Idle | RunState::Finished { .. } | RunState::Failed { .. }
            )
        {
            return false;
        }
        live.stale_turn = true;
        tracing::info!(%run, "a thawed run showed no sign of life; offering a retry");
        true
    }

    /// Re-send the last dispatched prompt into a run whose turn is presumed dead.
    ///
    /// **The only command in this module that spends the user's quota**, which is why it is a
    /// command of its own rather than `agents_ack_stale_turn(retry: true)`, and why it refuses
    /// when there is no offer outstanding: the flag is cleared under the lock, so a double press
    /// sends one prompt and gets a sentence for the second.
    ///
    /// The prompt is the one the registry recorded at dispatch ([`LiveRun::prompt`]) and not one
    /// recomposed from the task, which would silently drop whatever extra instruction the
    /// dispatch carried. How it is delivered is the harness's decision
    /// ([`cide_agents::Harness::deliver`]), not this module's.
    pub fn retry_turn(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: RunId,
    ) -> Result<()> {
        let (session, prompt, kind) = {
            let mut inner = self.inner.lock();
            let live = inner
                .runs
                .get_mut(&run)
                .filter(|live| live.project == project)
                .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
            if !live.stale_turn {
                return Err(CoreError::Io(
                    "this run has no interrupted turn to retry. Re-sending a prompt that arrived \
                     would spend a second turn on work the agent has already done."
                        .into(),
                ));
            }
            if live.frozen.is_some() {
                return Err(CoreError::Io(
                    "this run is paused. Resume it before re-sending its prompt — a write into a \
                     stopped child is read on resume, out of order with whatever it was doing."
                        .into(),
                ));
            }
            let session = live
                .session
                .ok_or_else(|| CoreError::Io("this run has no child to send a prompt to".into()))?;
            live.stale_turn = false;
            (session, live.prompt.clone(), live.harness)
        };

        // Everything below can refuse, and the offer was already taken off the row under the lock
        // — which is what makes a double press send one prompt. So a refusal puts it back: the
        // user is owed the choice they have not in fact spent yet.
        let sent = self.send_prompt(app, project, run, session, &prompt, kind);
        if sent.is_err()
            && let Some(live) = self.inner.lock().runs.get_mut(&run)
        {
            live.stale_turn = true;
        }
        self.mark_changed(app, project);
        sent
    }

    /// Send one prompt to a live run, the way its harness says to. See [`Self::retry_turn`].
    fn send_prompt(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: RunId,
        session: SessionId,
        prompt: &str,
        kind: Harness,
    ) -> Result<()> {
        let harness = cide_agents::for_kind(kind).ok_or_else(|| {
            CoreError::Io(format!(
                "this build has no implementation for the `{kind:?}` harness"
            ))
        })?;
        match harness.deliver(prompt) {
            Delivery::Stdin(bytes) => {
                let pty = app
                    .try_state::<SessionRegistry>()
                    .and_then(|sessions| sessions.get(session))
                    .ok_or_else(|| CoreError::Io("this run's child is no longer running".into()))?;
                type_submitted_line(app, session, &pty, bytes);
                Ok(())
            }
            // A real answer rather than a failure — see `cide_agents::Delivery`.
            Delivery::Respawn => self.respawn(app, project, run, prompt),
        }
    }

    /// The half of [`Self::respawn`] that happens under the lock, and therefore the half a test
    /// can drive.
    ///
    /// Split out for `plan_thaws`' reason, one screen up: reporting needs an `AppHandle`, this
    /// build cannot make one, and a respawn that could only be exercised by running the app is a
    /// respawn whose refusal nobody ever reads. Everything that decides *whether* and *what* is
    /// here; everything that acts is above.
    fn plan_respawn(&self, project: ProjectId, run: RunId, prompt: &str) -> Result<Continuation> {
        let mut inner = self.inner.lock();
        let live = inner
            .runs
            .get_mut(&run)
            .filter(|live| live.project == project)
            .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
        // The one refusal this path owns. A run whose child died before printing a single line
        // never told cide what its conversation was called, and there is nothing to continue:
        // starting a fresh one would spend a turn re-doing work from an empty context while the
        // row claimed the original had carried on.
        let harness_session = live.harness_session.clone().ok_or_else(|| {
            CoreError::Io(
                "this run never reported a conversation id, so there is nothing to continue. Its \
                 harness mints its own id and prints it, so a run that produced no output at all \
                 has to be dispatched again."
                    .into(),
            )
        })?;
        let planned = Ok(Continuation {
            // **The same `RunId`.** Not a new one, which is what makes this a follow-up rather
            // than a second dispatch — see the four rules on `respawn`.
            admission: Admission {
                run,
                project,
                agent: live.agent.clone(),
                task: live.task.clone(),
                task_title: live.task_title.clone(),
                change: live.change.clone(),
                prompt: prompt.to_string(),
                // A retry's continuation travels as `bring_up`'s explicit argument, not here:
                // this admission never goes through the queue, so there is no later point at
                // which a `ResumePoint` would be read.
                resume: None,
                purpose: live.purpose.clone(),
            },
            previous: live.session,
            harness_session,
        });
        // A **new prompt**, so this run now has a conversation worth keeping: from here a
        // provider failure is a mid-conversation one and must reuse the session rather than
        // abandon it. A failover does not come through here, which is exactly right — it re-sends
        // the same prompt and is not a turn. (M45)
        if let Some(live) = inner.runs.get_mut(&run) {
            live.turns = live.turns.saturating_add(1);
        }
        planned
    }

    /// Continue a run whose CLI cannot be spoken to, by starting the child that continues it.
    ///
    /// # The four things this must not get wrong
    ///
    /// 1. **It is the same run.** No [`Self::enqueue`], no admission, no second [`RunId`] — the
    ///    row, the task link, the dispatch time and the history are one run's, because that is
    ///    what a follow-up *is*. A respawn that went through the queue would show the user two
    ///    rows for one piece of work and account for it twice.
    /// 2. **It takes no second slot and no second worktree.** Nothing here touches
    ///    `agent_slots`/`project_slots`, and it must not: the slot this run took at admission is
    ///    still held. That is not luck — `retry_turn` is the only caller and it refuses unless
    ///    the run carries a stale-turn offer, which `mark_stale` refuses to raise on a run that
    ///    is `Idle`, `Finished` or `Failed`. So the run reaching here is one still inside the
    ///    working set, holding the slot its successor will go on holding. `start_child`'s own
    ///    `worktree::ensure` is idempotent and answers with the same checkout.
    /// 3. **The old child dies before the new one starts.** One checkout, one process — the whole
    ///    of what worktree isolation means. The old `opencode` has almost always exited already
    ///    (`run` is one turn per process), but "almost always" is not the guarantee, and a stale
    ///    turn is precisely the case where cide is unsure what the child is doing.
    /// 4. **The rebind happens before the kill.** The run's [`SessionId`] is moved to the new
    ///    one *first*, so the old child's exit — which arrives on the reaper thread moments later
    ///    — finds no run that owns that session and therefore cannot mark this run `Finished`.
    ///    Swapping these two lines would end the run at the exact moment it was being continued.
    ///
    /// # Why it answers before the child exists
    ///
    /// The fork, the disk read and the worktree check happen on a task, exactly as a dispatch's
    /// do, for `enqueue`'s reason: a command that waited on a spawn would let one slow checkout
    /// hold a Tauri worker, and the caller here is a user pressing Retry rather than a caller
    /// that can use the child. A failure after this point is reported the way a dispatch's is —
    /// the run goes `Failed` with the sentence on its row — rather than as a return value nobody
    /// is still waiting for.
    fn respawn(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: RunId,
        prompt: &str,
    ) -> Result<()> {
        // Refused before anything is planned or killed: a person may be typing into this very
        // conversation in a pane (M42), and a second CLI on it would write the same transcript
        // from two processes. See `Inner::viewers`.
        if let Some(sessions) = app.try_state::<SessionRegistry>() {
            let inner = self.inner.lock();
            if let Some(live) = inner.runs.get(&run)
                && viewed_by(&inner, &sessions, live).is_some()
            {
                return Err(CoreError::Io(VIEWED_IN_A_PANE.into()));
            }
        }
        let Continuation {
            admission,
            previous,
            harness_session,
        } = self.plan_respawn(project, run, prompt)?;

        // Rebound before anything is killed, and before the fork for `bind_session`'s own reason.
        // See point 4 above: this is what makes the old child's exit belong to nobody.
        let session = SessionId::new();
        self.bind_session(run, session);

        // Point 3. The dead session deliberately stays in `SessionRegistry` — `report_exit` keeps
        // it there so a pane that was showing this run can still paint the last thing it printed,
        // and a respawn is not a reason to blank a window somebody is reading.
        if let Some(previous) = previous
            && let Some(sessions) = app.try_state::<SessionRegistry>()
            && let Some(pty) = sessions.get(previous)
        {
            tracing::info!(%run, session = %previous, "winding down a run's previous child before continuing it");
            pty.kill();
        }

        let registry = Arc::clone(self);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            registry
                .bring_up(app, admission, session, Some(harness_session), None)
                .await;
        });
        Ok(())
    }

    /// Take the stale-turn offer off a run and do nothing else.
    pub fn ack_stale_turn(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: RunId,
    ) -> Result<()> {
        {
            let mut inner = self.inner.lock();
            let live = inner
                .runs
                .get_mut(&run)
                .filter(|live| live.project == project)
                .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
            live.stale_turn = false;
        }
        self.mark_changed(app, project);
        Ok(())
    }

    /// Fork this run's successor on the next pool candidate. The acting half. (M45)
    ///
    /// `bring_up` does the rest, and it is the *same* path a first start and a respawn take —
    /// which is what makes a failover's child indistinguishable from any other: the same
    /// `RunId`, the same worktree, the same slot, and `stamp_and_choose` handing it the candidate
    /// `plan_failover` just advanced to.
    ///
    /// The old child is already reaped — this runs from its exit handler — so there is nothing to
    /// wind down and no kill to order against. The rebind still matters for `bind_session`'s own
    /// reason: it is what makes any late frame on the dead session belong to nobody.
    async fn fork_failover(self: &Arc<Self>, app: AppHandle, failover: Failover) {
        // The session the successor is filed under: the one a restart already bound under the
        // lock that decided it (`plan_restarts`), or a fresh one bound here for a provider
        // failover. Either way the binding precedes the fork, for `bind_session`'s own reason.
        let session = match failover.session {
            Some(session) => session,
            None => {
                let session = SessionId::new();
                self.bind_session(failover.run, session);
                session
            }
        };
        let admission = Admission {
            run: failover.run,
            project: failover.project,
            agent: failover.agent,
            task: failover.task,
            task_title: failover.task_title,
            change: failover.change,
            // The **same** prompt. A failover re-asks the question the provider refused to answer;
            // it is not a follow-up, which is also why `turns` does not move.
            prompt: failover.prompt,
            resume: failover.resume.map(|conversation| ResumePoint {
                // `None`: the successor is a **new** cide session in every case that reaches
                // here. opencode's and codex's conversations travel in their own flags, and a
                // claude continuation under a fresh id is a fork (`respawn_spec` on that
                // harness), which is exactly what lets the dying child's exit belong to nobody.
                rebind: None,
                conversation,
            }),
            purpose: failover.purpose,
        };
        let resume = admission
            .resume
            .as_ref()
            .map(|point| point.conversation.clone());
        // The screen the old child last showed, read from the session it was filed under —
        // which `report_exit` deliberately leaves in the registry, so a pane that was watching
        // this run can still paint what it printed. Carried into the successor's mirror so the
        // transcript survives the swap rather than starting blank at the second model.
        //
        // A pane *already open* is attached to the old `SessionId` and does not follow; that is
        // unchanged from a respawn today, and this is what the next Open shows.
        let preload = app
            .try_state::<SessionRegistry>()
            .and_then(|sessions| {
                failover
                    .previous
                    .and_then(|previous| sessions.get(previous))
            })
            .map(|pty| (pty.full_state(), failover.why));
        self.bring_up(app, admission, session, resume, preload)
            .await;
    }

    /// Should this exit spend the next pool candidate instead of ending the run? (M45)
    ///
    /// **The decision half, under the lock, and therefore the half a test can drive** —
    /// `plan_respawn`'s split, copied deliberately: everything that decides is here and everything
    /// that forks is in the caller, which needs an `AppHandle` this build cannot make.
    ///
    /// Called on the reaper thread **before the exit is observed**, and that ordering is the whole
    /// change. `observe(Exit)` answers `Finished`, and `set_state` releases the slot and the
    /// checkout on it — after which the queued sibling of this role is admitted and its `bring_up`
    /// winds down whatever it finds in the checkout. A failover decided *after* that would be
    /// re-forking into a directory the queue has already given away: run 06202dd6's failure with
    /// the sequence reversed.
    ///
    /// `None` is the ordinary answer and leaves the exit to end the run exactly as it always has.
    ///
    /// Production calls [`Self::plan_failover_or_wait`]; this is its face for the tests that
    /// only ask whether a fork was planned.
    #[cfg(test)]
    fn plan_failover(&self, session: SessionId, code: i32) -> Option<Failover> {
        match self.plan_failover_or_wait(session, code)? {
            FailoverPlan::Fork(failover) => Some(*failover),
            FailoverPlan::Queued(_) => None,
        }
    }

    /// [`Self::plan_failover`], with the third answer running limits added: the next candidates
    /// exist but are all at their `max_running`, so the run goes back to the **front** of its
    /// role's queue — slot released, session unbound, the candidate position advanced — and
    /// admission forks it onto the first of them that frees. Forking over the limit instead
    /// would be spending the very concurrency the user capped.
    fn plan_failover_or_wait(&self, session: SessionId, code: i32) -> Option<FailoverPlan> {
        let mut inner = self.inner.lock();
        let at = inner
            .runs
            .values()
            .find(|live| live.session == Some(session))?
            .run;
        let live = inner.runs.get_mut(&at).expect("found above");

        // **Taken, never read.** A candidate's verdict belongs to the child that reported it; a
        // latch left behind would fail the *next* candidate over on its own clean exit.
        let reason = live.provider_failure.take()?;

        // Four refusals before the pool is consulted, each about a death that is not a provider's
        // fault:
        //
        // * cide killed this child on purpose — the user pressed Stop, and a failover would
        //   restart the run underneath them;
        // * the process is going down. `snapshot_sealed` is set by `final_snapshot`, which
        //   `lifecycle::shutdown` calls *before* the ladder, so every child the ladder kills lands
        //   here already sealed and none of them forks a successor into a teardown;
        // * the run is not working. Only `Starting` or `Running` has a turn to lose; a `Paused`
        //   one's child died under a freeze, and the terminal states hold no turn at all;
        // * the child exited **cleanly**. Measured on 1.18.29: both probed provider failures exit
        //   1. A zero exit after an error line is a turn opencode retried internally and finished,
        //   and failing it over would spend a candidate on a turn that succeeded. The code is a
        //   guard and never the trigger — a non-zero exit with nothing classified still ends the
        //   run as it always did.
        //   mimo breaks that contract and says so through `failure_exits_zero`: its error line is
        //   terminal and its exit is 0 either way, so for it the latch alone decides. (M81)
        let clean_is_final =
            cide_agents::for_kind(live.harness).is_some_and(|harness| harness.failure_exits_zero());
        if live.stopping
            || self.snapshot_sealed.load(Ordering::SeqCst)
            || !matches!(live.state, RunState::Starting | RunState::Running)
            || (code == 0 && !clean_is_final)
        {
            return None;
        }

        let spent = live.pool.get(live.pool_index).cloned();
        let (pool, from) = (live.pool.clone(), live.pool_index + 1);
        let (agent_label, pool_name, project) = (
            live.agent_label.clone(),
            live.pool_name.clone(),
            live.project,
        );

        // Bench what refused, for every run and not only this one (M90) — **before** choosing
        // where this run goes next, so a target named twice in one list is not chosen again a
        // line later. Benched even on exhaustion: the next run to dispatch should not have to
        // spend a turn learning it too.
        let now = now_unix_ms();
        if let Some(entry) = spent.as_ref() {
            let until = bench_entry(&mut inner, entry, reason, at, &agent_label, now);
            log_pool_event(
                &mut inner,
                cide_ipc::PoolEvent {
                    at_unix_ms: now,
                    pool: pool_name.clone(),
                    run: Some(at),
                    agent_label: Some(agent_label.clone()),
                    project: Some(project),
                    kind: cide_ipc::PoolEventKind::Refused {
                        entry: entry.clone(),
                        reason: reason.wire(),
                        until_unix_ms: until,
                    },
                },
            );
        }
        // Where this run could go next: the count needs every other run, and leaves this one out
        // because it is leaving its current entry.
        let choice = choose_entry(&inner, &pool, from, Some(at), now);
        let wait_note = choice
            .is_none()
            .then(|| pool_full_note(&inner, &pool, from));
        let passed_over = choice
            .as_ref()
            .filter(|choice| !choice.skipped.is_empty())
            .map(|choice| format!("; passed over {}", skipped_phrase(&choice.skipped, now)))
            .unwrap_or_default();
        let room = choice.as_ref().map(|choice| choice.at);
        let live = inner.runs.get_mut(&at).expect("found above");

        let next = room.unwrap_or(from);
        let Some(candidate) = live.pool.get(next).cloned() else {
            // Exhausted. The run ends as any run ends — the exit is real and so is its code — and
            // the row says what happened to it, which is the whole of what exhaustion owes a user.
            // Deliberately **not** `RunState::Failed`: that variant is for a run that never
            // reached a child, and `death_facts` branches on exactly that distinction to decide
            // whether the comment it writes can print a number.
            if let Some(entry) = spent {
                live.spent.push((entry, reason));
            }
            if !live.spent.is_empty() {
                let each = live
                    .spent
                    .iter()
                    .map(|(entry, why)| format!("{} {}", entry.model_flag(), why.phrase()))
                    .collect::<Vec<_>>()
                    .join(", ");
                live.pool_note = Some(format!("every model in the pool failed: {each}"));
            }
            return None;
        };

        if let Some(entry) = spent.clone() {
            live.spent.push((entry, reason));
        }
        // The fresh-or-continue decision, `Failover::resume`'s below — made here too because the
        // wait needs it as much as the fork.
        let fresh = live.turns <= 1;
        if room.is_none() {
            // Every remaining candidate is at its running limit. Wait for one rather than fork
            // over it; `admit_a_pass` searches from `next` on, so the order is kept and nothing
            // already refused is retried.
            live.pool_index = next;
            let resume = match fresh {
                true => {
                    live.harness_session = None;
                    None
                }
                false => live
                    .harness_session
                    .clone()
                    .map(|conversation| ResumePoint {
                        // A pool is opencode-shaped, whose child is a new cide session either way.
                        rebind: None,
                        conversation,
                    }),
            };
            live.pool_note = Some(format!(
                "{} {} — waiting for room on {} (entry {} of {})",
                spent.as_ref().map_or_else(String::new, PoolEntryExt::flag),
                reason.phrase(),
                candidate.model_flag(),
                next + 1,
                live.pool.len()
            ));
            // `requeue_unsubmitted`'s shape: unbind first, so the exit that follows belongs to
            // nobody and cannot end the run it has just been taken from.
            live.session = None;
            move_to(live, RunState::Queued, now_unix_ms());
            live.requeued = Some(Requeued {
                resume,
                prompt: live.prompt.clone(),
            });
            live.note = wait_note;
            let held = std::mem::replace(&mut live.slot, false);
            let (key, project) = (live.key(), live.project);
            if held {
                release(&mut inner, &key, project);
            }
            inner.queues.entry(key).or_default().push_front(at);
            return Some(FailoverPlan::Queued(project));
        }
        // **Monotonic**, and that is both the stickiness and the loop guard: a run fails over at
        // most `pool.len() - 1` times for its entire life.
        live.pool_index = next;
        debug_assert!(live.pool_index < live.pool.len());
        live.pool_note = Some(format!(
            "{} {} — now on {} (entry {} of {}){passed_over}",
            spent.as_ref().map_or_else(String::new, PoolEntryExt::flag),
            reason.phrase(),
            candidate.model_flag(),
            next + 1,
            live.pool.len()
        ));
        // `Starting` rather than a new variant: it already means *spawning, and the child has not
        // reported in*, which is exactly true from here until the next child prints a line. It is
        // a working phase in the panel and an occupying state in `admit_a_pass`'s checkout gate,
        // so the slot and the worktree stay this run's across the swap — which is the point.
        //
        // Written inline rather than through `set_state`: that method takes this same lock, and
        // `Running → Starting` is neither the handed-back edge nor an `over` transition, so there
        // is nothing of its bookkeeping to run.
        //
        // It does go through `move_to`, which takes no lock and so is reachable from here. The
        // transition is work → work, which leaves the open interval alone — a respawn onto the
        // next candidate is one run continuing, and the figure must not restart with the child.
        move_to(live, RunState::Starting, now_unix_ms());

        // A first-turn failure abandons its conversation, and that is the cheap, common case: the
        // session holds exactly the prompt cide just sent and one failed assistant message, so
        // nothing is lost and the next candidate never sees the error. Later, the conversation
        // carries real work and is worth a duplicated prompt.
        //
        // Clearing `harness_session` is what makes the abandonment real. `note_harness_session`
        // never overwrites — a respawn prints the same id, so it latches once — and left set, this
        // run would go on naming a conversation it walked away from: Open would show the failure,
        // and a later user retry would `--session` back into it.
        //
        // This clear is a decision taken **with the lock held, before the next child exists**,
        // which is categorically different from the line-driven overwrite that method refuses. Do
        // not "restore" the invariant here.
        let resume = match fresh {
            true => {
                live.harness_session = None;
                None
            }
            false => live.harness_session.clone(),
        };

        let failover = Failover {
            // **The same `RunId`**, which is what makes this a continuation of one run rather than
            // a second run of the same work. `plan_respawn`'s four rules apply unchanged.
            run: live.run,
            project: live.project,
            agent: live.agent.clone(),
            task: live.task.clone(),
            task_title: live.task_title.clone(),
            change: live.change.clone(),
            prompt: live.prompt.clone(),
            resume,
            session: None,
            previous: live.session,
            why: Carried::Failover,
            purpose: live.purpose.clone(),
        };
        // A failover's fork never passes through admission, so its start is logged here — or
        // the card's history would show a refusal and then nothing about where the run went.
        if let Some(choice) = choice {
            log_pool_event(
                &mut inner,
                cide_ipc::PoolEvent {
                    at_unix_ms: now,
                    pool: pool_name,
                    run: Some(at),
                    agent_label: Some(agent_label),
                    project: Some(project),
                    kind: cide_ipc::PoolEventKind::Started {
                        entry: candidate,
                        index: u16::try_from(next).unwrap_or(u16::MAX),
                        of: u16::try_from(pool.len()).unwrap_or(u16::MAX),
                        skipped: choice.skipped,
                        benched_fallback: choice.benched_fallback,
                    },
                },
            );
        }
        Some(FailoverPlan::Fork(Box::new(failover)))
    }

    /// Stamp this run's pool if it has none yet, and answer the candidate it is on. (M45)
    ///
    /// Called from every fork — the first, a respawn, and a failover's re-fork — and it is one
    /// function rather than three so the three cannot disagree about which candidate is current.
    ///
    /// The stamp happens **once**, on the first fork, for `LiveRun::pool`'s stated reason: a pool
    /// edited under a live run would move the candidate a failover advances to. Every later call
    /// reads what is already there, so `pool_index` — which only a failover moves, and only
    /// upward — is what decides.
    ///
    /// Every fork also records here what it was forked *with* (`LiveRun::forked_with`), because
    /// this is the one function every fork passes through and the comparison a Resume makes is
    /// only honest against the settings the standing child actually got.
    fn stamp_and_choose(
        &self,
        run: RunId,
        resolved: &cide_agents::overrides::Resolved,
        settings: cide_agents::overrides::ChildSettings,
    ) -> Option<cide_ipc::PoolChoice> {
        let mut inner = self.inner.lock();
        let live = inner.runs.get_mut(&run)?;
        live.last_model.clone_from(&settings.model);
        live.forked_with = Some(settings);
        if live.pool.is_empty() && live.pool_index == 0 {
            live.pool.clone_from(&resolved.pool);
        }
        // The pool's name and position are fields on the wire row now (`LiveRun::using`), not a
        // sentence in `pool_note` — which is kept for the events a pool has (a failover, an
        // exhaustion) rather than for the standing fact of which candidate a run is on. (M89)
        if live.pool_name.is_none() && !live.pool.is_empty() {
            live.pool_name.clone_from(&resolved.pool_name);
        }
        let entry = live.pool.get(live.pool_index)?.clone();
        Some(cide_ipc::PoolChoice {
            pool: resolved.pool_name.clone().unwrap_or_default(),
            // `u16` on the wire; a pool long enough to overflow one is not a pool.
            index: u16::try_from(live.pool_index).unwrap_or(u16::MAX),
            entry,
        })
    }

    /// Latch the provider failure this child last reported. (M45)
    ///
    /// Last writer wins: a turn that met two refusals is a turn whose *last* one its exit is
    /// about. Moves nothing — [`cide_agents::Harness::observe`] owns every state transition, and
    /// the line this is derived from is one that harness deliberately answers `None` to.
    /// Write down what this run's last completed step spent. (M80)
    ///
    /// Last writer wins, which is the whole of it: [`LiveRun::usage`] is a *position*, not a
    /// running total, for the reason that field states at length.
    ///
    /// Moves nothing, exactly as [`Self::note_provider_failure`] moves nothing — a token count
    /// is not a state transition, and a step ending is emphatically not a turn ending (opencode
    /// emits `step_finish` from nested subagents; run 06202dd6 is what reading one as the end of
    /// a turn cost). No event is emitted either: the figure is read on demand by a card somebody
    /// opened, and broadcasting a roster to every window per step would be a redraw per model
    /// call for a number nothing on screen is showing.
    fn note_usage(&self, run: RunId, usage: cide_ipc::TokenUsage) {
        if let Some(live) = self.inner.lock().runs.get_mut(&run) {
            live.usage = Some(usage);
        }
    }

    /// Which harness, which model, and how much context — for the card a `#7` opens. (M80)
    ///
    /// Keyed by session because that is all the card has: `session_log_detail` is handed the
    /// pane's session and a ring handle, and a pane knows nothing about runs. `None` for a
    /// session no run stands on, which is every shell pane in the application and is the
    /// ordinary answer.
    ///
    /// # Read off the run, never off the role's file
    ///
    /// [`LiveRun::harness`] is the harness the child was **forked with** — `note_harness` keeps
    /// it true against a local override — and the model is resolved from the run's own pool
    /// position and the settings it was forked with. Reading `agent.def` here would reproduce
    /// M78's bug in the one place built to explain a run after the fact: a card naming the
    /// committed file's harness beside a conversation held by another one.
    ///
    /// A **finished** run still answers, deliberately. Its row has left the live list, its
    /// child is gone, and its pane is exactly where somebody reads it back — which is the whole
    /// occasion this card exists for.
    pub fn log_run_info(&self, session: SessionId) -> Option<cide_ipc::LogRunInfo> {
        let inner = self.inner.lock();
        let live = inner
            .runs
            .values()
            .find(|run| run.session == Some(session))?;
        let candidate = live.pool.get(live.pool_index);
        // `LiveRun::using` is the one ladder — the row on the panel and this card must never
        // name two different models for one run. The card keeps its bare `1 of 3`; the pool's
        // name is the row's addition.
        let (model, _) = live.using();
        Some(cide_ipc::LogRunInfo {
            harness: live.harness,
            model,
            // `1 of 3`, and only where there is a pool. Without it the card would name a model
            // nobody chose and say nothing about the two that refused before it.
            pool_position: candidate
                .map(|_| format!("entry {} of {}", live.pool_index + 1, live.pool.len())),
            usage: live.usage,
            context_limit: candidate.and_then(|entry| {
                live.forked_with
                    .as_ref()
                    .and_then(|settings| settings.providers.as_ref())
                    .and_then(|providers| context_limit(providers, entry))
            }),
        })
    }

    fn note_provider_failure(&self, run: RunId, reason: cide_agents::FailoverReason) {
        if let Some(live) = self.inner.lock().runs.get_mut(&run) {
            tracing::warn!(%run, ?reason, "this run's provider refused");
            live.provider_failure = Some(reason);
        }
    }

    /// `SIGCONT` every session this registry froze, so the shutdown ladder's rungs can land.
    ///
    /// **Called from `lifecycle::shutdown` before the ladder, and it is not optional.** A
    /// `SIGSTOP`ped process does not *act* on SIGHUP or SIGTERM — they go pending — so the ladder
    /// would spend its whole `hup_grace + term_grace` on a child that cannot answer and then
    /// SIGKILL it, which is exactly the half-written transcript `lifecycle.rs`'s header says the
    /// ladder exists to prevent. One signal per paused session buys back both rungs.
    ///
    /// The *freeze* still does not survive a restart — the ladder ends the process either way,
    /// and a stopped-then-killed child is just a killed child. What survives since the snapshot
    /// landed is the *arrangement*: `lifecycle` calls [`Self::save_snapshot`] before this, so
    /// the paused queues come back paused and the runs come back as
    /// [`RunState::Interrupted`] rows a Resume can continue. (This paragraph used to say
    /// "nothing about a freeze is written to disk", and it was the recorded reason pause did
    /// not survive a restart; the snapshot is what changed.)
    ///
    /// Takes the [`SessionRegistry`] rather than an `AppHandle` so it is callable from a test,
    /// and clears the records as it goes: a second call during a teardown that runs twice must
    /// not signal sessions the first call already thawed.
    pub fn thaw_for_shutdown(&self, sessions: &SessionRegistry) {
        let frozen: Vec<SessionId> = {
            let mut inner = self.inner.lock();
            inner.frozen_sessions.drain().map(|(id, _)| id).collect()
        };
        if frozen.is_empty() {
            return;
        }
        tracing::info!(
            count = frozen.len(),
            "thawing paused sessions so the shutdown ladder can reach them"
        );
        for session in frozen {
            if let Some(pty) = sessions.get(session) {
                signal_pty(&pty, PauseSignal::Cont);
            }
        }
    }
}

/// The project's primary console session — the orchestrator. See [`AgentRegistry::pause`].
///
/// `None` when the project has no console tab, no primary pane, or a primary pane with no
/// session yet (a pane sitting at a resume splash). All three are ordinary, and all three mean
/// the same thing to a pause: there is nothing to freeze.
fn primary_session(app: &AppHandle, project: ProjectId) -> Option<SessionId> {
    let state = app.try_state::<crate::workspace_state::WorkspaceState>()?;
    state.with(|ws| {
        let id = cide_core::workspace::console_tab(ws, project).ok()?;
        let tab = cide_core::workspace::tab(ws, project, id).ok()?;
        tab.tree
            .panes
            .values()
            .find(|pane| pane.role == PaneRole::Primary)
            .and_then(|pane| pane.session)
    })
}

/// Signal one session's child, by id.
fn signal_session(app: &AppHandle, session: SessionId, signal: PauseSignal) {
    let Some(pty) = app
        .try_state::<SessionRegistry>()
        .and_then(|sessions| sessions.get(session))
    else {
        return;
    };
    signal_pty(&pty, signal);
}

/// The child's **process group**, through the one implementation of "signal a child".
///
/// The group is the whole point: `SIGSTOP` to the leader alone leaves the agent's `bash` tool
/// invocation and its stdio MCP servers running while the model process is frozen — half a pause,
/// and the half that is still spending.
#[cfg(unix)]
fn signal_pty(pty: &PtySession, signal: PauseSignal) {
    let Some(pid) = pty.child_pid() else {
        return;
    };
    cide_core::child_env::signal_group(
        pid,
        match signal {
            PauseSignal::Stop => libc::SIGSTOP,
            PauseSignal::Cont => libc::SIGCONT,
        },
    );
}

/// No-op off unix, which makes the whole pause one off unix — `cide_core::child_env::signal_group`
/// is a no-op there for the same reason, and the signal *numbers* are why this arm exists as well
/// as that one: `libc::SIGSTOP` does not exist to be matched on when `libc` is not linked.
#[cfg(not(unix))]
fn signal_pty(_pty: &PtySession, _signal: PauseSignal) {}

/// Everything the fork of a failover's successor needs. (M45)
///
/// A sibling of `Continuation` rather than the same type, and the difference is the one that
/// matters: a *user* retry through `plan_respawn` refuses a run that never reported a conversation
/// id, because there is nothing to continue and re-running would re-do work from an empty context.
/// A failover on that same run is the ordinary case — there is nothing to continue precisely
/// because the provider refused before a word was said — so `resume` is an `Option` here and its
/// `None` is a plan rather than a refusal.
#[derive(Debug, Clone)]
struct Failover {
    run: RunId,
    project: ProjectId,
    agent: AgentId,
    task: Option<TaskId>,
    task_title: Option<String>,
    change: Option<String>,
    /// The prompt to send. For a provider failover the **same** one the refused turn carried;
    /// for a restart on new settings, the continuation line — or the dispatch, replayed, when
    /// the frozen child had not begun its turn.
    prompt: String,
    /// The conversation to continue, or `None` to start a fresh one. See `plan_failover`.
    resume: Option<String>,
    /// The session the successor is already bound to, or `None` to mint and bind one at the
    /// fork. A restart binds under the lock that decides it — before the old child is signalled,
    /// `respawn`'s rule — so by the time this is acted on the binding is done.
    session: Option<SessionId>,
    /// The session the dying child was filed under: the screen carried into the successor's
    /// mirror is read from it. `None` for a run that never reached a child.
    previous: Option<SessionId>,
    /// Which sentence separates the carried screen from the successor's output.
    why: Carried,
    /// See [`RunPurpose`]: a review's successor is still a review.
    purpose: RunPurpose,
}

/// What a provider failure with pool candidates left comes to. See
/// [`AgentRegistry::plan_failover_or_wait`].
#[derive(Debug)]
enum FailoverPlan {
    /// Fork the next candidate now, in the slot this run already holds. Boxed because the
    /// other arm is one id, and this is the rare path.
    Fork(Box<Failover>),
    /// Every remaining candidate is at its running limit: the run is back at the front of its
    /// queue, its slot released. The project is whose queue to pump and whose rows to redraw.
    Queued(ProjectId),
}

/// Why a run's successor is carrying its predecessor's screen — what the separator between
/// the two says. A person reading the pane needs to know which of these happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carried {
    /// The provider refused and the pool's next candidate continues.
    Failover,
    /// A person changed the settings under a pause and resumed.
    Settings,
}

impl Carried {
    fn separator(self) -> &'static [u8] {
        match self {
            Self::Failover => FAILOVER_SEPARATOR,
            Self::Settings => SETTINGS_SEPARATOR,
        }
    }
}

// Nothing else is carried, deliberately: this is the *same* run, so its label, harness, limits,
// checkout and notify address are already on its `LiveRun` and `bring_up` reads them from there.
// A copy here would be a second source for facts that must not be able to differ.

/// `PoolEntry::model_flag` under a name a `map_or_else` can point at.
trait PoolEntryExt {
    fn flag(&self) -> String;
}

impl PoolEntryExt for cide_ipc::PoolEntry {
    fn flag(&self) -> String {
        self.model_flag()
    }
}

/// Give an agent and its project one slot back. Free rather than a method so [`AgentRegistry::
/// set_state`] can call it with the lock it already holds.
fn release(inner: &mut Inner, key: &AgentKey, project: ProjectId) {
    if let Some(held) = inner.agent_slots.get_mut(key) {
        *held = held.saturating_sub(1);
    }
    if let Some(held) = inner.project_slots.get_mut(&project) {
        *held = held.saturating_sub(1);
    }
}

// ==========================================================================================
// The snapshot: runs that outlive the process.
// ==========================================================================================

/// The disk shape of [`AgentRegistry::save_snapshot`]. Machine-local state, not project state:
/// worktree paths, session ids and prompts belong to this machine's checkout, so the file lives
/// in `$XDG_STATE_HOME` beside `workspace.json` — never in `.cide/`, which is committed.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunsFile {
    version: u32,
    /// Projects whose dispatch queue was shut when the process ended, so a halted workstream
    /// comes back halted rather than silently reopened.
    paused: Vec<ProjectId>,
    runs: Vec<SavedRun>,
}

/// One run's durable facts — everything a `Resume` after restart needs and nothing it does not.
/// The state is deliberately absent: whatever a run was doing, its child is gone, and every
/// restored run is [`RunState::Interrupted`].
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedRun {
    run: RunId,
    project: ProjectId,
    agent: AgentId,
    agent_label: String,
    harness: Harness,
    /// The claude conversation id — the shutdown ladder's graces exist so the child could
    /// finish writing the transcript this resumes from.
    session: Option<SessionId>,
    /// The opencode conversation id, whose store is that CLI's own.
    harness_session: Option<String>,
    task: Option<TaskId>,
    task_title: Option<String>,
    prompt: String,
    started_unix_ms: u64,
    /// The run's worked total, with any interval that was open at shutdown already closed —
    /// see [`worked_now`], which the write calls rather than copying the field raw. Copying it
    /// raw would lose the whole of the session a run was in the middle of when cide quit.
    ///
    /// `#[serde(default)]` is the schema rung: a snapshot written before this field existed
    /// reads `0`, which understates a pre-existing run's figure exactly once and is the only
    /// honest answer available — nothing on disk records what those runs did.
    ///
    /// There is deliberately **no** saved counterpart to `LiveRun::working_since_unix_ms`.
    /// Every restored run comes back [`RunState::Interrupted`], which is not work, so the
    /// stamp is `None` by construction and the hours cide spent shut add nothing to the
    /// figure. The offline case needs no field and no code.
    #[serde(default)]
    worked_ms: u64,
    /// Recorded limits, kept for [`admit_a_pass`]'s stated reason: admission must not read
    /// disk under the lock, and a config edited across the restart must not release a slot
    /// that was never taken.
    agent_limit: u16,
    project_limit: u16,
    /// The pool this run settled on, and how far down it. (M45)
    ///
    /// Persisted so a run interrupted by a restart comes back on the candidate it had reached
    /// rather than at the top of its list — an ordered pool whose position is forgotten across a
    /// restart is a pool that re-pays its first, failing candidate on every launch.
    ///
    /// `#[serde(default)]` on both: a snapshot written before this field existed reads as "no
    /// pool", which is exactly what such a run had.
    #[serde(default)]
    pool: Vec<cide_ipc::PoolEntry>,
    #[serde(default)]
    pool_index: usize,
    /// [`LiveRun::pool_name`] and [`LiveRun::last_model`], so a restored history row still says
    /// what it ran on. (M89) Absent in an older file, which reads as "not recorded" and draws the
    /// harness alone.
    #[serde(default)]
    pool_name: Option<String>,
    #[serde(default)]
    last_model: Option<String>,
    /// The stamped worktree name, kept for the checkout gate's stated reason — the same as the
    /// limits'. `None` reads as "shared isolation" for a file from before this field; the cost
    /// is that such a resumed run skips the gate once, which degrades to the pre-feature
    /// status quo of sharing a checkout, not to anything worse.
    #[serde(default)]
    checkout: Option<String>,
    /// Where the run's turn endings are announced. (M40) `Primary` for a file from before this
    /// field, which is what every run then had.
    #[serde(default)]
    notify: RunNotify,
    /// The state at quit, kept **only** for its terminal arms: a `Finished` or `Failed` run
    /// restores verbatim, because history is history — the user asked for the panel's tail to
    /// be a durable record of runs, not a per-process scratchpad. Every other state restores
    /// as [`RunState::Interrupted`], whatever it was: the child is gone either way, and the
    /// old state would be a claim about a process that no longer exists. `None` (a file from
    /// before this field) reads as non-terminal.
    #[serde(default)]
    state: Option<RunState>,
}

const RUNS_SNAPSHOT_VERSION: u32 = 1;

fn snapshot_path() -> std::path::PathBuf {
    cide_core::persist::state_dir().join("agent-runs.json")
}

/// Where [`AgentRegistry::save_screens_for_snapshot`] keeps each run's last screen.
fn screens_dir() -> std::path::PathBuf {
    cide_core::persist::state_dir().join("run-screens")
}

/// The saved screen a resumed run's mirror is seeded from, if the last teardown wrote one.
fn saved_screen(run: RunId) -> Option<Vec<u8>> {
    std::fs::read(screens_dir().join(format!("{run}.screen"))).ok()
}

/// How many of a run log's trailing lines a resumed run's pane is seeded with.
///
/// [`crate::logring`]'s own `CAP`, deliberately and not a number of its own: every replayed
/// line the harness says a person may click is recorded into the new session's ring **as it is
/// drawn**, so a budget larger than the ring's would draw `#7` handles the ring had already
/// evicted before the replay finished — a row on screen whose click answers *gone*, which is
/// the one thing the handle exists not to do.
const REPLAY_LINES: usize = 2_000;

/// What a resumed run's mirror is seeded with: the run's own earlier output, re-rendered.
///
/// # Why the saved screen was not enough
///
/// A resumed `opencode` or `codex` run is a **new cide session** — `resume_point` answers
/// `rebind: None` for those two, because their conversation id is the CLI's own and cide's is
/// free to move. So the mirror, the sinks and the [`crate::logring`] ring the old child filled
/// are all gone, and the only bridge was `run-screens/<run>.screen`, which has two holes: it is
/// written at teardown only (`save_screens_for_snapshot` says so — a crash keeps the runs and
/// loses the screens), and on this machine every file it last wrote was ten bytes, the
/// `full_state` lead-in over a mirror with nothing in it. Either way the pane came back holding
/// one separator and the next turn, which is what "it started from zero" looked like from the
/// outside while the conversation itself was entirely intact.
///
/// The log has neither hole. It is teed line by line in [`AgentRegistry::stream_hook`], keyed
/// by **run** rather than by session, so it already spans every child the run has had *and*
/// every restart between them — measured on run `e43e6c3b`: 431 events from 13:13 to 13:34,
/// straight through the 13:25 restart that ended the process which wrote the first half.
///
/// # The rows must be the rows the live stream would have drawn
///
/// So this replays through the *same* two function pointers the live hook installs —
/// [`SessionBinding::Harness`]'s `keep` and `render`, handed in by `start_child` from the very
/// binding it is about to give the session — and records each kept line into the new session's
/// ring in the same pass, which is what makes a replayed row's handle resolve through
/// `session_log_detail` like a live one's. Two passes, or a second rendering written here,
/// would be a second spelling of the row format that `check:json-log` pins in one place.
///
/// `None` for anything that would draw nothing: no log (a `claude` or `qwen` run is not teed —
/// see [`run_logs_dir`]), an unreadable one, an empty one, or a log whose every line the
/// rendering drops. The caller falls back to [`saved_screen`] and then to no preload at all;
/// nothing here may fail a fork.
fn replayed_run_log(
    app: &AppHandle,
    run: RunId,
    session: SessionId,
    keep: fn(&str) -> bool,
    render: fn(&mut RenderState, &str, Option<u64>) -> cide_pty::Rendered,
) -> Option<Vec<u8>> {
    let ring = app.try_state::<Arc<crate::logring::JsonLogRing>>();
    replayed_log_at(
        &run_logs_dir().join(format!("{run}.log")),
        keep,
        render,
        |line, stamp| {
            ring.as_ref()
                .map(|ring| ring.record_at(session, line, stamp))
        },
    )
}

/// The read, path-parameterised and with the ring injected, for the same reason
/// [`AgentRegistry::write_snapshot_to`] takes a path: so a test never touches the real state
/// directory, and so the budget and the skipped-events rule are assertable without a run.
fn replayed_log_at(
    path: &std::path::Path,
    keep: fn(&str) -> bool,
    render: fn(&mut RenderState, &str, Option<u64>) -> cide_pty::Rendered,
    record: impl FnMut(&str, u64) -> Option<u64>,
) -> Option<Vec<u8>> {
    let text = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    // When a line carries no clock of its own — codex records none on any item — the moment the
    // log was last written stands in. It is a bound rather than a measurement, and it is the
    // honest half of a bad pair: the alternative is `now`, which would make every replayed
    // card claim its command ran at the instant somebody pressed Resume.
    let last_written = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or_else(crate::logring::now_unix_ms, |since| {
            since.as_millis() as u64
        });

    let skipped = lines.len().saturating_sub(REPLAY_LINES);
    let rows = replay_rows(&lines[skipped..], keep, render, last_written, record);
    if rows.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    if skipped > 0 {
        // Said rather than silently dropped, and it names the file: the budget is a mirror and
        // a ring decision, not a claim that the earlier events are gone.
        out.extend_from_slice(
            format!(
                "\x1b[2m— {skipped} earlier event(s) not replayed; the whole log is {} —\x1b[0m\r\n",
                path.display()
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(&rows);
    Some(out)
}

/// The replay itself, pure over its inputs so the table below can drive it with no repository,
/// no app handle and no ring.
///
/// It is `cide_pty`'s own `render_lines` loop with the plumbing removed: the log holds one event
/// per line with its terminator already stripped by `writeln!`, so there is no partial-line
/// hazard to carry and nothing to resync. Every emitted row is terminated `\r\n` — including a
/// [`cide_pty::Rendered::Keep`], which the live path re-emits byte for byte instead. The
/// difference is deliberate and safe here for the same reason the live rule exists: `Keep`'s
/// warning is about a raw-mode TUI whose own `\n`-only lines must not be walked back to column
/// 0, and a `SessionBinding::Harness` stream is machine events and the CLI's own prose, never
/// a TUI — the binding is what *makes* it a line stream.
///
/// `record` answers the ring handle for a line `keep` accepted, or `None` where there is no
/// ring; `fallback_stamp` is used for a line that carries no clock of its own.
fn replay_rows(
    lines: &[&str],
    keep: fn(&str) -> bool,
    render: fn(&mut RenderState, &str, Option<u64>) -> cide_pty::Rendered,
    fallback_stamp: u64,
    mut record: impl FnMut(&str, u64) -> Option<u64>,
) -> Vec<u8> {
    let mut state = RenderState::default();
    let mut out = Vec::new();
    for line in lines {
        let stamp = line_stamp(line).unwrap_or(fallback_stamp);
        let handle = keep(line).then(|| record(line, stamp)).flatten();
        state.now_unix_ms = Some(stamp);
        match render(&mut state, line, handle) {
            cide_pty::Rendered::Drop => {}
            cide_pty::Rendered::Keep => {
                out.extend_from_slice(line.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            cide_pty::Rendered::Replace(text) => {
                out.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
                out.extend_from_slice(b"\r\n");
            }
        }
    }
    // A log that ended mid-step ends with the live marker drawn and nothing coming to erase it
    // — the next thing written here is cide's own separator, which would then sit under a row
    // claiming the run is working. See `cide_agents::ERASE_MARKER`.
    if state.marker {
        out.extend_from_slice(cide_agents::ERASE_MARKER.as_bytes());
    }
    out
}

/// The moment one logged event says it happened, where its harness records one.
///
/// opencode stamps every event line (`"timestamp"`, milliseconds); codex records no clock on
/// any item, which is the `None` [`RenderState::now_unix_ms`] is an `Option` for. A line that
/// is not JSON at all — the CLI's own prose, or [`RUN_LOG_CAPPED`] — answers `None` too.
fn line_stamp(line: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()?
        .get("timestamp")?
        .as_u64()
}

/// Where each harness-bound run's raw output is teed, line by line, for a post-mortem.
///
/// The debug report's first ask, in its own words: *"e83a75fe ran 9 minutes and left literally
/// zero evidence of what it was doing"* — no log, no captured tail, nothing to read after a
/// death. This is that trail. Only the harness-bound stream is teed, deliberately: a `claude`
/// run's durable transcript already exists under `~/.claude/projects` (the shutdown ladder's
/// graces exist so it finishes writing), so a second copy here would be two records of one
/// conversation.
pub(crate) fn run_logs_dir() -> std::path::PathBuf {
    cide_core::persist::state_dir().join("run-logs")
}

/// How much one run may log before the tee stops. A cap, not a rotation: the log answers "what
/// was this run doing when it died", which the first five megabytes answer as well as fifty —
/// and an unbounded file per run is a disk the user never agreed to spend.
pub(crate) const RUN_LOG_CAP: u64 = 5 * 1024 * 1024;

/// The one line a capped log ends with, so a reader knows it was cut rather than quiet.
const RUN_LOG_CAPPED: &str = "\u{2014} log capped; the rest of this run is not recorded \u{2014}";

/// One run's append-only log. Held behind a `Mutex` in the stream hook's closure.
///
/// Every failure path latches `dead` and never retries: the hook runs on the coalescer thread
/// — the thread every byte of every session flows through — and a tee that blocks, errors
/// loudly, or retries per line would put disk trouble in front of every terminal in the
/// process. Losing the log is the cheap half; one warn says it happened.
pub(crate) struct RunLog {
    path: std::path::PathBuf,
    file: Option<std::fs::File>,
    written: u64,
    cap: u64,
    dead: bool,
}

impl RunLog {
    pub(crate) fn at(path: std::path::PathBuf, cap: u64) -> Self {
        Self {
            path,
            file: None,
            written: 0,
            cap,
            dead: false,
        }
    }

    /// Append one raw line. Opens lazily on the first — a run that prints nothing owns no file.
    pub(crate) fn append(&mut self, line: &str) {
        use std::io::Write as _;
        if self.dead {
            return;
        }
        if self.file.is_none() {
            let opened = self
                .path
                .parent()
                .map(std::fs::create_dir_all)
                .transpose()
                .and_then(|_| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.path)
                });
            match opened {
                Ok(file) => {
                    // Seeded from what is already there: a respawned child appends to its
                    // run's one log, and the cap covers the run's whole life rather than
                    // resetting per child.
                    self.written = file.metadata().map(|m| m.len()).unwrap_or(0);
                    self.file = Some(file);
                }
                Err(error) => {
                    tracing::warn!(path = %self.path.display(), %error, "no run log; this run leaves no post-mortem trail");
                    self.dead = true;
                    return;
                }
            }
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if self.written + line.len() as u64 > self.cap {
            let _ = writeln!(file, "{RUN_LOG_CAPPED}");
            self.dead = true;
            self.file = None;
            return;
        }
        match writeln!(file, "{line}") {
            Ok(()) => self.written += line.len() as u64 + 1,
            Err(error) => {
                tracing::warn!(path = %self.path.display(), %error, "run log write failed; the trail stops here");
                self.dead = true;
                self.file = None;
            }
        }
    }
}

impl AgentRegistry {
    /// Write the durable half of the registry to disk. Called from the coalescer's flush — so
    /// it is at most one small write per emit burst, and a crash loses at worst the last
    /// coalescing window — and once more from `lifecycle`'s teardown, where the paused set's
    /// final state is decided.
    pub fn save_snapshot(&self) {
        self.write_snapshot_to(&snapshot_path());
    }

    /// The teardown's authoritative write: seal first, then record — idempotent, so a teardown
    /// that runs twice cannot write the post-ladder wreckage over its own first answer. The
    /// screens ride along because this is the one moment the children still exist to render.
    pub fn final_snapshot(&self, sessions: &SessionRegistry) {
        if self
            .snapshot_sealed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        self.write_snapshot_now(&snapshot_path());
        self.save_screens_for_snapshot(sessions);
    }

    /// The write, with the path as an argument so a test never touches the real state dir.
    ///
    /// Refuses after the seal — see [`Self::snapshot_sealed`] for the shutdown race this is
    /// the answer to.
    fn write_snapshot_to(&self, path: &std::path::Path) {
        if self
            .snapshot_sealed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        self.write_snapshot_now(path);
    }

    /// The unconditional half, which only the two callers above may reach.
    fn write_snapshot_now(&self, path: &std::path::Path) {
        // Read once, outside the lock and before the walk, so every run in one snapshot closes
        // its open working interval against the *same* instant. A clock read per run would
        // give the last row a few milliseconds the first row did not get, which is harmless
        // and is still two answers to one question.
        let snapshot_now = now_unix_ms();
        let file = {
            let inner = self.inner.lock();
            let mut paused: Vec<ProjectId> = inner.paused_projects.iter().copied().collect();
            paused.sort();
            // Terminal runs ride along — the panel's tail is a history, and a history that
            // ends at every restart is a scratchpad. [`Self::forget_old`] has already capped
            // them at [`RECENT_KEPT`] per project, so this is bounded by construction.
            // Not a review (M85): its checkout is deleted when cide quits, so a restored one
            // could only be resumed into a directory that no longer exists.
            let mut runs: Vec<&LiveRun> = inner
                .runs
                .values()
                .filter(|live| matches!(live.purpose, RunPurpose::Work))
                .collect();
            runs.sort_by_key(|live| live.seq);
            RunsFile {
                version: RUNS_SNAPSHOT_VERSION,
                paused,
                runs: runs
                    .into_iter()
                    .map(|live| SavedRun {
                        run: live.run,
                        project: live.project,
                        agent: live.agent.clone(),
                        agent_label: live.agent_label.clone(),
                        harness: live.harness,
                        // A run whose restart is still pending names the session its
                        // conversation is filed under — the old child's, which is what a
                        // `claude --resume` after this restart needs — not the fresh id its
                        // successor would have taken had the old exit landed in time.
                        session: inner
                            .restarts
                            .values()
                            .find(|pending| pending.run == live.run)
                            .and_then(|pending| pending.previous)
                            .or(live.session),
                        harness_session: live.harness_session.clone(),
                        task: live.task.clone(),
                        task_title: live.task_title.clone(),
                        prompt: live.prompt.clone(),
                        started_unix_ms: live.started_unix_ms,
                        // Closed here, not copied: a run that was `Running` when cide quit has
                        // an interval open, and `live.worked_ms` alone would throw away
                        // everything it did since its last state change.
                        worked_ms: worked_now(live, snapshot_now),
                        agent_limit: live.agent_limit,
                        project_limit: live.project_limit,
                        pool: live.pool.clone(),
                        pool_index: live.pool_index,
                        pool_name: live.pool_name.clone(),
                        last_model: live.last_model.clone(),
                        checkout: live.checkout.clone(),
                        notify: live.notify.clone(),
                        state: Some(live.state.clone()),
                    })
                    .collect(),
            }
        };
        let json = match serde_json::to_vec_pretty(&file) {
            Ok(json) => json,
            Err(error) => {
                tracing::error!(%error, "could not encode the run snapshot");
                return;
            }
        };
        if let Err(error) = cide_core::persist::write_atomic(path, &json) {
            tracing::warn!(%error, "could not write the run snapshot; runs will not survive a restart");
        }
    }

    /// Bring the snapshot's runs back as [`RunState::Interrupted`] rows, and its paused set
    /// back as closed queues. Called once, at startup, after the workspace has loaded.
    ///
    /// `keep` filters by project — a run for a project the restored workspace no longer holds
    /// is a row no panel could ever show. Reads never fail the launch: a missing file is a
    /// first run, an unparseable one is logged and ignored, because
    /// "a broken layout must not become a launch loop" applies one state file over.
    ///
    /// `root_of` and `resume_enabled` serve the one filesystem check restore makes (M42):
    /// whether a `claude` row's transcript is still where its worktree filed it, which is what
    /// decides if the row keeps its session and offers Open. Once per row, at launch, so the
    /// roster broadcasts that follow read nothing off disk.
    pub fn restore_snapshot(
        &self,
        keep: impl Fn(ProjectId) -> bool,
        root_of: impl Fn(ProjectId) -> Option<PathBuf>,
        resume_enabled: bool,
    ) {
        self.restore_snapshot_from(&snapshot_path(), keep, root_of, |harness, cwd, session| {
            crate::lifecycle::resumable_on(harness, cwd, session, resume_enabled)
        });
    }

    /// The read, path-parameterised for the same reason as the write's.
    fn restore_snapshot_from(
        &self,
        path: &std::path::Path,
        keep: impl Fn(ProjectId) -> bool,
        root_of: impl Fn(ProjectId) -> Option<PathBuf>,
        transcript: impl Fn(Harness, &std::path::Path, SessionId) -> bool,
    ) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not read the run snapshot");
                return;
            }
        };
        let file: RunsFile = match serde_json::from_slice(&bytes) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "the run snapshot did not parse; starting without it");
                return;
            }
        };

        let mut inner = self.inner.lock();
        for project in file.paused {
            if keep(project) {
                inner.paused_projects.insert(project);
            }
        }
        let mut restored: std::collections::HashSet<String> = std::collections::HashSet::new();
        for saved in file.runs {
            if !keep(saved.project) || inner.runs.contains_key(&saved.run) {
                continue;
            }
            inner.seq += 1;
            let seq = inner.seq;
            // Whether the real harness can be put back on this row's conversation (M42): a
            // `claude` transcript still filed under the worktree the row stamped, or an
            // `opencode` `ses_…` the run named. The one `is_file` restore makes per row.
            let reopenable = match saved.harness {
                Harness::Claude | Harness::Qwen => match (saved.session, root_of(saved.project)) {
                    (Some(session), Some(root)) => transcript(
                        saved.harness,
                        &checkout_dir(&root, saved.checkout.as_deref()),
                        session,
                    ),
                    _ => false,
                },
                Harness::Opencode | Harness::Codex | Harness::Mimo => {
                    saved.harness_session.is_some()
                }
            };
            // History restores as history; everything that still had a child restores as
            // `Interrupted` — see `SavedRun::state`. A terminal row keeps its session only
            // when the conversation can be re-opened: the process behind it died with the old
            // cide, and before `open_plan` existed an Open aimed at such a session mirrored
            // nothing. Now Open asks the registry first, and for this row the answer is the
            // real harness on the transcript — which needs the id. A row whose transcript is
            // gone drops it, so the withheld-not-disabled rule holds through `openable`.
            let (state, session, note) = match saved.state {
                Some(state @ (RunState::Finished { .. } | RunState::Failed { .. })) => {
                    (state, saved.session.filter(|_| reopenable), None)
                }
                _ => (
                    RunState::Interrupted,
                    saved.session,
                    Some(
                        "its child ended with a cide restart; Resume continues the conversation"
                            .to_string(),
                    ),
                ),
            };
            inner.runs.insert(
                saved.run,
                LiveRun {
                    run: saved.run,
                    agent: saved.agent,
                    agent_label: saved.agent_label,
                    harness: saved.harness,
                    project: saved.project,
                    session,
                    task: saved.task,
                    task_title: saved.task_title,
                    // Not persisted: a restored run is `Interrupted` and its *next* child is a
                    // resume, which re-reads the task. Persisting it would be a third copy of a
                    // fact `.cide/tasks.json` already holds, and the one most likely to be stale
                    // after a restart — the change may have been archived while cide was down.
                    change: None,
                    // Restored, so a run interrupted by a restart comes back on the candidate it
                    // had settled on rather than walking back to the top of its pool — which is
                    // the one thing an ordered pool exists to prevent.
                    pool: saved.pool,
                    pool_index: saved.pool_index,
                    // **Not** persisted. A restored run is `Interrupted` with no child, so a
                    // latch about a dead process's last words would be a claim about nothing —
                    // the argument `death_noted` and `stale_turn` already make for themselves.
                    provider_failure: None,
                    spent: Vec::new(),
                    pool_note: None,
                    pool_name: saved.pool_name,
                    last_model: saved.last_model,
                    // Not persisted. A restored run's next child is a resume that re-reads the
                    // settings and stamps this afresh; there is no frozen child whose argv this
                    // could be compared against.
                    forked_with: None,
                    stopping: false,
                    // Not persisted — see `LiveRun::stop`. A restored row is `Interrupted` with
                    // no child, and a pending wind-down is a promise about a process that is gone.
                    stop: None,
                    // A resumed run has had at least one prompt, so its next failure is a
                    // mid-conversation one and must not abandon the transcript.
                    turns: 1,
                    state,
                    started_unix_ms: saved.started_unix_ms,
                    worked_ms: saved.worked_ms,
                    // A restored run is `Interrupted` — not work — so no interval is open and
                    // the gap between the snapshot and this launch is excluded by construction.
                    working_since_unix_ms: None,
                    prompt: saved.prompt,
                    note,
                    slot: false,
                    agent_limit: saved.agent_limit.max(1),
                    project_limit: saved.project_limit.max(1),
                    checkout: saved.checkout,
                    notify: saved.notify,
                    // Review runs are never saved (see `save_snapshot`), so a restored run was work.
                    purpose: RunPurpose::Work,
                    // A restored run has no child until it is resumed, and the resume calls
                    // `note_cwd` like a first start does.
                    cwd: None,
                    seq,
                    frozen: None,
                    stale_turn: false,
                    harness_session: saved.harness_session,
                    continuing: false,
                    requeued: None,
                    follow_ups: Vec::new(),
                    opening_requeues: 0,
                    death_noted: false,
                    reopenable,
                    // Not persisted — see the field. The figure described a child this process
                    // never met, and the resume's child starts a context of its own.
                    usage: None,
                },
            );
            restored.insert(format!("{}.log", saved.run));
        }
        drop(inner);

        // The log prune, `save_screens_for_snapshot`'s pattern one directory over: a run the
        // snapshot no longer names is a run whose post-mortem nobody can reach from any row,
        // and its log would otherwise sit on disk for ever. Restored runs — history included —
        // keep theirs; that file is the trail the debug report asked for.
        if let Ok(entries) = std::fs::read_dir(run_logs_dir()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !restored.contains(&name) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Write the screen of every run that still has a live child, for [`ResumePoint`]'s
    /// preload — so a resumed run's pane starts with what the old child last showed, above the
    /// continuation, instead of starting blank.
    ///
    /// Teardown-only, deliberately: a screen per flush would be a `vt100` render per run per
    /// second for a file nothing reads until the next restart. A crash therefore loses the
    /// screens and keeps the runs, which is the right half to keep. The directory is pruned to
    /// the runs written, so it cannot grow past [`RECENT_KEPT`]-ish per project ever.
    fn save_screens_for_snapshot(&self, sessions: &SessionRegistry) {
        self.save_screens_to(&screens_dir(), sessions);
    }

    /// The write, path-parameterised for the same reason [`Self::write_snapshot_to`] is: so a
    /// test never touches the real state directory — and so *what this actually produces* is
    /// assertable at all. Every screen the last teardown on the reporting machine wrote was ten
    /// bytes, `full_state`'s lead-in over a mirror with nothing in it, and nothing in the suite
    /// could have seen that: this function had no test, because it had no seam.
    fn save_screens_to(&self, dir: &std::path::Path, sessions: &SessionRegistry) {
        if let Err(error) = std::fs::create_dir_all(dir) {
            tracing::warn!(%error, "no run-screens directory; resumed panes will start blank");
            return;
        }
        let live: Vec<(RunId, SessionId)> = {
            let inner = self.inner.lock();
            inner
                .runs
                .values()
                .filter(|live| {
                    !matches!(
                        live.state,
                        RunState::Finished { .. } | RunState::Failed { .. } | RunState::Interrupted
                    )
                })
                .filter_map(|live| live.session.map(|session| (live.run, session)))
                .collect()
        };
        let mut kept: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (run, session) in live {
            let Some(pty) = sessions.get(session) else {
                continue;
            };
            let name = format!("{run}.screen");
            // `full_state`, not `screen_state`: the file preloads the resumed child's fresh
            // mirror, and a one-screen picture cost a resumed run everything above its final
            // two dozen lines — the same amputation History's Open had, fixed the same day.
            // The whole transcript replays into the new mirror's scrollback, so a pane on the
            // continued run can scroll back past the restart.
            if std::fs::write(dir.join(&name), pty.full_state()).is_ok() {
                kept.insert(name);
            }
        }
        // The prune: whatever an earlier process left for runs this one no longer holds.
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !kept.contains(&name) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// Which of the panel's groups a state belongs to. See `AgentRoster::Ready::runs`.
fn group_rank(state: &RunState) -> u8 {
    match state {
        RunState::Running | RunState::AwaitingPermission => 0,
        RunState::Starting => 1,
        RunState::Paused { .. } => 2,
        RunState::Idle => 3,
        // Beside `Queued`, because that is what it is about to become: an interrupted run is
        // actionable, and sorting it into the finished tail would bury the one row that still
        // has a Resume on it.
        RunState::Queued | RunState::Interrupted => 4,
        RunState::Finished { .. } | RunState::Failed { .. } => 5,
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

// ==========================================================================================
// Starting a run.
// ==========================================================================================

/// Everything a spawn needs that lives outside `.cide/` — read once, on the caller's thread.
///
/// Gathered as a value rather than looked up inside the spawn for [`RunPlan`]'s stated reason:
/// every one of these is behind a lock or an `AppHandle`, and a `RunPlan` built from arguments is
/// what makes `cide-agents`' argv rules provable without any of them.
struct Facts {
    root: PathBuf,
    theme: Theme,
    proxy: ProxySettings,
    claude: ClaudeSettings,
    /// Settings → Harness → Codex, for a role whose harness is codex. (M93)
    codex: cide_ipc::CodexSettings,
    /// The providers and pools this fork's child is configured with. (M45)
    ///
    /// Read here with everything else, on the thread that already holds the lock, because a
    /// provider edited under a queued run must not change what that run is already doing — the
    /// rule `unattended` states beside the `RunPlan` literal below.
    llm: cide_ipc::LlmSettings,
    /// This project's local, uncommitted role redirections. (M45)
    ///
    /// Read here with the rest, on the same thread and before anything forks, so a run is
    /// resolved against the overrides as they stood at *this* dispatch — an override edited
    /// under a queued run must not change what that run turns out to be.
    ///
    /// From the profile's config directory and never the checkout: `cide_ipc::overrides`' header
    /// carries the argument, which is that a teammate's clone must not name a pool they do not
    /// have.
    overrides: cide_ipc::ProjectOverrides,
    /// What opencode resolves as this project's own configuration, asked of the binary the first
    /// time an opencode role needs it and not before — a `claude` fork never pays for it. `None`
    /// inside means it was asked and could not answer, which degrades a fork to "let opencode
    /// pick" and a Resume comparison to "nothing to compare"; the log has the sentence.
    /// See `cide_agents::harness::opencode::UserConfig` for why every opencode child needs it.
    opencode: std::cell::OnceCell<Option<cide_agents::harness::opencode::UserConfig>>,
    /// The same answer from `mimo`, the opencode fork, asked of *its* binary: the two CLIs keep
    /// separate configuration files, so one project has two defaults. (M81)
    mimo: std::cell::OnceCell<Option<cide_agents::harness::opencode::UserConfig>>,
    hook_bin: Option<PathBuf>,
    hook_sock: Option<PathBuf>,
    agent_sock: Option<PathBuf>,
    /// Where `openspec` is, for `spec_preamble` to name. (M28)
    ///
    /// Resolved here rather than left to the child, and for `hook_bin`'s reason exactly: it is
    /// installed by `npm -g` into a Node directory that a shell rc file puts on `PATH` and a
    /// desktop launcher does not, while a child gets `toolchain::extra_dirs` — `~/.cargo/bin`
    /// and `~/go/bin`. A bare `openspec` in a run's brief would resolve to nothing on precisely
    /// the machines that have it.
    spec_cli: Option<PathBuf>,
    /// What this project types to run OpenSpec's apply workflow, or `None`. (M28)
    ///
    /// Read off `.claude/` here, on the thread that already has the root, because
    /// `cide_agents` may not go to disk — see `RunPlan::spec_apply`.
    spec_apply: Option<String>,
}

/// Read the workspace and the two sockets. One lock acquisition, released before anything forks.
fn facts(app: &AppHandle, project: ProjectId) -> Result<Facts> {
    let state = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .ok_or_else(|| CoreError::Io("the workspace is not available".into()))?;
    let (root, theme, proxy, claude, codex, llm) = state.with(|ws| {
        let root = cide_core::workspace::project(ws, project)?
            .roots
            .first()
            .map(|root| root.path.clone())
            .ok_or(CoreError::NoRoots)?;
        Ok::<_, CoreError>((
            root,
            ws.settings.theme,
            ws.settings.proxy.clone(),
            ws.settings.claude.clone(),
            ws.settings.codex.clone(),
            ws.settings.llm.clone(),
        ))
    })?;

    // Before `root` moves into the struct, like `spec_apply` below and for the same reason.
    let root_key = root.to_string_lossy().to_string();
    Ok(Facts {
        // A project that never ran `openspec init --tools claude` has none, which is the
        // ordinary case and adds nothing to the brief. Read before `root` moves into the struct.
        spec_apply: cide_spec::claude::line(&root, "apply-change"),
        root,
        theme,
        proxy,
        claude,
        codex,
        // Read after the workspace lock is released — it is a file, and `facts` holds that lock
        // for as short a time as it can.
        overrides: cide_core::persist::load_agent_overrides(
            &cide_core::persist::agent_overrides_path(),
        )
        .project(&root_key),
        llm,
        opencode: std::cell::OnceCell::new(),
        mimo: std::cell::OnceCell::new(),
        hook_bin: cide_hook_binary(),
        hook_sock: app
            .try_state::<crate::hooks::HookServer>()
            .map(|server| server.socket().to_path_buf()),
        agent_sock: app
            .try_state::<crate::agent_rpc::AgentRpcServer>()
            .map(|server| server.socket().to_path_buf()),
        // A miss is not a failure: a project with no OpenSpec never reads this, and a run whose
        // change cannot name an absolute path still gets the bare word, which works for a user
        // whose shell has it. `find` re-probes on a miss, so installing it mid-session and
        // dispatching again picks it up without a relaunch.
        spec_cli: cide_spec::discover::find().ok(),
    })
}

impl Facts {
    /// An opencode-shaped CLI's resolved configuration for this project, read once per flavour.
    /// See the fields. `None` for a harness that is not one.
    fn user_config(&self, harness: Harness) -> Option<&cide_agents::harness::opencode::UserConfig> {
        let flavor = cide_agents::harness::opencode::Flavor::of(harness)?;
        let cell = match harness {
            Harness::Mimo => &self.mimo,
            _ => &self.opencode,
        };
        cell.get_or_init(
            || match cide_agents::harness::opencode::user_config(flavor, &self.root) {
                Ok(config) => Some(config),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        harness = flavor.program(),
                        "the harness could not say what it resolves as this project's \
                         configuration; its children will run on whatever it picks, and a Resume \
                         cannot compare it"
                    );
                    None
                }
            },
        )
        .as_ref()
    }

    /// The role as it resolves here, with the CLI's own default folded in where cide names no
    /// model — `Resolved::with_default_model`, fed from the one place that can read it. Only a
    /// harness that reads the provider document has such a default; `user_config` answers
    /// `None` for every other, and `with_default_model` ignores them anyway.
    fn resolve(&self, agent: &cide_agents::LoadedAgent) -> cide_agents::overrides::Resolved {
        self.resolve_with(agent, &self.overrides)
    }

    /// [`Self::resolve`] with no local override applied: the role runs exactly the harness it
    /// names. For an MR review (M85), whose harness is the one the user just picked.
    fn resolve_pinned(&self, agent: &cide_agents::LoadedAgent) -> cide_agents::overrides::Resolved {
        self.resolve_with(agent, &cide_ipc::ProjectOverrides::default())
    }

    fn resolve_with(
        &self,
        agent: &cide_agents::LoadedAgent,
        overrides: &cide_ipc::ProjectOverrides,
    ) -> cide_agents::overrides::Resolved {
        let resolved = cide_agents::overrides::resolve(agent, overrides, &self.llm);
        if !resolved.harness.reads_provider_document() {
            return resolved;
        }
        let default = self
            .user_config(resolved.harness)
            .and_then(|config| config.model.clone());
        resolved.with_default_model(default)
    }

    /// What a child of `resolved` is forked with, for the record every fork keeps and the
    /// comparison a Resume makes. See `LiveRun::forked_with`.
    fn child_settings(
        &self,
        resolved: &cide_agents::overrides::Resolved,
    ) -> cide_agents::overrides::ChildSettings {
        let config = match resolved.harness.reads_provider_document() {
            true => self.user_config(resolved.harness),
            false => None,
        };
        resolved.child_settings(&self.llm, config)
    }
}

/// The `cide-hook` binary, beside our own executable and **absolute**.
///
/// The third copy of four lines in this workspace, and deliberately not a shared helper: the two
/// in `cmd::session` are private to that module, this crate is the only one that may see
/// `current_exe`, and the alternative — a `pub fn` in `cmd::session` — would make a session
/// module the home of a fact about packaging. Absolute is the whole point: a run's cwd is a git
/// worktree and its `PATH` is the user's, so a bare `cide-hook` in an `--mcp-config` resolves to
/// nothing and the CLI reports `CONNECTION_CLOSED` from a process three levels below anything
/// cide logs.
///
/// # It says so when it misses, once
///
/// `None` reaches `cide_agents::dispatch_refusal` and refuses the dispatch, which is the user's
/// half of the answer. The `warn!` is the other half: it names the path that was looked for, so a
/// log from a machine where dispatch is refused says *which* file to build rather than leaving
/// the reader to guess where "beside the executable" resolved to. Before this, a missing hook was
/// silent in every channel cide has.
///
/// The **check** runs every call and the **log** runs once. Both halves matter. Re-checking is
/// what lets somebody build `cide-hook` into a running instance and have the next dispatch work
/// without a relaunch — the same "re-probes on a miss" courtesy `Facts::spec_cli` extends to
/// `openspec`. Logging once is because this is reached from `roster`, which the agents coalescer
/// rebuilds on every change: a per-call warning would print a paragraph about a static fact of the
/// installation every time anybody touched a task.
pub(crate) fn cide_hook_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let hook = exe.parent()?.join("cide-hook");
    if !hook.exists() {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            tracing::warn!(
                path = %hook.display(),
                "no cide-hook beside this executable; subagent runs have no task tools and are refused"
            );
        });
        return None;
    }
    Some(hook)
}

/// What a successful start produced.
struct Started {
    session: Arc<PtySession>,
    opening: Option<Vec<u8>>,
    cwd: PathBuf,
    /// The JSON event file the child writes, when its harness reports that way. See
    /// [`event_tap`].
    events: Option<PathBuf>,
}

impl AgentRegistry {
    /// Start every run the queue will admit. Returns at once; each start runs on its own task.
    pub fn pump(self: &Arc<Self>, app: &AppHandle) {
        let (admitted, noted) = self.take_admissions_noting();
        for project in noted {
            self.mark_changed(app, project);
        }
        for admission in admitted {
            let registry = Arc::clone(self);
            let app = app.clone();
            tauri::async_runtime::spawn(async move { registry.start(app, admission).await });
        }
    }

    /// Bring one admitted run to a live child, or fail it with a sentence its row can show.
    async fn start(self: Arc<Self>, app: AppHandle, admission: Admission) {
        // Minted *before* the fork and recorded *before* the fork, so the child's first
        // `SessionStart` — applied on the hook thread the instant it arrives — finds a run this
        // registry can already name. See `bind_session`.
        let (session_id, resume) = match admission.resume.clone() {
            // A resumed claude run keeps its old id — `--resume <id>` keeps the conversation's
            // identity, so reusing it keeps hook routing and the row continuous. An opencode
            // child is a new cide session either way; `--session` names the conversation.
            Some(point) => (
                point.rebind.unwrap_or_else(SessionId::new),
                Some(point.conversation),
            ),
            None => (SessionId::new(), None),
        };
        self.bind_session(admission.run, session_id);
        self.bring_up(app, admission, session_id, resume, None)
            .await;
    }

    /// Fork one child for a run and wire it up, fresh or continuing.
    ///
    /// `resume` is the **harness's own** conversation id and is `Some` only for a respawn — see
    /// [`Self::respawn`]. Everything else about the two is identical by construction, which is
    /// the point of there being one function: a continuing child that differed from a fresh one
    /// in its worktree, its environment or its exit wiring would be a second process-hosting
    /// path, and this module's header says there is not one.
    async fn bring_up(
        self: &Arc<Self>,
        app: AppHandle,
        admission: Admission,
        session_id: SessionId,
        resume: Option<String>,
        // What the dying child last had on screen, and why it is being carried, when this fork
        // continues a run whose pane a person may be reading. `None` for an ordinary start. (M45)
        preload: Option<(Vec<u8>, Carried)>,
    ) {
        let run = admission.run;
        let project = admission.project;
        // Cloned out before `admission` moves into the fork's closure; wanted again only if the
        // child comes up.
        let task = admission.task.clone();
        // And what this child would be forked with again, should its opening never take — see
        // `abandon_unsubmitted`. A fork of the same conversation (`rebind: None`) and the same
        // prompt: the child that loses them has read neither.
        let requeue = Requeued {
            resume: resume.clone().map(|conversation| ResumePoint {
                rebind: None,
                conversation,
            }),
            prompt: admission.prompt.clone(),
        };

        let facts = match facts(&app, project) {
            Ok(facts) => facts,
            Err(error) => {
                self.finish_failed(&app, run, error.to_string());
                return;
            }
        };

        let registry = Arc::clone(self);
        let for_blocking = app.clone();
        let started = blocking(move || {
            start_child(
                &for_blocking,
                &registry,
                &facts,
                &admission,
                session_id,
                resume.as_deref(),
                preload,
            )
        })
        .await;

        let started = match started {
            Ok(started) => started,
            Err(error) => {
                self.finish_failed(&app, run, error.to_string());
                return;
            }
        };

        // Before the registry insert, so no window and no reaper can learn about this session
        // before something is watching for its death — `cmd::session::session_spawn`'s ordering,
        // kept because this is the same path and not a parallel one.
        crate::lifecycle::watch_for_exit(app.clone(), session_id, &started.session);
        self.watch_exit(&app, session_id, &started.session);

        if let Some(sessions) = app.try_state::<SessionRegistry>() {
            sessions.insert(session_id, Arc::clone(&started.session));
        }

        // And only now the opening prompt. `None` for a harness that took the prompt in its argv
        // instead; for `claude` it is written into the terminal, because a bare prompt does not
        // begin with `-` and `--mcp-config <configs...>` would swallow it — see
        // `HarnessSpawn::opening`. Through `type_submitted_line`, not a plain write: the TUI
        // does not exist yet, and a chunk this long read whole off the boot buffer is bundled
        // as a paste with its Enter eaten — a run that starts, idles at a full composer, and
        // holds its slot and worktree while reporting nothing. See the helper's measurements.
        //
        // `type_opening_line` rather than the plain helper, because that measurement is not the
        // whole story: under load the paste trap still springs, and the run it springs on holds
        // its concurrency slot for ever — see `type_opening_line` for the selfcraft queue it
        // stalled.
        if let Some(opening) = started.opening {
            let registry = Arc::clone(self);
            let for_stuck = app.clone();
            type_opening_line(&app, session_id, &started.session, opening, move || {
                registry.abandon_unsubmitted(&for_stuck, run, session_id, requeue);
            });
        }

        // The event tap, for a harness that reports through a file rather than through hooks
        // or its stdout (M43). After the registry insert, like the watchers: what the tap
        // observes is applied to a run every window can already name.
        if let Some(path) = started.events.clone() {
            event_tap::spawn(
                app.clone(),
                Arc::clone(self),
                run,
                session_id,
                Arc::clone(&started.session),
                path,
            );
        }

        self.note_cwd(run, &started.cwd);
        // The board reflects that somebody is on it now: `Todo → Doing`, and only that hop —
        // `note_run_started`'s own doc has the reviewer-on-a-review-task case and the respawn
        // idempotence argument. After the child is live rather than at enqueue, so a run the
        // queue held for minutes does not claim a task nobody has started.
        if let Some(task) = task.as_ref() {
            crate::task_triggers::note_run_started(&app, project, task);
        }
        tracing::info!(%run, session = %session_id, cwd = %started.cwd.display(), "subagent run started");
        self.mark_changed(&app, project);
    }

    /// Put a run whose opening prompt never started a turn back on its queue, giving its slot
    /// back. Called by [`type_opening_line`] once its Enter retries are spent.
    ///
    /// # Why a requeue and not a failure
    ///
    /// Nothing about the *work* went wrong: the child was never told what the work was. A
    /// failure would hand a person a retry that cide can do itself, and a loaded machine — the
    /// case that springs this — is also the case in which several runs lose their opening at
    /// once. So the run goes back to the **front** of its role's queue (it had already waited its
    /// turn) and is admitted again as slots allow, forked with the same conversation and the
    /// same prompt ([`Requeued`]). Only after [`OPENING_REQUEUES`] of those does it fail, with a
    /// sentence, because a child that cannot be handed a prompt that many times running is not
    /// failing for a reason one more fork will change.
    ///
    /// # The order under the lock, and why the kill comes after it
    ///
    /// Decided by [`opening_never_took`], because this thread has been asleep for most of a
    /// minute and the run may have moved on in any direction. Then, before the lock is dropped,
    /// the session is **unbound**: the dying child's exit then belongs to nobody, rather than
    /// finding its run `Queued` and answering `Finished` over it — `respawn`'s rebind-before-kill
    /// rule. The successor is a fresh cide session that forks the conversation, which is what
    /// makes the unbinding safe for a `claude` continuation whose id *was* the session.
    fn abandon_unsubmitted(
        self: &Arc<Self>,
        app: &AppHandle,
        run: RunId,
        session: SessionId,
        requeue: Requeued,
    ) {
        let outcome = {
            let mut inner = self.inner.lock();
            match inner.runs.get(&run) {
                Some(live) if opening_never_took(live, session) => {}
                _ => return,
            }
            requeue_unsubmitted(&mut inner, run, requeue)
        };
        match outcome {
            OpeningOutcome::Requeued(attempt) => {
                tracing::warn!(%run, %session, attempt, "an opening prompt never started a turn; requeued the run");
                self.kill_child(app, Some(session));
                after_transition(app, self, run);
            }
            OpeningOutcome::GiveUp => {
                self.finish_failed(app, run, OPENING_NEVER_SUBMITTED.to_string());
                self.kill_child(app, Some(session));
            }
        }
    }

    /// Fail a run that never reached a child, tell the windows, and let the queue move on.
    fn finish_failed(self: &Arc<Self>, app: &AppHandle, run: RunId, reason: String) {
        tracing::warn!(%run, %reason, "a subagent run did not start");
        // The id was minted and recorded before the fork (see `start`), and there is now no child
        // behind it. Left in place it would put an Open control on a row with nothing to open, and
        // `agent_open_pane` would bind a session `SessionRegistry` has never held into a pane.
        if let Some(live) = self.inner.lock().runs.get_mut(&run) {
            live.session = None;
        }
        self.fail(Some(app), run, reason);
        // The slot this run was holding is free again, so the next queued run of that role may
        // now start. Without this the queue stalls on the first failure, silently.
        after_transition(app, self, run);
    }

    /// Report the child's real exit into the run, from `cide-pty`'s reaper thread.
    ///
    /// A second `on_exit` registration beside [`crate::lifecycle::watch_for_exit`]'s rather than a
    /// hop inside it: `PtySession::on_exit` keeps a list, the pane path has no business knowing
    /// what a run is, and this way a run's exit is wired where runs are — one file, one reader.
    fn watch_exit(self: &Arc<Self>, app: &AppHandle, session: SessionId, pty: &Arc<PtySession>) {
        let app = app.clone();
        let for_failover = app.clone();
        let for_failover_app = app.clone();
        let for_wait = app.clone();
        self.watch_exit_with(
            session,
            pty,
            move |registry, run| {
                after_transition(&app, registry, run);
                // The exit's nudge, delivered by the wrapper that holds the app — the observation
                // itself travels app-free so a test can drive it (see `set_state`'s `None` arm).
                // For a claude run this is a child that died; for an opencode run it is the *normal*
                // end of every turn, which is why finished runs could not stay silent.
                let project = registry
                    .inner
                    .lock()
                    .runs
                    .get(&run)
                    .map(|live| live.project);
                if let Some(project) = project {
                    crate::agent_rpc::note_run_over(&app, project, run);
                }
            },
            move |registry, failover| {
                // Nothing was released — the branch above returned before `observe` — so there is no
                // `after_transition` to run here: no slot came free, nothing can be admitted, and the
                // run is `Starting` with its checkout still its own. All that is owed is the row's new
                // sentence and the fork.
                registry.mark_changed(&for_failover, failover.project);
                let registry = Arc::clone(registry);
                tauri::async_runtime::spawn(async move {
                    registry.fork_failover(for_failover_app, failover).await;
                });
            },
            move |registry, project| {
                registry.mark_changed(&for_wait, project);
                registry.pump(&for_wait);
            },
        );
    }

    /// The registration, with what follows the transition left to the caller.
    ///
    /// Split out for the reason `lifecycle::watch_exit_with` states about itself: reporting needs
    /// an `AppHandle`, this build cannot make one — `tauri`'s mock app is behind a feature it does
    /// not enable — and a test can therefore reach the interesting half only by not testing it.
    /// Everything that can be wrong is here, where `a_real_childs_exit_finishes_its_run` drives it
    /// against a real reaped process.
    fn watch_exit_with(
        self: &Arc<Self>,
        session: SessionId,
        pty: &Arc<PtySession>,
        then: impl FnOnce(&Arc<Self>, RunId) + Send + 'static,
        // The app half of a failover: fork the next candidate's child. Separate from `then` for
        // this method's own stated reason — forking needs an `AppHandle`, this build cannot make
        // one, and a test drives `plan_failover` directly with a closure that does nothing. (M45)
        over_to: impl FnOnce(&Arc<Self>, Failover) + Send + 'static,
        // The app half of a failover that has to *wait* for a candidate with room: redraw and
        // pump, because the slot it gave back may start somebody else. (Pool running limits.)
        wait: impl FnOnce(&Arc<Self>, ProjectId) + Send + 'static,
    ) {
        let registry = Arc::clone(self);
        pty.on_exit(move |exit| {
            // **Before the observation, and that ordering is the whole change.** `observe`
            // answers `Finished` to an exit from any state, and `set_state` releases the slot and
            // the checkout on it — after which the queued sibling is admitted and `bring_up` winds
            // down whatever it finds in the checkout. A failover decided after that would re-fork
            // into a directory the queue has already given away. See `plan_failover`.
            // A restart decided at Resume, carried out by the exit it asked for. First, because
            // the run is no longer bound to this session and `plan_failover` could not find it.
            if let Some(restart) = registry.plan_restart(session) {
                over_to(&registry, restart);
                return;
            }
            match registry.plan_failover_or_wait(session, exit.code) {
                Some(FailoverPlan::Fork(failover)) => {
                    over_to(&registry, *failover);
                    return;
                }
                // Unbound from this session under the lock, so the `observe` below finds nobody
                // and cannot end it: the run is queued, and its slot came free.
                Some(FailoverPlan::Queued(project)) => {
                    wait(&registry, project);
                    return;
                }
                None => {}
            }
            // `Observation::Exit` is the only observation carrying ground truth about the child,
            // so the harness answers it from any state — which is what turns an `Idle` run whose
            // child was killed into a `Finished` one rather than leaving it idle for ever.
            // `None`: this closure is the app-free half by design (see this method's doc), and
            // an exit is the one observation that can never be the handed-back edge — the harness
            // answers `Finished` and nothing else to it — so there is no nudge to lose here.
            if let Some((run, _)) = registry.observe(None, session, Observation::Exit(exit.code)) {
                then(&registry, run);
            }
        });
    }

    /// The hook a [`SessionBinding::Harness`] run's session is spawned with: the raw channel
    /// and the display rendering, one closure, in that order.
    ///
    /// # Why this exists at all, and only for some runs
    ///
    /// A `claude` run reports through the hook socket: `cide-hook` echoes `CIDE_SESSION` back on
    /// ten hook points, `note_hook` applies them, and nothing has to read a byte of the child's
    /// output. `opencode` has no hooks and no `--settings`, so **its `--format json` stream is
    /// the only channel it has**: without this hook an opencode run's row sits at `Starting`
    /// until the process dies, and a follow-up has no conversation id to name.
    ///
    /// # A `LineRender`, where a sink used to be — and what the move deleted
    ///
    /// This was a `cide_pty::Sink` once, and its doc spent paragraphs on the three hazards that
    /// came with being one: the ack-credit debt (a sink that never pays is choked and starts
    /// receiving rendered screens — fatal to a scanner), the re-entrant sink-list lock (the
    /// first version parked the coalescer for the whole process), and the Weak-vs-Arc cycle
    /// through the session's own sink list. The render hook has none of them by construction:
    /// it is called by the coalescer *outside* the sink-list lock, owes no acknowledgement
    /// because it is not credited, and is owned by the coalescer thread rather than by the
    /// session. `cide_pty::render_lines` owns the line splitting now and hands this closure
    /// complete lines — the `Scan` buffer, its cap and its resync all moved there.
    ///
    /// What forced the move is the display half: sinks now carry the **rendered** stream (the
    /// harness's `render`, so a pane opened onto the run reads prose rather than ndjson), and a
    /// scanner attached as a sink would have been parsing its own output format's rendering.
    /// Observation therefore reads each raw line here, before the rendering is returned.
    ///
    /// # `app: Option<&AppHandle>`
    ///
    /// [`Self::set_state`]'s convention, with [`Self::watch_exit_with`]'s motive: `None` means
    /// *this caller cannot nudge or admit*, and the only such caller is a test — `tauri`'s mock
    /// app is behind a feature this build does not enable, so a hook that required one could be
    /// exercised only by running the application.
    ///
    /// Production passes `Some`, and that is not a detail: the `Running → Idle` edge this hook
    /// produces **is** the product-owner nudge for a harness with no hooks, so a `None` here in
    /// earnest would be an opencode run that finishes its turn with nothing typed into the
    /// orchestrator's console and nothing started off its queue.
    // Eight, of which three are `SessionBinding::Harness`'s own function pointers and two are the
    // run's identity. Bundling them would mean a struct whose only purpose is to be unpacked at
    // the single call site, which `cide_git::pull` reached the same conclusion about.
    #[allow(clippy::too_many_arguments)]
    fn stream_hook(
        self: &Arc<Self>,
        app: Option<&AppHandle>,
        run: RunId,
        session: SessionId,
        capture: fn(&str) -> Option<String>,
        keep: fn(&str) -> bool,
        render: fn(&mut RenderState, &str, Option<u64>) -> cide_pty::Rendered,
        // Which CLI's stream this is, so its provider verdicts can be read. (M45)
        kind: Harness,
    ) -> cide_pty::LineRender {
        let registry = Arc::clone(self);
        // The ring a click resolves against (M42): the same per-session store a shell pane's
        // structured log lines live in, so `session_log_detail` answers for a run's events
        // with no second command. `None` in a test without an app, where no handle is minted
        // and the rendering carries no token.
        let ring = app.and_then(|app| {
            app.try_state::<Arc<crate::logring::JsonLogRing>>()
                .map(|ring| Arc::clone(&ring))
        });
        // What the rendering remembers between lines — one per session, behind a lock only
        // because the hook is `Fn` and runs on one thread anyway.
        let state = Mutex::new(RenderState::default());
        let app = app.cloned();
        // A one-way latch: a respawn continues the same conversation and prints the same id,
        // so this is one write per run rather than one parse per line for its whole life.
        let captured = AtomicBool::new(false);
        // Resolved once, outside the closure: `for_kind` answers a `&'static dyn Harness`, so
        // there is nothing per-line to look up and nothing to keep alive. (M45)
        let diagnoser = cide_agents::for_kind(kind);
        // The post-mortem tee — see `run_logs_dir`. Raw lines, before rendering: a death is
        // debugged from what the child said, not from what the pane showed of it.
        let log = Mutex::new(RunLog::at(
            run_logs_dir().join(format!("{run}.log")),
            RUN_LOG_CAP,
        ));
        cide_pty::LineRender::new(Arc::new(move |line: &str| {
            log.lock().append(line);
            if !captured.load(Ordering::Acquire)
                && let Some(harness_session) = capture(line)
            {
                registry.note_harness_session(run, harness_session);
                captured.store(true, Ordering::Release);
            }
            // Runs on the coalescer thread — the thread every byte of every session flows
            // through — which is where the sink version ran too: the registry lock, the emit
            // coalescer and the nudge are all costs that thread already carries.
            if let Some((moved, _)) =
                registry.observe(app.as_ref(), session, Observation::Line(line))
                && let Some(app) = app.as_ref()
            {
                after_transition(app, &registry, moved);
            }
            // The provider's verdict, latched on the run and read by the exit handler. It moves
            // **nothing**: `observe` above owns every transition, and this is derived from a line
            // that harness deliberately answers `None` to. Making it a transition would be an
            // end-of-turn declared by a line, which releases the concurrency slot and hands the
            // checkout to the queued sibling — run 06202dd6's bug arriving through a new door.
            //
            // `diagnose`'s own first act is a substring test, so the ordinary line costs no parse
            // on this thread. (M45)
            if let Some(reason) = diagnoser.and_then(|harness| harness.diagnose(line)) {
                registry.note_provider_failure(run, reason);
            }
            // And what the step spent, latched the same way and for the same reason: it moves
            // nothing, and the card that reads it is opened by a person long after this line
            // scrolled past. (M80) `usage`'s own first act is a substring test, so the ordinary
            // line costs no parse on this thread — `diagnose`'s rule one statement up.
            if let Some(spent) = diagnoser.and_then(|harness| harness.usage(line)) {
                registry.note_usage(run, spent);
            }
            // The wall clock, read once, on the coalescer thread, at the only moment this line
            // is in hand — `logring`'s module header makes the whole argument, and M62 gave the
            // renderer the same need: codex states no clock on any item, so a thinking block's
            // duration is the silence it ended and cide is the only thing that can time it. One
            // read for both, because two reads of `SystemTime::now()` for one line are two
            // different answers to when it arrived.
            let now = crate::logring::now_unix_ms();
            // Kept before it is rendered and only when the harness says a person may want it
            // whole — `json_log_render`'s rule, one hook over. The raw line is what the card
            // shows; the rendering is what the pane shows.
            let handle = keep(line)
                .then(|| {
                    ring.as_ref()
                        .map(|ring| ring.record_at(session, line.trim(), now))
                })
                .flatten();
            let mut state = state.lock();
            state.now_unix_ms = Some(now);
            render(&mut state, line, handle)
        }))
    }

    /// Write down the harness a run is **actually** being forked with. (M78)
    ///
    /// # Why a run's harness is written twice
    ///
    /// `DispatchSpec::harness` is `agent.def.harness` — the committed file's answer — because at
    /// dispatch there is no child and the definition is the only thing that has spoken. That is a
    /// *plan*. A local override may redirect the role onto another CLI, and `start_child` has
    /// always honoured it: `RunPlan::harness` is `resolved.harness`, and the comment there says
    /// in as many words that reading the definition would "fork the harness the file names while
    /// every other decision above was made for the one the user chose".
    ///
    /// Every other decision — except this one. The registry went on holding the definition's
    /// answer for the life of the run, and `observe` reads it to pick the state machine that
    /// interprets the child's output. So a role redirected from `opencode` to `codex` was forked
    /// as codex and read as opencode: codex says `thread.started` and `item.completed`, opencode's
    /// machine matches neither, every line answered `None`, and **the run never left
    /// `RunState::Starting`** for its whole life. It worked perfectly and reported nothing — the
    /// panel's mark is `circle-dashed`, which is a still glyph, so the visible symptom was a
    /// spinner that never span. `holds_its_checkout` keeps a `Starting` run's worktree, so the
    /// queue behind it also never moved.
    ///
    /// Written at the fork and not at the dispatch, because the fork is the moment the answer
    /// stops being a plan: the override file is re-read there (a `git checkout` may have changed
    /// it while the run sat in the queue), and a queued run has no child to be wrong about.
    /// Every later reader wants the fact rather than the file — the stop route's `deliver`, the
    /// transcript `Open` guards, the tools vocabulary spliced into the prompt, the label the row
    /// draws, and the snapshot a restored run is rebuilt from.
    fn note_harness(&self, run: RunId, harness: Harness) {
        let mut inner = self.inner.lock();
        let Some(live) = inner.runs.get_mut(&run) else {
            return;
        };
        if live.harness == harness {
            return;
        }
        tracing::debug!(
            %run,
            from = ?live.harness,
            to = ?harness,
            "a local override redirected this run onto another harness",
        );
        live.harness = harness;
    }

    /// Write down the conversation id a harness minted for a run. Answers whether it was new.
    fn note_harness_session(&self, run: RunId, harness_session: String) -> bool {
        let mut inner = self.inner.lock();
        let Some(live) = inner.runs.get_mut(&run) else {
            return false;
        };
        // Never overwritten. A respawn passes the captured id back with `--session`, so the id
        // the continuing child prints is the same one; a second write would be a no-op on a good
        // day and a lost conversation on the day it is not.
        if live.harness_session.is_some() {
            return false;
        }
        tracing::debug!(%run, session = %harness_session, "a run named its harness session");
        live.harness_session = Some(harness_session);
        // And that id is what `opencode --session` takes: the conversation can be re-opened in
        // the real harness from now on, however this child ends.
        live.reopenable = true;
        true
    }

    /// Which project a run belongs to.
    /// Is a run that is not over standing in — or queued to stand in — this checkout? (M89)
    /// [`retire_worktree`]'s first half; its doc says why `Interrupted` does not count.
    fn checkout_in_use(&self, project: ProjectId, name: &str) -> bool {
        self.inner.lock().runs.values().any(|run| {
            run.project == project
                && run.checkout.as_deref() == Some(name)
                && !matches!(
                    run.state,
                    RunState::Finished { .. } | RunState::Failed { .. } | RunState::Interrupted
                )
        })
    }

    /// This run's checkout, if the run is over and had one. What [`after_transition`] offers
    /// [`retire_worktree`].
    fn ended_checkout(&self, run: RunId) -> Option<(ProjectId, String)> {
        let inner = self.inner.lock();
        let live = inner.runs.get(&run)?;
        if !matches!(
            live.state,
            RunState::Finished { .. } | RunState::Failed { .. }
        ) {
            return None;
        }
        Some((live.project, live.checkout.clone()?))
    }

    fn project_of(&self, run: RunId) -> Option<ProjectId> {
        self.inner.lock().runs.get(&run).map(|live| live.project)
    }

    /// Decide a stop and write it down, under the lock. The half a test can drive. (M67)
    ///
    /// `plan_respawn`'s split, and it carries the same weight: everything that *decides* is here
    /// and in [`stop_route`] beneath it, everything that signals, types or spawns is in
    /// [`AgentRegistry::stop`]. The registry never reads a disk from here — the grace arrives in
    /// the [`StopRequest`], already read at the door.
    fn plan_stop(
        &self,
        project: ProjectId,
        run: RunId,
        request: &StopRequest,
    ) -> Result<(StopRoute, StopFacts)> {
        let sealed = self.snapshot_sealed.load(Ordering::SeqCst);
        let mut inner = self.inner.lock();
        let restart_pending = inner.restarts.values().any(|failover| failover.run == run);
        let live = inner
            .runs
            .get_mut(&run)
            .filter(|live| live.project == project)
            .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;

        let facts = StopFacts {
            session: live.session,
            task: live.task.clone(),
            harness: live.harness,
        };
        // A `Delivery::Respawn` harness continues a conversation it has to be able to name, so
        // for those the ask is only possible once the child has said what it calls itself. A
        // `Stdin` harness is typed into and needs nothing. Asked here rather than inside
        // `stop_route`, which must stay pure of the registry.
        let can_continue = match cide_agents::for_kind(live.harness) {
            Some(harness) => match harness.deliver("") {
                Delivery::Stdin(_) => true,
                Delivery::Respawn => live.harness_session.is_some(),
            },
            // No implementation for this harness in this build: nothing to ask, and the kill
            // road works for any child whatever forked it.
            None => false,
        };

        let route = stop_route(
            &live.state,
            live.session,
            live.stop
                .as_ref()
                .is_some_and(|stop| stop.how.is_winding_down()),
            restart_pending,
            sealed,
            can_continue,
            request.grace,
            request.force,
        );

        // Latched **before any signal**, so the exit this is about to cause cannot be read as a
        // provider's fault and spend a pool candidate restarting the run underneath the person
        // who just pressed Stop. (M45) It stays a bare bool and stays set on every road,
        // including the ask — a provider error raised *during* a wind-down turn must not fork a
        // successor either.
        live.stopping = true;

        let how = match &route {
            StopRoute::Cancel => StopHow::BeforeStart,
            StopRoute::Discard => StopHow::Discarded,
            // Nothing to record: the run has ended and its epitaph, if it had one, is written.
            // Writing a stop record here would be a second claim about a settled death.
            StopRoute::Nothing => return Ok((route, facts)),
            StopRoute::Kill { why } => StopHow::Immediate { why: *why },
            StopRoute::Ask => StopHow::WindingDown {
                saw_a_turn: false,
                deadline_unix_ms: now_unix_ms().saturating_add(request.grace.as_millis() as u64),
            },
        };
        // The *first* stop's words are kept. A second press is an escalation of the first
        // request, not a new one, and its (usually absent) reason must not blank the sentence
        // the caller gave when they first asked.
        let reason = live
            .stop
            .as_ref()
            .and_then(|stop| stop.reason.clone())
            .or_else(|| request.reason.clone());
        live.stop = Some(StopRecord {
            by: request.by,
            reason,
            how,
            grace_secs: request.grace.as_secs(),
        });
        Ok((route, facts))
    }

    /// Ask a run to wind down: interrupt whatever it is doing, then give it the instruction.
    ///
    /// The interrupt is the harness's ([`cide_agents::Harness::interrupt`]) and goes in **before**
    /// the line, so "stop work now" means now rather than "once you have finished the thing I am
    /// stopping you for". It is written straight at the PTY rather than through
    /// `type_submitted_line`, which owes an Enter and would submit an empty composer.
    ///
    /// `send_prompt` then does the rest, unchanged, down whichever road the harness declares —
    /// typed into a live TUI, or a fresh child continuing the same conversation.
    fn ask_to_wind_down(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: RunId,
        facts: &StopFacts,
        request: &StopRequest,
    ) -> Result<()> {
        let session = facts
            .session
            .ok_or_else(|| CoreError::Io("this run has no child to ask".into()))?;
        let harness = cide_agents::for_kind(facts.harness).ok_or_else(|| {
            CoreError::Io(format!(
                "this build has no implementation for the `{:?}` harness",
                facts.harness
            ))
        })?;

        // The bridge is what decides whether the line may name a tracker tool — `None` drops
        // that clause rather than telling a run to call something that was never attached.
        // `continuation_prompt`'s scar, in a third place.
        let bridged = app
            .try_state::<crate::agent_rpc::AgentRpcServer>()
            .is_some()
            .then_some(harness);
        let prompt = wind_down_prompt(facts.task.as_ref(), bridged, request.reason.as_deref());

        if let Some(bytes) = harness.interrupt()
            && let Some(sessions) = app.try_state::<SessionRegistry>()
            && let Some(pty) = sessions.get(session)
        {
            tracing::debug!(%run, "interrupting a run's turn before asking it to wind down");
            pty.write(bytes);
        }

        self.send_prompt(app, project, run, session, &prompt, facts.harness)
    }

    /// Watch a winding-down run for its grace, and end it if it is still here. (M67)
    ///
    /// `watch_thaw`'s shape exactly — one named short-lived thread per stop, a sleep, then one
    /// decision under the lock and an act outside it. Not a timer wheel and not a poll: this
    /// happens at the rate somebody presses Stop, and a process with no runs in it must not wake
    /// up for ever.
    fn watch_wind_down(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: RunId,
        grace: Duration,
    ) {
        let registry = Arc::clone(self);
        let for_thread = app.clone();
        let spawned = std::thread::Builder::new()
            .name("cide-agents-winddown".into())
            .spawn(move || {
                std::thread::sleep(grace);
                if let Some(session) = registry.plan_forced_stop(run) {
                    tracing::info!(%run, "a run did not wind down in its grace; ending it");
                    registry.kill_child(&for_thread, Some(session));
                    registry.mark_changed(&for_thread, project);
                }
            });
        if let Err(error) = spawned {
            // Unlike `watch_thaw`'s equivalent, the fallback here cannot be "the offer is lost":
            // a wind-down with nothing watching it is a run that never ends, holding its role's
            // worktree under a row that says it is stopping. Kill now and say so.
            tracing::warn!(%error, %run, "no thread to watch a wind-down; ending the run now");
            self.record_forced(run, COULD_NOT_ASK);
            let session = self
                .inner
                .lock()
                .runs
                .get(&run)
                .and_then(|live| live.session);
            self.kill_child(app, session);
        }
    }

    /// The grace expired. Answers the session to kill, or `None` — every other outcome. (M67)
    ///
    /// `None` is the ordinary answer: the run wound down in time, and its record already says
    /// so. Under the lock, so the forced record is written by whatever decided to force.
    fn plan_forced_stop(&self, run: RunId) -> Option<SessionId> {
        let mut inner = self.inner.lock();
        let live = inner.runs.get_mut(&run)?;
        // A run that ended on its own is not forced, whatever the clock says — and a row whose
        // stop was already escalated by a second press has had its sentence written once.
        if !live
            .stop
            .as_ref()
            .is_some_and(|stop| stop.how.is_winding_down())
        {
            return None;
        }
        if matches!(
            live.state,
            RunState::Finished { .. } | RunState::Failed { .. } | RunState::Interrupted
        ) {
            return None;
        }
        if let Some(stop) = live.stop.as_mut() {
            stop.how = StopHow::Forced {
                why: RAN_OUT_OF_TIME,
            };
        }
        live.session
    }

    /// Write down that this child is being killed to give its worktree away. (M67)
    ///
    /// Not a stop anybody asked for, and the record says so rather than borrowing the crash
    /// sentence. Keyed by session because that is what the reclaim loop holds.
    fn mark_reclaimed(&self, session: SessionId) {
        if let Some(live) = self
            .inner
            .lock()
            .runs
            .values_mut()
            .find(|live| live.session == Some(session))
            && live.stop.is_none()
        {
            live.stop = Some(StopRecord {
                // The one arm with no hand behind it — the queue did this, not a person and
                // not an orchestrator — and `epitaph`'s `Reclaimed` sentence names nobody for
                // that reason. The field is filled because the struct requires it and read by
                // nothing here; do not start reading it without giving `StopBy` a third
                // variant that tells the truth.
                by: StopBy::User,
                reason: None,
                how: StopHow::Reclaimed,
                grace_secs: 0,
            });
        }
    }

    /// End every idle child of `project` whose task the board now calls `done`.
    ///
    /// # Why an idle run needs ending at all
    ///
    /// A `claude` run hands its turn back and goes `Idle` with its child **still alive** at the
    /// prompt — `RunState::Idle`'s whole content — and the only ways out were a Stop, a reclaim
    /// (`idle_children_in`, which fires only when another run needs *that* checkout, and never
    /// does once worktrees went per-task) and cide quitting. So on a project run through the
    /// claude harness every finished task left a gray `Idle` row and a live process behind it,
    /// under its role, for the rest of the session: selfcraft had fourteen at once, their tasks
    /// integrated and closed. opencode, mimo and codex never showed it, because each of their
    /// turns is a child that exits.
    ///
    /// `done` and not `review`, deliberately: a task in review may be sent back, and the idle
    /// child is exactly what a send-back is typed into, with its whole context still loaded.
    /// Once a task is done nothing will be.
    ///
    /// # What is left alone
    ///
    /// A run whose session a pane is showing (`shown`): somebody opened that conversation to read
    /// it or to take the next turn by hand, and closing it under them is a lost message. A run
    /// that still holds its checkout (`holds_its_checkout`, the admission gate's own predicate):
    /// that is a stop mid-wind-down, and it must end its own way. And a run already carrying a
    /// stop record — every other stop has its own words, and this must not overwrite them.
    ///
    /// Called from the two places a board is broadcast (`tasks_state::broadcast` and
    /// `cmd::tasks::answer`), which between them are every road a task's status changes by:
    /// the panel, a run's tool, the orchestrator's, an integrate, and an edit made on disk.
    pub fn retire_done(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        board: &cide_ipc::TaskBoard,
    ) {
        let cide_ipc::TaskBoard::Ready { tasks, .. } = board else {
            return;
        };
        let done: HashSet<&TaskId> = tasks
            .iter()
            .filter(|row| row.status == cide_ipc::TaskStatus::Done)
            .map(|row| &row.id)
            .collect();
        if done.is_empty() {
            return;
        }
        let Some(sessions) = app.try_state::<SessionRegistry>() else {
            return;
        };
        let shown: HashSet<SessionId> = app
            .try_state::<crate::workspace_state::WorkspaceState>()
            .map(|state| {
                state.with(|ws| {
                    cide_core::workspace::session_panes(ws, Some(project))
                        .into_iter()
                        .map(|found| found.session)
                        .collect()
                })
            })
            .unwrap_or_default();
        for session in self.plan_retire(project, |task| done.contains(task), &shown) {
            if let Some(pty) = sessions.get(session) {
                tracing::info!(%session, "ending an idle run whose task is done");
                pty.kill();
            }
        }
    }

    /// The half of [`Self::retire_done`] that decides, under the lock, and marks each run it
    /// picks **before** any signal — `mark_reclaimed`'s rule, so the exit this causes is read as
    /// what it was.
    fn plan_retire(
        &self,
        project: ProjectId,
        is_done: impl Fn(&TaskId) -> bool,
        shown: &HashSet<SessionId>,
    ) -> Vec<SessionId> {
        let mut inner = self.inner.lock();
        let mut picked = Vec::new();
        for live in inner.runs.values_mut() {
            let Some(session) = live.session else {
                continue;
            };
            if live.project != project
                || live.state != RunState::Idle
                || live.stop.is_some()
                || holds_its_checkout(live)
                || shown.contains(&session)
                || !live.task.as_ref().is_some_and(&is_done)
            {
                continue;
            }
            // Latched with the record, as `plan_stop` does: the exit is not a provider's fault
            // and must not spend a pool candidate forking a successor.
            live.stopping = true;
            live.stop = Some(StopRecord {
                // Nobody's hand, as with `Reclaimed`; read by nothing for this arm.
                by: StopBy::User,
                reason: None,
                how: StopHow::Retired,
                grace_secs: 0,
            });
            picked.push(session);
        }
        picked
    }

    /// Was `run` ended by [`Self::retire_done`]? `note_run_over` asks, to type no line for it.
    pub fn was_retired(&self, run: RunId) -> bool {
        self.inner
            .lock()
            .runs
            .get(&run)
            .and_then(|live| live.stop.as_ref())
            .is_some_and(|stop| stop.how == StopHow::Retired)
    }

    /// Write down that a wind-down ended in a kill after all. See `stop`'s `Err` arm.
    fn record_forced(&self, run: RunId, why: &'static str) {
        if let Some(live) = self.inner.lock().runs.get_mut(&run)
            && let Some(stop) = live.stop.as_mut()
            && stop.how.is_winding_down()
        {
            stop.how = StopHow::Forced { why };
        }
    }

    /// Stop a run: cancel it if it is queued, **ask its child to wind down** if it has one, and
    /// kill it if the grace runs out. (M67)
    ///
    /// Idempotent on a run that has already ended, deliberately — a second press of a Stop button
    /// on a row that finished while the pointer was travelling is not an error to show anybody.
    /// A second press on a row that is *winding down* is not idempotent and is not meant to be:
    /// it is the escalation, and [`stop_route`] says so.
    ///
    /// The decision is [`stop_route`]'s, resolved here under the lock and acted on after it.
    pub fn stop(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: RunId,
        request: &StopRequest,
    ) -> Result<Stopped> {
        let (route, facts) = self.plan_stop(project, run, request)?;

        let outcome = match route {
            StopRoute::Cancel => {
                {
                    let mut inner = self.inner.lock();
                    if let Some(live) = inner.runs.get(&run) {
                        let key = live.key();
                        if let Some(queue) = inner.queues.get_mut(&key) {
                            queue.retain(|queued| *queued != run);
                        }
                    }
                }
                self.fail(Some(app), run, "stopped before it started");
                Stopped::BeforeStart
            }
            StopRoute::Nothing => Stopped::AlreadyOver,
            // No child to kill and no reaper to report one, so the kill arm below would move
            // nothing and the row would be unstoppable, silently. Stopping an interrupted run
            // means "do not resume this": the row closes into Recent, the conversation stays on
            // disk, and a fresh dispatch remains available.
            StopRoute::Discard => {
                self.fail(Some(app), run, "discarded without resuming");
                Stopped::Discarded
            }
            StopRoute::Kill { why } => {
                // The child, through the one registry that owns it. The run's own state moves
                // when the reaper reports, not here: a state written on the way *into* a kill
                // would be a claim about a process that is still running.
                self.kill_child(app, facts.session);
                Stopped::Killed {
                    why: why.map(str::to_string),
                    task: facts.task.clone(),
                }
            }
            StopRoute::Ask => {
                // The reason reaches the board here and not at the end, because this is the one
                // road with a window: up to the whole grace passes before anything else is
                // written, and a cide that quits inside it would otherwise lose the reason
                // entirely. Every other road is over by the time this function returns and says
                // everything it has to say in its epitaph.
                crate::agent_rpc::note_stop_asked(app, project, run);

                match self.ask_to_wind_down(app, project, run, &facts, request) {
                    Ok(()) => {
                        self.watch_wind_down(app, project, run, request.grace);
                        Stopped::WindingDown {
                            secs: request.grace.as_secs(),
                            task: facts.task.clone(),
                        }
                    }
                    // The ask could not be delivered after all — a conversation somebody has
                    // open in a pane, a child that died between the decision and the write. A
                    // graceful stop that silently fails to stop is the worst outcome available
                    // here, so it becomes the outcome it would have had without the asking.
                    Err(error) => {
                        tracing::warn!(%run, %error, "a run could not be asked to wind down; ending it now");
                        self.record_forced(run, COULD_NOT_ASK);
                        self.kill_child(app, facts.session);
                        // The caller gets the refusal's own words — most often
                        // `VIEWED_IN_A_PANE`, which names something they can act on (close the
                        // pane) rather than the generic sentence the epitaph settles for. The
                        // record keeps the short form because `StopHow::Forced` carries a
                        // `&'static str` and a run's epitaph is not the place for a stack of
                        // somebody else's error text.
                        Stopped::Killed {
                            why: Some(format!("{COULD_NOT_ASK} — {error}")),
                            task: facts.task.clone(),
                        }
                    }
                }
            }
        };

        self.mark_changed(app, project);
        self.pump(app);
        Ok(outcome)
    }

    /// Kill a run's child, if it still has one. The one spelling, for `stop`'s three callers.
    fn kill_child(&self, app: &AppHandle, session: Option<SessionId>) {
        if let (Some(session), Some(sessions)) = (session, app.try_state::<SessionRegistry>())
            && let Some(pty) = sessions.get(session)
        {
            pty.kill();
        }
    }

    /// Note that this project's roster changed. Coalesced; see the module header.
    pub fn mark_changed(self: &Arc<Self>, app: &AppHandle, project: ProjectId) {
        // **Before** the early return below, not after it. A burst is one thread claiming the
        // roster flush and every later change bailing out here, so a `mark` placed after the
        // return would only ever see the first change of a burst — and the counts that matter
        // to the header (a run finishing, the queue emptying) are usually the *last* one.
        // The two coalescers are independent by design; each drops what it has already said.
        crate::running::mark(app);
        if !self.emit.mark(project) {
            return;
        }
        let registry = Arc::clone(self);
        let app = app.clone();
        // One short-lived thread per burst rather than a permanent ticker: a process with no
        // subagents in it should not wake up twenty times a second for ever, and the thread ends
        // as soon as it has emitted.
        if let Err(error) = std::thread::Builder::new()
            .name("cide-agents-emit".into())
            .spawn(move || registry.flush(app))
        {
            tracing::warn!(%error, "no thread for the agents emit; this roster change is not broadcast");
        }
    }

    /// Wait out the burst, then emit one roster per project that changed.
    fn flush(&self, app: AppHandle) {
        loop {
            std::thread::sleep(COALESCE_TICK);
            let Some(projects) = self.emit.take_due() else {
                continue;
            };
            for project in projects {
                // The roster is rebuilt from disk here — on this thread, off the hook applier and
                // off the reaper — because it is derived: `.cide/config.json` and the role files
                // can have moved under a running app, and `cide_agents`' module header is
                // explicit that nothing caches them.
                if let Some(roster) = crate::cmd::agents::project_roster(&app, project) {
                    // Unless it is the roster already sent: this flush rebuilds on every
                    // marker, and most markers move nothing a window draws.
                    crate::emit::agents_changed_unless_repeat(&app, project, &roster);
                }
            }
            // The durable half rides the same coalescing: every burst that changed a roster
            // may have changed which runs a restart must bring back.
            self.save_snapshot();
            return;
        }
    }
}

/// Prepare the worktree, build the argv and fork. **On the blocking pool** — every line of it
/// touches a disk or a process.
/// Drawn between a dead child's screen and its successor's first line, when a provider refused.
///
/// Dim, so it reads as cide's voice rather than the model's, and it names the fact rather than the
/// mechanism: a person looking at a pane wants to know the model changed, not that a latch was
/// taken at an exit.
const FAILOVER_SEPARATOR: &[u8] =
    b"\r\n\x1b[2m\xe2\x80\x94 that provider refused; continuing on the next model in the pool \xe2\x80\x94\x1b[0m\r\n";

/// The same, for a run picked up after cide itself restarted.
const RESTART_SEPARATOR: &[u8] =
    b"\r\n\x1b[2m\xe2\x80\x94 resumed after a cide restart \xe2\x80\x94\x1b[0m\r\n";

/// The same, for a run restarted on new settings at Resume. See `AgentRegistry::plan_restarts`.
const SETTINGS_SEPARATOR: &[u8] =
    b"\r\n\x1b[2m\xe2\x80\x94 restarted on the current model settings \xe2\x80\x94\x1b[0m\r\n";

fn start_child(
    app: &AppHandle,
    registry: &Arc<AgentRegistry>,
    facts: &Facts,
    admission: &Admission,
    session: SessionId,
    resume: Option<&str>,
    // The dying child's screen, for a fork that continues a run somebody may be watching, and
    // the sentence that separates it from what comes next. (M45)
    preload: Option<(Vec<u8>, Carried)>,
) -> Result<Started> {
    // Read fresh, at the moment of the spawn, and refused again here. The refusal is not a
    // repetition of the one `agents_dispatch` made: a run can sit in a queue for minutes, and in
    // that time a `git pull` can disable the project or a role's file can be deleted. Spawning on
    // the strength of a check made before the wait is spawning on a fact that has expired.
    let project = cide_agents::load_project(&facts.root);
    // A review's role is built here rather than read, from what its launcher composed (M85) —
    // see `RunPurpose`. Everything below it is the same fork every run takes.
    let synthetic = match &admission.purpose {
        RunPurpose::Work => None,
        RunPurpose::MrReview {
            brief,
            tools,
            harness,
            ..
        } => Some(cide_agents::LoadedAgent::synthetic(
            &admission.agent.0,
            "MR review",
            *harness,
            brief.clone(),
            tools.clone(),
        )),
    };
    let agent = match &synthetic {
        Some(agent) => agent,
        None => project.get(&admission.agent).ok_or_else(|| {
            CoreError::Io(format!(
                "no role named `{}` in this project",
                admission.agent
            ))
        })?,
    };
    // A review belongs to no project role, so "subagents are off for this project" is not its
    // refusal to give: the rest of the gate — the bridge, the harness being installed, a
    // dangerous mode — is. The queue's pause still holds it, which is the user's own brake.
    let gate = match &admission.purpose {
        RunPurpose::Work => project.config.agents.clone(),
        RunPurpose::MrReview { .. } => cide_agents::AgentsConfig {
            enabled: true,
            ..project.config.agents.clone()
        },
    };
    if let Some(why) = cide_agents::dispatch_refusal(agent, &gate, facts.hook_bin.as_deref()) {
        return Err(CoreError::Io(why));
    }

    // Where the run stands. `run_checkout` is the one rule, shared with `plan_dispatch`'s stamp
    // so the admission gate and this fork cannot disagree. Read off the same fresh `project.get`
    // as the refusal, so a role's `worktree: false` flipped mid-queue is honoured at the fork —
    // and a run with no task lands in the project root under every isolation (M40).
    let checkout = match &admission.purpose {
        RunPurpose::Work => {
            cide_agents::run_checkout(agent, &project.config.agents, admission.task.as_ref())
        }
        RunPurpose::MrReview { .. } => None,
    };
    let in_worktree = checkout.is_some();
    let cwd = match checkout {
        Some(name) => {
            // One worktree per (role, task), and the name's determinism is load-bearing — a
            // resumed run recomputes this and must land in the directory its transcript lives
            // under. Recomputed here from the fresh config rather than read off the run, exactly
            // as the refusal above re-reads the role: `bring_up` acts on facts as they stand at
            // the fork.
            //
            // Idempotent, and called before every dispatch rather than once per checkout: a
            // user can delete `.cide/worktrees/<name>` between two runs, and `ensure` repairs
            // a registration whose directory has gone.
            let tree = cide_git::worktree::ensure(&facts.root, &name)
                .map_err(|error| CoreError::Io(error.to_string()))?;

            // One checkout, one process. An `Idle` run whose child is still sitting in *this*
            // directory is wound down at the one moment the checkout is actually needed. See
            // `idle_children_in` — scoped to the name, never the role, since worktrees went
            // per-task. In practice only a `claude` is ever found here: opencode's observer
            // stopped answering `Idle` after run 06202dd6, where a misread mid-turn
            // `step_finish` released the slot and this very kill took a working child —
            // `cide_agents::harness::opencode`'s `observe` doc carries the incident, and an
            // opencode turn now ends only at its process's exit.
            if let Some(sessions) = app.try_state::<SessionRegistry>() {
                for idle in registry.idle_children_in(admission.project, &name, admission.run) {
                    if let Some(pty) = sessions.get(idle) {
                        tracing::info!(session = %idle, checkout = %name, "winding down an idle run to reclaim its worktree");
                        // Marked before the signal, so the epitaph this causes says what it
                        // was. Until M67 a reclaim produced `ended with exit 129 before
                        // finishing this task` — a crash report about a run whose turn had
                        // *already* ended. That was merely odd while the sentence meant nothing
                        // in particular; it is a lie now that it means **nobody asked**.
                        registry.mark_reclaimed(idle);
                        pty.kill();
                    }
                }
            }
            tree.path
        }
        // Nothing separates two runs here — shared isolation, a `worktree: false` role, a run
        // with no task — which is what each of those says on its face (and what
        // `agents_config_set` refuses to arrange by accident). Nothing is wound down to reclaim
        // the root either: the user is standing in it.
        None => match &admission.purpose {
            RunPurpose::Work => facts.root.clone(),
            // The MR's own checkout, which the launcher made. Gone means the review was closed
            // while this run queued, and a reviewer standing in the project root would review
            // the wrong tree without knowing it.
            RunPurpose::MrReview { cwd, .. } if cwd.is_dir() => cwd.clone(),
            RunPurpose::MrReview { .. } => {
                return Err(CoreError::Io(
                    "the MR's checkout is gone — the review was closed; start it again from the MR panel"
                        .into(),
                ));
            }
        },
    };

    // The project's isolated XDG directories for this worktree (`agents.isolateEnv`), from the
    // same fresh `project.get` as the rest of the fork. Only a run in a checkout of its own: a
    // run in the project root stands where the user stands, in the user's environment, and an
    // MR review is nobody's branch to verify. `milestones::before_integrate` calls the same
    // function on the same path, so the verify of this branch sees these very directories.
    let isolated = if in_worktree {
        project.config.agents.isolated_env(&cwd)
    } else {
        Vec::new()
    };

    // What this role actually runs as *here*: the committed definition folded with this
    // machine's local override. Pure, and computed before anything is forked or written, so its
    // one refusal costs nothing when it fires. (M45)
    // A review is pinned to the harness the user picked in the MR panel: a local override that
    // redirects "every role that names nothing" is about the project's roles, not this one.
    let resolved = match &admission.purpose {
        RunPurpose::Work => facts.resolve(agent),
        RunPurpose::MrReview { .. } => facts.resolve_pinned(agent),
    };
    // The only refusal this fold produces: an override naming a pool that is not configured
    // here. Refused rather than degraded, because falling back would answer with a model nobody
    // chose and bill for it — and people build a pool to *cap* spend. See `overrides::resolve`.
    if let Some(refusal) = resolved.refusal {
        return Err(CoreError::Io(refusal));
    }
    // The registry has been holding the *definition's* harness since the dispatch. From here the
    // run has a child, and the child is `resolved.harness`'s — above all for `observe`, which
    // picks the state machine that reads this stream. See `note_harness`.
    registry.note_harness(admission.run, resolved.harness);
    // The definition as the harness reads it: this machine's single-model and effort overrides
    // folded **into** it. Every harness reads `plan.agent.def.model` and `plan.agent.effort`, and
    // until this fold existed the resolution's `model` and `effort` were computed and read by
    // nothing — the Settings screen's Model and Effort fields changed no child. `Resolved::apply`
    // says why it is a fold and not two more plan fields. The file on disk is untouched.
    let folded = resolved.apply(agent);
    let agent = &folded;

    // Where a harness that reports through a JSON event file would write it: beside the agent
    // socket, one per run, so it lives and dies with this cide's runtime directory. Minted for
    // every run and *made* only for a harness that hands it back — see the destructure below.
    let events_path = event_tap::path_for(facts.agent_sock.as_deref(), admission.run);

    let plan = RunPlan {
        run: admission.run,
        session,
        agent,
        cwd: cwd.clone(),
        project: admission.project,
        task: admission.task.clone(),
        task_title: admission.task_title.clone(),
        change: admission.change.clone(),
        spec_cli: facts.spec_cli.clone(),
        spec_apply: facts.spec_apply.clone(),
        prompt: admission.prompt.clone(),
        hook_bin: facts.hook_bin.clone(),
        hook_sock: facts.hook_sock.clone(),
        agent_sock: facts.agent_sock.clone(),
        events_path: Some(events_path.clone()),
        theme: facts.theme,
        // Resolved here, on a thread that may block, because `ProxyEnv::for_target` reads this
        // process's own environment and the scope decision belongs to the settings layer. A
        // subagent is a `claude`, so it takes the `claude` column.
        proxy: cide_core::proxy::ProxyEnv::for_target(&facts.proxy, facts.proxy.scope.claude),
        env: isolated,
        // A headless run has no pane and therefore no measurement. `PaneRestore`'s "only the
        // frontend knows a pane's size" is why the default is written down as a decision: a run
        // later opened into a pane is resized then, through the path a re-docked pane uses.
        geometry: Geometry::default(),
        claude: facts.claude.clone(),
        codex: facts.codex.clone(),
        // From the same `facts` read as everything else above, so the providers a child is
        // configured with are the ones that stood at this fork. See `Facts::llm`.
        llm: facts.llm.clone(),
        // The candidate this run starts on, or `None` when no pool applies. Stamped once, here,
        // and carried across every respawn — a run that re-chose per turn would be a conversation
        // answered by different models with nothing recording which.
        // The candidate this run is on *now*, which is not always the first: a failover has
        // already advanced `pool_index` by the time it re-forks, and a respawn must continue on
        // whatever the run settled on rather than walking back to the top of the list. The pool
        // itself is stamped on the first fork and never re-read — see `LiveRun::pool`.
        choice: registry.stamp_and_choose(
            admission.run,
            &resolved,
            facts.child_settings(&resolved),
        ),
        // The resolved harness, so the implementation `for_kind` picked below and the one the
        // plan claims are the same CLI. A redirected role would otherwise be refused by the very
        // harness it was deliberately routed to. See `RunPlan::harness`.
        harness: resolved.harness,
        // From the same fresh `load_project` the refusal above used, so the mode a child runs
        // with is the file as it stood at this fork. A `git checkout` that changes
        // `agents.permissionMode` is honoured from the next dispatch, never cached past it.
        unattended: project.config.agents.unattended(),
        // A review's connection lists the `cide_mr_*` tools only; its brief is the whole of
        // what it is told. See `RunPlan::tracker_paragraphs`.
        tracker_paragraphs: matches!(admission.purpose, RunPurpose::Work),
    };

    // `resolved.harness`, not `agent.def.harness`: a local override may have redirected this role
    // onto another CLI, and reading the definition here would fork the harness the file names
    // while every other decision above was made for the one the user chose. (M45)
    let harness = cide_agents::for_kind(resolved.harness).ok_or_else(|| {
        CoreError::Io(format!(
            "this build has no implementation for the `{:?}` harness",
            resolved.harness
        ))
    })?;
    // The one difference between a fresh child and a continuing one, and it is the harness's to
    // express: `respawn_spec` names the conversation the CLI already minted. See
    // `AgentRegistry::respawn`.
    let spawn = match resume {
        Some(previous) => harness.respawn_spec(&plan, previous),
        None => harness.spawn_spec(&plan),
    }
    .map_err(|error| CoreError::Io(error.to_string()))?;

    let cide_agents::HarnessSpawn {
        spec,
        opening,
        binding,
        events,
    } = spawn;
    // The FIFO a harness that reports through a JSON event file writes into (M43), made
    // **before** the fork: a path that does not exist when the child opens it becomes a
    // regular file the child appends to, which the tap could only tail. Made only when the
    // harness said it will write there — every other run's path stays unused and unmade.
    if events.is_some() {
        event_tap::make_fifo(&events_path).map_err(|error| {
            CoreError::Io(format!(
                "cannot make the event file {}: {error}",
                events_path.display()
            ))
        })?;
    }
    // Installed **in the spec**, not attached after the fork, and the difference is real twice
    // over: a run whose whole liveness channel is its own output cannot afford to have its
    // first lines delivered to nobody, and the hook is also the display renderer — a line that
    // reached the mirror before it was installed would sit on screen as raw ndjson for ever.
    // Captured before the `match` below reads the binding for the hook, and the reason is the
    // seed a few lines down: a resumed run's mirror is filled by replaying its own log through
    // **these** two functions, so the rows it opens onto are the rows the live stream would
    // have drawn for the same lines. `SessionBinding` is `Copy`, so this costs a move of two
    // function pointers and nothing else. `None` for `Caller` — a claude or qwen run is not
    // teed at all (see `run_logs_dir`) and keeps the saved-screen road.
    let rendering = match binding {
        SessionBinding::Harness { keep, render, .. } => Some((keep, render)),
        SessionBinding::Caller => None,
    };
    let spec = match binding {
        SessionBinding::Harness {
            capture,
            keep,
            render,
        } => spec.render(registry.stream_hook(
            Some(app),
            admission.run,
            session,
            capture,
            keep,
            render,
            // The resolved harness, not the definition's: a redirected role's stream is read by
            // the CLI actually producing it.
            resolved.harness,
        )),
        SessionBinding::Caller => spec,
    };
    // A resumed run's pane opens onto what the run itself last showed, above the continuation —
    // the same mechanism a restored shell pane uses (`SpawnSpec::preload`: mirror only, never
    // the child).
    //
    // **Its own log first, the saved screen second**, and the order is the fix rather than a
    // preference. A resumed opencode or codex run is a new cide session, so the old mirror, the
    // old sinks and the old ring are all gone; the screen file was the only bridge and it is
    // written at teardown only, so a crash loses it — and on the machine this was reported from
    // every screen the last teardown wrote was ten bytes of `full_state` lead-in over an empty
    // mirror. The pane therefore came back holding one separator and the next turn, which is
    // exactly what "the agent started from zero" looks like from outside a conversation that
    // was in fact intact. `replayed_run_log` re-renders the run's own tee, which is keyed by
    // run and spans every child *and* every restart. Absent both, absent preload: a run with
    // nothing recorded has nothing to show, and blank-above-continuation is honest.
    //
    // A failover's or a settings restart's screen is handed in directly (`preload`) and is the
    // *live* mirror of the child that just died, read before this fork; a cide restart's comes
    // off disk. They are separated because their separators differ — "the provider refused,
    // continuing elsewhere", "restarted on new settings" and "resumed after a cide restart" are
    // different facts and a person reading the pane needs to know which.
    let carried = match preload {
        Some((screen, why)) => Some((screen, why.separator())),
        None => admission
            .resume
            .as_ref()
            .and_then(|_| {
                rendering
                    .and_then(|(keep, render)| {
                        replayed_run_log(app, admission.run, session, keep, render)
                    })
                    .or_else(|| saved_screen(admission.run))
            })
            .map(|screen| (screen, RESTART_SEPARATOR)),
    };
    let spec = match carried {
        Some((mut screen, separator)) => {
            screen.extend_from_slice(separator);
            spec.preload(screen)
        }
        None => spec,
    };
    let pty = PtySession::spawn(spec).map_err(|error| CoreError::Io(error.to_string()))?;

    Ok(Started {
        session: pty,
        opening,
        cwd,
        events,
    })
}

/// Run work that touches a disk or forks on the blocking pool.
///
/// The same helper `cmd::agents`, `cmd::tasks` and `cmd::git` each carry, for the same reason:
/// `#[tauri::command(async)]` only moves the call onto the async runtime, where a blocking
/// `read_dir` still occupies a runtime worker for its whole duration.
async fn blocking<T>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| CoreError::Io(format!("the agents worker did not finish: {error}")))?
}

// ==========================================================================================
// The one hop out of `hooks.rs`, and what follows a transition.
// ==========================================================================================

/// A hook frame reached a session. If a run owns it, move the run.
///
/// # Why this is called from `hooks::apply` and what it may not do there
///
/// The hook applier is a **single ordered thread**, and that ordering is a correctness
/// requirement rather than a simplification: a `PostToolUse` applied after a `Stop` takes a
/// session from idle back to busy with nobody at the keyboard. So this hop takes one mutex, maps
/// one frame through the harness and returns. Everything expensive it may cause — rebuilding a
/// roster off disk, forking the next queued run — happens on the coalescer's thread or on a task,
/// never here.
///
/// # Why the frame travels rather than the `SessionState` beside it
///
/// `hooks::decide` has already computed a `SessionState` for this frame, and forwarding that
/// number would need a second copy of `SessionState → RunState` living in `cide-app`. The mapping
/// belongs to [`cide_agents::Harness::observe`], which owns two refusals a caller cannot enforce
/// — a `Paused` run is never moved by a frame that was already in flight, and a late frame never
/// resurrects a finished run — and which decides from the **run's** current state, not the
/// session's. Those two states legitimately differ (a paused run's session is whatever it was
/// when the freeze landed), so re-deriving from the session's would be deciding from the wrong
/// one.
pub fn note_hook(app: &AppHandle, session: &str, frame: &HookFrame) {
    let Some(registry) = app.try_state::<Arc<AgentRegistry>>() else {
        return;
    };
    let Ok(session) = session.parse::<SessionId>() else {
        return;
    };
    let registry = Arc::clone(&registry);
    // Before the mapping, and unconditionally: this is one of the two channels the thaw watch
    // reads, and it has to record the frame *arriving* rather than the frame moving something.
    // A `PostToolUse` mid-turn is the clearest possible evidence that a thawed run’s request
    // survived, and it produces no transition at all when the run is already `Running`.
    registry.saw_life(session);
    if let Some((run, _)) = registry.observe(Some(app), session, Observation::Hook(frame)) {
        after_transition(app, &registry, run);
    }
}

/// Tell every window, and start whatever the transition may have unblocked.
///
/// Both halves are cheap and neither does the work itself: `mark_changed` parks the emit on the
/// coalescer, and `pump` takes the admissions under one lock and spawns a task per run.
pub(crate) fn after_transition(app: &AppHandle, registry: &Arc<AgentRegistry>, run: RunId) {
    if let Some(project) = registry.project_of(run) {
        registry.mark_changed(app, project);
    }
    registry.pump(app);
    // A run that has ended may have been the last thing standing in a checkout whose branch was
    // already taken — the ordinary order is "the run hands its turn back, the orchestrator
    // integrates, the run is wound down", and at the integrate the idle child still held the
    // directory. See `retire_worktree`.
    if let Some((project, name)) = registry.ended_checkout(run) {
        retire_worktree(app, registry, project, name);
    }
}

/// Remove a task's worktree once its work has landed and nothing is standing in it. (M89)
///
/// Called after every integrate — the panel's and the orchestrator's — and after a run ends.
/// Integrating used to leave `.cide/worktrees/<role>-<task>` on disk for good: a full checkout
/// per merged task, and a History row offering Integrate beside each because its worktree was
/// still there.
///
/// Three checks, each asked where its facts live:
///
/// * **no run that is not over holds this checkout** — asked of the registry, because
///   deleting the cwd of a live `claude` (an *idle* one is the usual case at merge time: it
///   handed its turn back and the orchestrator integrated) leaves a process running in a
///   directory that no longer exists. `Interrupted` does not hold it: there is no child, and a
///   Resume's `bring_up` calls `worktree::ensure`, which re-makes the checkout from the kept
///   branch. A queued run does hold it — it is about to stand there;
/// * **no pane's child stands in it** — asked of the session registry by [`a_pane_stands_in`].
///   The registry of runs is not enough, and the case that proved it is **the reviewer**: the
///   tab cide opens for a finished run (M79) is started *in the worktree it reviews*, is not a
///   run, and is usually the very session that called `cide_agent_integrate`. Without this, the
///   developer run's end would delete the directory under a reviewer that had just merged and
///   was about to comment and close the task. An Open pane re-opening a finished run's
///   conversation, and a shell somebody `cd`'d into, are the same case. Each such pane's exit
///   offers the checkout again ([`retire_after_pane_exit`]), so a worktree kept for a reviewer
///   goes when the reviewer's tab does;
/// * **nothing in the checkout would be lost** — asked of git, by
///   `cide_git::worktree::remove_if_integrated`: the branch is in, the tree is clean, `HEAD` is
///   on the branch. Any doubt keeps the directory, and says why in the log.
///
/// On a thread of its own, because the second half is a revwalk, an in-memory merge and a
/// status walk, and the first caller of this is the transition path. A dispatch admitted in the
/// gap between the two halves re-makes the checkout through `ensure` at its `bring_up`, so the
/// race costs a checkout, never work. The branch is always kept, as `worktree::remove` keeps it.
pub(crate) fn retire_worktree(
    app: &AppHandle,
    registry: &Arc<AgentRegistry>,
    project: ProjectId,
    name: String,
) {
    if registry.checkout_in_use(project, &name) {
        tracing::debug!(checkout = %name, "a run still holds this worktree; keeping it for now");
        return;
    }
    let Some(root) = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .and_then(|state| crate::tasks_state::project_root(&state, project).ok())
    else {
        return;
    };
    let app = app.clone();
    let registry = Arc::clone(registry);
    let spawned = std::thread::Builder::new()
        .name("cide-retire-worktree".into())
        .spawn(move || {
            let dir = cide_git::worktree::path_of(&root, &name);
            if a_pane_stands_in(&app, &dir) {
                tracing::info!(checkout = %name, "a pane is standing in this worktree; keeping it");
                return;
            }
            match cide_git::worktree::remove_if_integrated(&root, &name) {
                Ok(cide_git::worktree::Retired::Removed) => {
                    tracing::info!(checkout = %name, "its work has landed; removed the worktree");
                    // The roster's `AgentRun::worktree` just changed, and History's Integrate
                    // with it.
                    registry.mark_changed(&app, project);
                }
                Ok(cide_git::worktree::Retired::Absent) => {}
                Ok(kept) => {
                    tracing::info!(checkout = %name, ?kept, "keeping the worktree");
                }
                Err(error) => {
                    tracing::warn!(checkout = %name, %error, "could not retire the worktree");
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the worktree retirement thread");
    }
}

/// Is a live pane's child standing in `dir`, or anywhere under it? (M89)
///
/// Two readings per session, because each misses what the other sees: where the child was
/// **started** (`PtySession::spawn_cwd` — every platform, and the whole answer for a `claude`,
/// which never changes directory), and where it is **now** (`/proc/<pid>/cwd` — Linux only, and
/// the answer for a shell somebody `cd`'d into the worktree). Both are compared canonically, since
/// a spawn directory is whatever path the caller built and `/proc` hands back a resolved one.
///
/// Exited sessions do not count: the registry keeps their entries after the reap on purpose (see
/// `lifecycle::report_exit`), and a corpse is standing nowhere. With no session registry at all —
/// a state the app never has — the answer is *yes*, because the cost of a wrong *no* is a
/// deleted directory under somebody.
fn a_pane_stands_in(app: &AppHandle, dir: &std::path::Path) -> bool {
    let Some(sessions) = app.try_state::<crate::state::SessionRegistry>() else {
        return true;
    };
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let under = |path: &std::path::Path| {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        path.starts_with(&dir)
    };
    sessions.ids().into_iter().any(|id| {
        let Some(session) = sessions.get(id) else {
            return false;
        };
        if session.has_exited() {
            return false;
        }
        under(session.spawn_cwd())
            || session
                .child_pid()
                .and_then(crate::cmd::session::cwd_of_pid)
                .is_some_and(|cwd| under(&cwd))
    })
}

/// A pane's child has exited; if it was started inside an agent worktree, offer that worktree
/// for retirement again. (M89)
///
/// The other half of [`a_pane_stands_in`]: a worktree kept because a reviewer tab was standing
/// in it has no run left whose end would ask again, so the tab's own exit is what does. Called
/// from `lifecycle::report_exit` for every watched session; anything not under some open
/// project's `.cide/worktrees/` returns at the first check.
pub(crate) fn retire_after_pane_exit(app: &AppHandle, spawn_cwd: &std::path::Path) {
    let Some(state) = app.try_state::<crate::workspace_state::WorkspaceState>() else {
        return;
    };
    // `root_of_checkout` is `path_of` inverted, so this matches exactly the directories cide
    // starts things in — a checkout itself, which is where a reviewer and an Open pane start.
    let Some(checkout_root) = cide_git::worktree::root_of_checkout(spawn_cwd) else {
        return;
    };
    let Some(name) = spawn_cwd
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
    else {
        return;
    };
    let found = state.with(|ws| {
        ws.projects
            .values()
            .find(|project| {
                project
                    .roots
                    .first()
                    .is_some_and(|root| root.path == checkout_root)
            })
            .map(|project| project.id)
    });
    let Some(project) = found else {
        return;
    };
    if let Some(registry) = app.try_state::<Arc<AgentRegistry>>() {
        retire_worktree(app, &registry, project, name);
    }
}

// ==========================================================================================
// Typing a submitted line into a claude TUI.
// ==========================================================================================

/// The paste-detection gap: how long after its text the lone Enter is written.
///
/// 150 ms was measured to fall outside the TUI's paste coalescing (see
/// [`type_submitted_line`]); 250 leaves margin without being felt by anybody watching.
const ENTER_GAP: Duration = Duration::from_millis(250);

/// How long the Enter thread waits for a spawning TUI to come up before submitting anyway.
///
/// Generous, because the cost of expiring early is the whole failure this helper exists to
/// prevent, and the cost of waiting is one parked thread. It expires at all only for a session
/// whose hooks never report in — a build whose hook socket failed to bind answers `Spawning`
/// for ever — and a late submit there still beats a lost one.
const ENTER_BOOT_DEADLINE: Duration = Duration::from_secs(15);

/// Type one submitted line into a `claude` TUI: the text now, the Enter alone once the TUI is
/// up and a beat has passed.
///
/// # Why the Enter travels separately — measured against claude 2.1.245
///
/// The TUI reads raw and detects pastes **by length**: a short chunk submits, but a chunk the
/// size of a real nudge or opening prompt, arriving with its `\r` in the same `read`, is
/// bundled as a paste — the CR becomes a newline in the composer, and the line sits there
/// unsubmitted while later lines stack below it. That was the reported shape, twice over: two
/// nudges piled up in the product owner's input box, and an opening prompt a freshly dispatched
/// run would never have acted on. Probed three ways against the shipped binary: a long
/// one-chunk write never submits, a short one does (which is why the `/bin/sh` `read` test in
/// `agent_rpc` cannot see this — a shell has no paste detection), and text-then-lone-CR submits
/// both into a live TUI and over a composer filled from the pre-boot buffer.
///
/// # Why it waits on the hook state first
///
/// An opening prompt is written before the child's TUI exists, so a fixed delay alone is not a
/// separation — both writes would sit in the kernel buffer and come back in one `read`. The
/// session leaving [`SessionState::Spawning`] is the CLI's own "I am up" (its `SessionStart`
/// hook, applied by `crate::hooks`), and only after it does the gap mean anything. Live-TUI
/// callers — the nudge, a retry — pass the check on the first poll and pay only the gap.
///
/// Bytes not ending in `\r` are not a submitted line and are written verbatim: this helper must
/// not invent an Enter the harness did not produce.
pub(crate) fn type_submitted_line(
    app: &AppHandle,
    session: SessionId,
    pty: &Arc<PtySession>,
    bytes: Vec<u8>,
) {
    type_line(app, session, pty, bytes, None);
}

/// How long after an opening prompt's Enter the session has to show that a turn began before
/// the Enter is sent again. See [`type_opening_line`].
///
/// A submit is announced by `UserPromptSubmit`, which the CLI raises before it calls any model,
/// so this is a hook's round trip through `cide-hook` — generous for a machine booting a dozen
/// CLIs at once, and short against a slot held for the life of the process.
const OPENING_CONFIRM: Duration = Duration::from_secs(8);

/// How many extra Enters an opening prompt may be given. Bounded because an Enter the TUI
/// ignores is not evidence of anything, and a child that never reports a turn after three is
/// wrong in a way another keystroke will not fix.
const OPENING_RETRIES: u32 = 3;

/// [`type_submitted_line`] for a run's **opening** prompt: the same text-then-lone-Enter, and
/// then a check that the Enter actually started a turn, repeating the Enter until one does.
///
/// # Why the measurement above is not enough
///
/// The Enter waits for `SessionStart` and a 250 ms gap, and that separates the two writes only
/// if the TUI *reads* its input within the gap. A TUI that is up but busy — the typical case is
/// a cide restart resuming fifteen runs at once, each `claude --resume` loading a transcript of
/// half a megabyte — reads the text and the `\r` in one chunk, bundles them as a paste, and sits
/// at a full composer. The run has gone `Starting → Idle` on `SessionStart`, which by design
/// releases nothing (the module header's slot note), so it holds its concurrency slot **for
/// ever** while reporting `idle`. On selfcraft two runs did exactly this after a restart, and
/// with three real runs they filled the project's five slots: fifteen queued runs, nothing
/// working, no error anywhere. One of the two transcripts shows the resume's session metadata
/// written at the restart and no user message after it.
///
/// # Why another Enter is the safe repair
///
/// A lone `\r` on a composer holding the pasted text submits it, which is the repair; on an
/// empty composer it does nothing at all. Re-typing the text instead would double it on the
/// composer the paste filled. Only a run's opening gets this: the same helper types nudges into
/// the product owner's own pane, where a spare Enter would submit whatever the person was
/// half-way through writing.
///
/// # And when even that fails, the run is ended
///
/// `on_stuck` runs if no retry produced a turn. The caller fails the run, which is what gives
/// its slot back: a run idling on a composer nothing will ever submit is not a run, and leaving
/// it holding a slot is the silent stall above with a longer fuse. See
/// [`AgentRegistry::abandon_unsubmitted`].
fn type_opening_line(
    app: &AppHandle,
    session: SessionId,
    pty: &Arc<PtySession>,
    bytes: Vec<u8>,
    on_stuck: impl FnOnce() + Send + 'static,
) {
    type_line(app, session, pty, bytes, Some(Box::new(on_stuck)));
}

/// How many times a run is put back on the queue for an opening that never took before it is
/// failed instead. See [`AgentRegistry::abandon_unsubmitted`].
const OPENING_REQUEUES: u8 = 2;

/// The row's sentence for a run [`AgentRegistry::abandon_unsubmitted`] gave up on.
const OPENING_NEVER_SUBMITTED: &str = "its opening prompt was typed but never submitted, three \
     children in a row: each sat at an idle composer holding a concurrency slot, so cide ended \
     it to let the queue move. Dispatch it again.";

/// What [`requeue_unsubmitted`] did.
#[derive(Debug, PartialEq, Eq)]
enum OpeningOutcome {
    /// Back at the front of its queue; the number is which requeue this was.
    Requeued(u8),
    /// Out of requeues. The caller fails it, and `finish_failed` releases the slot.
    GiveUp,
}

/// The whole of a stuck opening's requeue, under the lock the caller already holds. Free rather
/// than a method so a test drives it with a registry and no app.
///
/// Unbinds the session, releases the slot, and puts the run at the **front** of its role's
/// queue, carrying the fork to make. `GiveUp` touches nothing: the failure path owns the rest.
fn requeue_unsubmitted(inner: &mut Inner, run: RunId, requeue: Requeued) -> OpeningOutcome {
    let Some(live) = inner.runs.get_mut(&run) else {
        return OpeningOutcome::GiveUp;
    };
    if live.opening_requeues >= OPENING_REQUEUES {
        return OpeningOutcome::GiveUp;
    }
    live.opening_requeues += 1;
    let attempt = live.opening_requeues;
    live.session = None;
    move_to(live, RunState::Queued, now_unix_ms());
    live.requeued = Some(requeue);
    live.note = Some(
        "its opening prompt was never submitted; back in the queue to start again".to_string(),
    );
    let held = std::mem::replace(&mut live.slot, false);
    let (key, project) = (live.key(), live.project);
    if held {
        release(inner, &key, project);
    }
    inner.queues.entry(key).or_default().push_front(run);
    OpeningOutcome::Requeued(attempt)
}

/// Is this run still the one a stuck opening left behind? All four, or the timer is stale.
///
/// * **The same child** — a respawn, a failover or a restart rebinds the run to a new session,
///   and that child's opening is its own thread's business.
/// * **Still holding its slot** — the one fact that separates `Starting → Idle` (the
///   `SessionStart` window, which releases nothing) from a turn handed back, which releases on
///   the edge. An idle run without a slot has worked and is exactly where it should be.
/// * **`Starting` or `Idle`** — any other state is a turn in progress, a freeze, or an ending.
/// * **Not being stopped** — a stop in flight has its own ending and its own sentence.
fn opening_never_took(live: &LiveRun, session: SessionId) -> bool {
    live.session == Some(session)
        && live.slot
        && live.stop.is_none()
        && matches!(live.state, RunState::Starting | RunState::Idle)
}

/// Whether the session has shown that a submitted prompt reached the CLI.
///
/// `AwaitingInput` counts: [`cide_claude::next_state`] produces it only from `Busy`, so a turn
/// quick enough to begin and end between two polls still left this behind. `Exited` and
/// `Paused` stop the retries for their own reasons — nothing to submit into, and a frozen child
/// would read the keystrokes out of order on resume.
fn opening_settled(state: Option<SessionState>) -> bool {
    matches!(
        state,
        Some(
            SessionState::Busy
                | SessionState::AwaitingInput
                | SessionState::AwaitingPermission
                | SessionState::Exited { .. }
                | SessionState::Paused
        )
    )
}

/// How long a codex TUI is given to come up before a line is typed into it. (M93)
///
/// Codex announces nothing at startup — its `SessionStart` hook fires with the *first prompt*
/// (measured on 0.155.1), so the "wait until the TUI has left `Spawning`" rule below would wait
/// out its whole deadline on every fresh codex pane. A paste written before the TUI reads its
/// input was measured lost (the first probe's paste at eight seconds, while the model was
/// still loading, never reached the composer). So a codex session that has never reported
/// anything is treated as up once it has been alive this long.
const CODEX_BOOT: Duration = Duration::from_secs(6);

/// Which CLI a session runs, when it is a codex one: a console the registry recorded, or an
/// agent run the run registry holds. (M93)
fn session_is_codex(app: &AppHandle, session: SessionId) -> bool {
    let console = app
        .try_state::<crate::state::SessionRegistry>()
        .and_then(|registry| registry.harness_of(session));
    let run = || {
        app.try_state::<Arc<AgentRegistry>>()
            .and_then(|registry| registry.harness_of_session(session))
    };
    console.or_else(run) == Some(Harness::Codex)
}

/// `text` as a bracketed paste: how a codex TUI takes a line whole, newlines and all.
///
/// Idempotent: a codex run's follow-up already arrives wrapped (`CodexHarness::deliver`), and a
/// second wrapping would paste the markers themselves as text.
fn as_paste(text: Vec<u8>) -> Vec<u8> {
    if text.starts_with(b"\x1b[200~") {
        return text;
    }
    let mut out = Vec::with_capacity(text.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(&text);
    out.extend_from_slice(b"\x1b[201~");
    out
}

fn type_line(
    app: &AppHandle,
    session: SessionId,
    pty: &Arc<PtySession>,
    bytes: Vec<u8>,
    // `Some` for a run's opening: confirm the submit, retry the Enter, and call this if no
    // retry started a turn. `None` for everything typed into a pane a person may be using.
    on_stuck: Option<Box<dyn FnOnce() + Send>>,
) {
    let (text, enter) = strip_enter(bytes);
    // A codex TUI (M93) takes the line as a paste, and only once it is up: its text is held for
    // the thread below rather than written now. See `CODEX_BOOT`.
    let codex = session_is_codex(app, session);
    let held = if codex {
        Some(as_paste(text))
    } else {
        pty.write(text);
        None
    };
    if !enter && held.is_none() {
        return;
    }

    let app = app.clone();
    let for_thread = Arc::clone(pty);
    let spawned = std::thread::Builder::new()
        .name("cide-type-enter".into())
        .spawn(move || {
            let state_of = || {
                app.try_state::<crate::hooks::HookServer>()
                    .map(|hooks| hooks.state(session))
            };
            let booted = || {
                codex
                    && app
                        .try_state::<crate::state::SessionRegistry>()
                        .and_then(|registry| registry.started(session))
                        .and_then(|at| at.elapsed().ok())
                        .is_some_and(|alive| alive >= CODEX_BOOT)
            };
            let deadline = Instant::now() + ENTER_BOOT_DEADLINE;
            while Instant::now() < deadline {
                if state_of() != Some(SessionState::Spawning) || booted() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if let Some(text) = held {
                for_thread.write(text);
                if !enter {
                    return;
                }
            }
            std::thread::sleep(ENTER_GAP);
            for_thread.write(b"\r".to_vec());
            let Some(on_stuck) = on_stuck else {
                return;
            };
            for attempt in 1..=OPENING_RETRIES {
                let deadline = Instant::now() + OPENING_CONFIRM;
                while Instant::now() < deadline {
                    if opening_settled(state_of()) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                tracing::warn!(
                    %session,
                    attempt,
                    "an opening prompt started no turn; pressing Enter again"
                );
                for_thread.write(b"\r".to_vec());
            }
            // One last look after the final Enter, which gets the same wait as the others.
            let deadline = Instant::now() + OPENING_CONFIRM;
            while Instant::now() < deadline {
                if opening_settled(state_of()) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            on_stuck();
        });
    if let Err(error) = spawned {
        // No thread means no gap: write the Enter now and say so. A maybe-eaten submit beats a
        // caller blocked for the gap, and beats a line that never gets its Enter at all.
        tracing::warn!(%error, "no thread for the split Enter; submitting inline");
        pty.write(b"\r".to_vec());
    }
}

/// The split, as a pure decision: the bytes to write now, and whether an Enter is owed.
///
/// Exactly one trailing `\r` is peeled — the harness `submit` produces exactly one, and a
/// second would be a second Enter, which is finding 8's whole subject.
fn strip_enter(mut bytes: Vec<u8>) -> (Vec<u8>, bool) {
    let enter = bytes.last() == Some(&b'\r');
    if enter {
        bytes.pop();
    }
    (bytes, enter)
}

// ==========================================================================================
// The emit coalescer.
// ==========================================================================================

/// A trailing debounce with a ceiling, over the set of projects whose roster moved.
///
/// `crate::lsp`'s shape, one order of magnitude down: a diagnostic burst is a compiler
/// publishing, a roster burst is four runs each reporting a tool call. The ceiling is what keeps
/// a run that never stops working visible — a bare trailing debounce would never fire for it.
#[derive(Default)]
struct Coalescer {
    state: Mutex<CoalesceState>,
}

/// A `Mutex` and not atomics: `first` and `last` have to move together, or the ceiling and the
/// trailing debounce disagree about the same burst.
#[derive(Default)]
struct CoalesceState {
    /// Which projects owe an emit. A set, because a burst is usually one project reporting
    /// twenty times and it must cost one roster, not twenty.
    pending: HashSet<ProjectId>,
    /// When the first un-emitted change of this burst arrived. Drives [`COALESCE_CEILING`].
    first: Option<Instant>,
    /// And the most recent. Drives [`COALESCE`].
    last: Option<Instant>,
    /// Whether a flusher thread is already waiting this burst out.
    flushing: bool,
}

impl Coalescer {
    /// Record a change. `true` means *you are the thread that must flush it*.
    fn mark(&self, project: ProjectId) -> bool {
        let mut state = self.state.lock();
        let now = Instant::now();
        state.pending.insert(project);
        state.first.get_or_insert(now);
        state.last = Some(now);
        if state.flushing {
            return false;
        }
        state.flushing = true;
        true
    }

    /// The projects to emit for, or `None` while the burst is still arriving.
    fn take_due(&self) -> Option<Vec<ProjectId>> {
        let mut state = self.state.lock();
        let (first, last) = (state.first?, state.last?);
        if last.elapsed() < COALESCE && first.elapsed() < COALESCE_CEILING {
            return None;
        }
        state.first = None;
        state.last = None;
        state.flushing = false;
        Some(state.pending.drain().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cide_pty::SpawnSpec;

    /// A dispatch as `cmd::agents::plan_dispatch` would have produced it.
    ///
    /// `checkout: None` — shared isolation's stamp — so the numeric limits stay the whole
    /// story in every test that is not *about* the checkout gate; the gate's own tests stamp
    /// names explicitly.
    fn spec(project: ProjectId, agent: &str, agent_limit: u16, project_limit: u16) -> DispatchSpec {
        DispatchSpec {
            project,
            agent: AgentId(agent.into()),
            agent_label: agent.to_string(),
            harness: Harness::Claude,
            task: None,
            task_title: None,
            change: None,
            prompt: "do the thing".into(),
            agent_limit,
            project_limit,
            checkout: None,
            notify: RunNotify::Primary,
            purpose: RunPurpose::Work,
            pool: Vec::new(),
        }
    }

    /// **A checkout is held while any run that is not over stands in it, and only then.** (M89)
    ///
    /// `retire_worktree`'s first half. The case that matters is `Idle`: the orchestrator
    /// integrates right after a run hands its turn back, and the child is still sitting in the
    /// directory. Removing it then would leave a live `claude` with no cwd. The run's end is what
    /// frees it, and `ended_checkout` is what offers it for retirement at that edge.
    #[test]
    fn a_follow_up_is_typed_to_an_idle_run_and_held_for_a_busy_one() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "mr-review", 4, 4));
        let session = SessionId::new();
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&run).expect("run");
            live.session = Some(session);
            live.state = RunState::Running;
        }
        // Mid-turn: a line typed now would land in whatever the model is composing.
        assert!(
            registry
                .plan_follow_up(project, run, "first")
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .plan_follow_up(project, run, "second")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            registry.inner.lock().runs[&run].follow_ups,
            ["first", "second"]
        );

        // Idle: typed now, with what was held, as one line — two lines would be two turns.
        registry.inner.lock().runs.get_mut(&run).expect("run").state = RunState::Idle;
        let (to, _, line) = registry
            .plan_follow_up(project, run, "third")
            .unwrap()
            .expect("an idle run is typed into");
        assert_eq!(to, session);
        assert_eq!(line, "first — then: second — then: third");
        assert!(registry.inner.lock().runs[&run].follow_ups.is_empty());

        // Over: nothing to say it to, and the caller revives the conversation instead.
        assert!(registry.set_state(None, run, RunState::Finished { code: 0 }));
        assert!(registry.plan_follow_up(project, run, "late").is_err());
        assert!(!registry.follow_up_reachable(project, run));
        assert!(
            registry
                .plan_follow_up(ProjectId::new(), run, "elsewhere")
                .is_err(),
            "a run is spoken to only in its own project"
        );
    }

    #[test]
    fn a_revived_reviewer_is_admitted_onto_its_old_conversation_with_the_discussion() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry
            .enqueue_continuing(
                DispatchSpec {
                    prompt: "The user is discussing your draft d1.".into(),
                    ..spec(project, "mr-review", 4, 4)
                },
                "conv-1".into(),
            )
            .expect("no task, nothing to duplicate");
        let (admitted, _) = registry.take_admissions_noting();
        let admission = admitted
            .into_iter()
            .find(|a| a.run == run)
            .expect("admitted");
        assert_eq!(admission.prompt, "The user is discussing your draft d1.");
        let resume = admission.resume.expect("continues a conversation");
        assert_eq!(resume.conversation, "conv-1");
        assert!(
            resume.rebind.is_none(),
            "a fork, so a predecessor still filed under the old id cannot end this run"
        );
    }

    #[test]
    fn a_checkout_is_held_until_the_run_standing_in_it_is_over() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(DispatchSpec {
            checkout: Some("developer-t-7".into()),
            ..spec(project, "developer", 4, 4)
        });
        assert!(
            registry.checkout_in_use(project, "developer-t-7"),
            "a queued run is about to stand there"
        );
        assert!(
            !registry.checkout_in_use(project, "developer-t-8"),
            "another task's is free"
        );
        assert!(
            !registry.checkout_in_use(ProjectId::new(), "developer-t-7"),
            "and so is the same name in another project"
        );
        assert_eq!(
            registry.ended_checkout(run),
            None,
            "a run that is not over offers nothing"
        );

        registry.inner.lock().runs.get_mut(&run).expect("run").state = RunState::Idle;
        assert!(
            registry.checkout_in_use(project, "developer-t-7"),
            "an idle child is still in the directory"
        );

        assert!(registry.set_state(None, run, RunState::Finished { code: 0 }));
        assert!(
            !registry.checkout_in_use(project, "developer-t-7"),
            "over frees it"
        );
        assert_eq!(
            registry.ended_checkout(run),
            Some((project, "developer-t-7".to_string())),
            "and the ended run offers its checkout for retirement"
        );
    }

    // ==========================================================================================
    // The pool failover's decision half (M45). Everything that decides is under the lock and
    // app-free, so it is driven directly here; everything that forks is `fork_failover`'s.
    // ==========================================================================================

    fn pool_entry(provider: &str, model: &str) -> cide_ipc::PoolEntry {
        cide_ipc::PoolEntry {
            provider: provider.into(),
            model: model.into(),
            variant: String::new(),
            max_running: None,
        }
    }

    /// A run admitted, forked, and standing on the first of `n` candidates.
    fn run_on_a_pool(registry: &AgentRegistry, n: usize) -> (ProjectId, RunId, SessionId) {
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 4, 4));
        let mut admitted = Vec::new();
        admit_a_pass(&mut registry.inner.lock(), &mut admitted, &mut Vec::new());
        let session = SessionId::new();
        registry.bind_session(run, session);
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&run).expect("the run");
            live.pool = (0..n)
                .map(|i| pool_entry("openrouter", &format!("model-{i}")))
                .collect();
            live.pool_index = 0;
            live.state = RunState::Running;
            live.harness_session = Some("ses_first".into());
        }
        (project, run, session)
    }

    // ==========================================================================================
    // Pool entries with a running limit: admission places a run on the first entry with room.
    // ==========================================================================================

    /// A pool of `limits.len()` entries, `model-0…`, each with the given `max_running`.
    fn limited_pool(limits: &[Option<u16>]) -> Vec<cide_ipc::PoolEntry> {
        limits
            .iter()
            .enumerate()
            .map(|(i, &max_running)| cide_ipc::PoolEntry {
                max_running,
                ..pool_entry("openrouter", &format!("model-{i}"))
            })
            .collect()
    }

    fn pooled_spec(project: ProjectId, pool: &[cide_ipc::PoolEntry]) -> DispatchSpec {
        DispatchSpec {
            harness: Harness::Opencode,
            pool: pool.to_vec(),
            ..spec(project, "developer", 8, 8)
        }
    }

    fn index_of(registry: &AgentRegistry, run: RunId) -> usize {
        registry.inner.lock().runs[&run].pool_index
    }

    /// **The request exactly**: the first entry allows two, so the third run starts on the next.
    #[test]
    fn a_full_entry_sends_the_next_run_to_the_next_entry() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[Some(2), Some(1)]);
        let runs: Vec<RunId> = (0..3)
            .map(|_| registry.enqueue(pooled_spec(project, &pool)))
            .collect();

        assert_eq!(registry.take_admissions().len(), 3);
        assert_eq!(index_of(&registry, runs[0]), 0);
        assert_eq!(index_of(&registry, runs[1]), 0);
        assert_eq!(index_of(&registry, runs[2]), 1, "entry 0 was full");
    }

    /// A pool whose every entry is full keeps the run queued with a sentence, and the slot a
    /// finished run gives back is what lets it start — on the entry that freed.
    #[test]
    fn a_full_pool_queues_and_a_freed_entry_admits() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[Some(1), Some(1)]);
        let first = registry.enqueue(pooled_spec(project, &pool));
        let second = registry.enqueue(pooled_spec(project, &pool));
        let third = registry.enqueue(pooled_spec(project, &pool));

        let (admitted, noted) = registry.take_admissions_noting();
        assert_eq!(admitted.len(), 2, "the pool holds two");
        assert_eq!(noted, vec![project], "the waiting row is redrawn");
        assert_eq!(state_of(&registry, third), RunState::Queued);
        let note = registry.inner.lock().runs[&third]
            .note
            .clone()
            .unwrap_or_default();
        assert!(note.contains("running limit"), "{note}");
        assert!(note.contains("openrouter/model-0 1/1"), "{note}");

        // Asking again changes nothing, and says nothing new.
        let (admitted, noted) = registry.take_admissions_noting();
        assert!(admitted.is_empty());
        assert!(noted.is_empty(), "the same sentence is not a change");

        assert!(registry.set_state(None, second, RunState::Finished { code: 0 }));
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, third);
        assert_eq!(index_of(&registry, third), 1, "the entry that freed");
        assert_eq!(index_of(&registry, first), 0);
    }

    /// An idle run holds no slot, so it holds no place on its entry either — the provider is not
    /// being asked anything while a run waits for its next instruction.
    #[test]
    fn an_idle_run_leaves_room_on_its_entry() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[Some(1), Some(1)]);
        let first = registry.enqueue(pooled_spec(project, &pool));
        registry.take_admissions();
        assert!(registry.set_state(None, first, RunState::Running));
        assert!(registry.set_state(None, first, RunState::Idle));

        let second = registry.enqueue(pooled_spec(project, &pool));
        registry.take_admissions();
        assert_eq!(index_of(&registry, second), 0);
    }

    /// Counted across projects: pools are machine-wide, and so is a provider's concurrency.
    #[test]
    fn an_entrys_limit_is_shared_by_every_project() {
        let registry = AgentRegistry::default();
        let pool = limited_pool(&[Some(1), None]);
        let here = registry.enqueue(pooled_spec(ProjectId::new(), &pool));
        let there = registry.enqueue(pooled_spec(ProjectId::new(), &pool));
        assert_eq!(registry.take_admissions().len(), 2);
        assert_eq!(index_of(&registry, here), 0);
        assert_eq!(
            index_of(&registry, there),
            1,
            "another project's run fills entry 0"
        );
    }

    // ==========================================================================================
    // The bench (M90): one run's refusal steers every later admission, until it expires or a
    // person resets it.
    // ==========================================================================================

    /// A pooled run admitted onto its first entry, bound, running — and then refused.
    fn refuse_a_pooled_run(
        registry: &AgentRegistry,
        project: ProjectId,
        pool: &[cide_ipc::PoolEntry],
        reason: cide_agents::FailoverReason,
    ) -> (RunId, Option<FailoverPlan>) {
        let run = registry.enqueue(pooled_spec(project, pool));
        registry.take_admissions();
        let session = SessionId::new();
        registry.bind_session(run, session);
        registry.inner.lock().runs.get_mut(&run).unwrap().state = RunState::Running;
        registry.note_provider_failure(run, reason);
        (run, registry.plan_failover_or_wait(session, 1))
    }

    /// **The report**: a local server that is down refuses one run, and the next run starts past
    /// it without spending a turn — and says why on its row.
    #[test]
    fn a_refusal_benches_its_entry_for_the_next_run() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[None, None, None]);
        let (first, plan) = refuse_a_pooled_run(
            &registry,
            project,
            &pool,
            cide_agents::FailoverReason::Unreachable,
        );
        assert!(matches!(plan, Some(FailoverPlan::Fork(_))));
        assert_eq!(index_of(&registry, first), 1);

        let second = registry.enqueue(DispatchSpec {
            agent: AgentId("qa".into()),
            ..pooled_spec(project, &pool)
        });
        registry.take_admissions();
        assert_eq!(index_of(&registry, second), 1, "entry 0 is benched");
        let note = registry.inner.lock().runs[&second]
            .pool_note
            .clone()
            .unwrap_or_default();
        assert!(
            note.contains("openrouter/model-0 benched (unreachable"),
            "{note}"
        );

        let report = registry.pool_state(&cide_ipc::LlmSettings {
            providers: Vec::new(),
            pools: vec![cide_ipc::ModelPool {
                name: "default".into(),
                description: String::new(),
                entries: pool.clone(),
            }],
        });
        assert!(report.off_pool.is_empty(), "both runs are on the pool");
        let entries = &report.pools[0].entries;
        let bench = entries[0].bench.as_ref().expect("entry 0 benched");
        assert_eq!(bench.reason, cide_ipc::PoolRefusal::Unreachable);
        assert_eq!(bench.run, first);
        assert!(entries[1].bench.is_none());
        assert_eq!(entries[1].running, 2, "both runs stand on entry 1");
        assert_eq!(entries[0].provider, cide_ipc::PoolProviderState::Missing);
        // Newest first: the second run's start, the first run's failover start, its refusal, and
        // the first run's own start.
        assert!(matches!(
            report.events[0].kind,
            cide_ipc::PoolEventKind::Started { index: 1, .. }
        ));
        assert!(report.events.iter().any(|event| matches!(
            event.kind,
            cide_ipc::PoolEventKind::Refused {
                reason: cide_ipc::PoolRefusal::Unreachable,
                ..
            }
        )));
    }

    /// **The second report**: a project whose roles were never pointed at a pool runs on the
    /// CLI's default model, and the card must list those runs rather than show the pool idle.
    #[test]
    fn an_opencode_run_with_no_pool_is_listed_as_off_pool() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(pooled_spec(project, &[]));
        registry.enqueue(spec(project, "claude-role", 8, 8));
        registry.take_admissions();
        let report = registry.pool_state(&cide_ipc::LlmSettings::default());
        let off: Vec<RunId> = report.off_pool.iter().map(|r| r.run).collect();
        assert_eq!(
            off,
            vec![run],
            "the claude run is not \"off\" a pool it could never use"
        );
    }

    /// A bench is a preference: when nothing after a benched entry has room, the run is placed
    /// on it anyway rather than parked behind a timer.
    #[test]
    fn a_benched_entry_is_still_taken_when_nothing_else_has_room() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[None, Some(1)]);
        let (first, _) = refuse_a_pooled_run(
            &registry,
            project,
            &pool,
            cide_agents::FailoverReason::RateLimited,
        );
        assert_eq!(index_of(&registry, first), 1, "entry 1 is now full");

        let second = registry.enqueue(DispatchSpec {
            agent: AgentId("qa".into()),
            ..pooled_spec(project, &pool)
        });
        assert_eq!(registry.take_admissions().len(), 1, "admitted, not parked");
        assert_eq!(index_of(&registry, second), 0);
        let note = registry.inner.lock().runs[&second]
            .pool_note
            .clone()
            .unwrap_or_default();
        assert!(note.contains("although it is benched"), "{note}");
    }

    /// Expiry is read, not swept: a bench past its `until` is simply not there.
    #[test]
    fn an_expired_bench_steers_nothing() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[None, None]);
        refuse_a_pooled_run(
            &registry,
            project,
            &pool,
            cide_agents::FailoverReason::Unreachable,
        );
        registry.inner.lock().benched[0].until_unix_ms = now_unix_ms() - 1;

        let second = registry.enqueue(DispatchSpec {
            agent: AgentId("qa".into()),
            ..pooled_spec(project, &pool)
        });
        registry.take_admissions();
        assert_eq!(index_of(&registry, second), 0);
    }

    /// A bench is on the *target*, so a second pool naming the same server steers around it too.
    #[test]
    fn a_bench_is_shared_by_every_pool_naming_the_target() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[None, None]);
        refuse_a_pooled_run(&registry, project, &pool, cide_agents::FailoverReason::Auth);

        // Another pool, another order, the same refused target in second place.
        let other = vec![
            pool_entry("zai", "glm"),
            pool[0].clone(),
            pool_entry("deepseek", "flash"),
        ];
        let run = registry.enqueue(DispatchSpec {
            agent: AgentId("qa".into()),
            ..pooled_spec(project, &other)
        });
        // Start it past entry 0, as a failover would have left it.
        registry.inner.lock().runs.get_mut(&run).unwrap().pool_index = 1;
        registry.take_admissions();
        assert_eq!(index_of(&registry, run), 2);
    }

    /// Reset clears the bench and moves a *waiting* run back onto the entry, but leaves a run
    /// standing on a child exactly where its child is.
    #[test]
    fn a_reset_clears_the_bench_and_rewinds_only_waiting_runs() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[None, None, None]);
        let (running, _) = refuse_a_pooled_run(
            &registry,
            project,
            &pool,
            cide_agents::FailoverReason::Unreachable,
        );
        // A waiting run that a failover had already walked past entry 0.
        let waiting = registry.enqueue(DispatchSpec {
            agent: AgentId("qa".into()),
            project_limit: 1,
            ..pooled_spec(project, &pool)
        });
        registry
            .inner
            .lock()
            .runs
            .get_mut(&waiting)
            .unwrap()
            .pool_index = 2;

        let touched = registry.reset_pool_entry(Some(&pool[0]));
        assert_eq!(touched, vec![project]);
        assert!(registry.inner.lock().benched.is_empty());
        assert_eq!(index_of(&registry, waiting), 0, "moved back up");
        assert_eq!(
            index_of(&registry, running),
            1,
            "its child stays where it is"
        );
        let report = registry.pool_state(&cide_ipc::LlmSettings::default());
        assert!(matches!(
            report.events[0].kind,
            cide_ipc::PoolEventKind::Reset { rewound: 1, .. }
        ));
    }

    /// A run a failover moved down never climbs back to an entry that already refused it, even
    /// when that entry has room.
    #[test]
    fn a_run_is_never_placed_above_where_a_failover_left_it() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[Some(4), Some(4), Some(4)]);
        let run = registry.enqueue(pooled_spec(project, &pool));
        registry.inner.lock().runs.get_mut(&run).unwrap().pool_index = 2;
        registry.take_admissions();
        assert_eq!(index_of(&registry, run), 2);
    }

    /// A failover whose remaining candidates are all at their limit **waits** rather than forks
    /// over the limit: back at the front of its queue, slot released, session unbound — and
    /// admitted onto the next entry once it frees.
    #[test]
    fn a_failover_into_a_full_entry_waits_for_room() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let pool = limited_pool(&[None, Some(1)]);
        // Something else holds entry 1.
        let other = registry.enqueue(DispatchSpec {
            agent: AgentId("other".into()),
            ..pooled_spec(project, &pool)
        });
        registry.take_admissions();
        registry
            .inner
            .lock()
            .runs
            .get_mut(&other)
            .unwrap()
            .pool_index = 1;

        let run = registry.enqueue(pooled_spec(project, &pool));
        registry.take_admissions();
        assert_eq!(index_of(&registry, run), 0);
        let session = SessionId::new();
        registry.bind_session(run, session);
        registry.inner.lock().runs.get_mut(&run).unwrap().state = RunState::Running;

        registry.note_provider_failure(run, cide_agents::FailoverReason::RateLimited);
        match registry.plan_failover_or_wait(session, 1) {
            Some(FailoverPlan::Queued(queued)) => assert_eq!(queued, project),
            other => panic!("expected a wait, got {other:?}"),
        }
        {
            let inner = registry.inner.lock();
            let live = &inner.runs[&run];
            assert_eq!(live.state, RunState::Queued);
            assert!(!live.slot, "the slot is given back while it waits");
            assert_eq!(
                live.session, None,
                "the dying child's exit must find nobody"
            );
            assert_eq!(live.pool_index, 1, "advanced past the entry that refused");
            assert!(live.requeued.is_some(), "carrying the fork to make");
            assert_eq!(inner.queues[&live.key()].front(), Some(&run));
        }
        assert!(
            registry.take_admissions().is_empty(),
            "entry 1 is still full"
        );

        assert!(registry.set_state(None, other, RunState::Finished { code: 0 }));
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, run);
        assert_eq!(index_of(&registry, run), 1);
    }

    // ==========================================================================================
    // What the card a `#7` opens is told about the run behind the line. (M80)
    // ==========================================================================================

    /// The harness the child was **forked with**, the candidate it is on, and the last step's
    /// spend — all read off the run and none of them off the role's file.
    ///
    /// M78's bug is the reason for the first assertion: `spec()` dispatches on `Harness::Claude`
    /// and the fork resolved an override onto opencode, so a card reading the definition would
    /// caption an opencode conversation `claude` — in the one surface built to explain a run
    /// after the fact.
    #[test]
    fn a_log_line_names_the_harness_model_and_spend_of_the_run_behind_it() {
        let registry = AgentRegistry::default();
        let (_project, run, session) = run_on_a_pool(&registry, 3);
        registry.note_harness(run, Harness::Opencode);
        registry.note_usage(
            run,
            cide_ipc::TokenUsage {
                input: 4_000,
                output: 300,
                reasoning: 200,
                cache_read: 1_000,
                cache_write: 64,
            },
        );

        let info = registry
            .log_run_info(session)
            .expect("the run behind the line");
        assert_eq!(
            info.harness,
            Harness::Opencode,
            "the fork's answer, not the file's"
        );
        assert_eq!(
            info.model.as_deref(),
            Some("openrouter/model-0"),
            "the candidate, joined as the child was told it"
        );
        assert_eq!(info.pool_position.as_deref(), Some("entry 1 of 3"));
        assert_eq!(info.usage.map(|spent| spent.context()), Some(5_500));
        assert_eq!(
            info.context_limit, None,
            "nothing in this fixture's configuration states a window"
        );

        // A failover moves the model the card names — which is the whole reason the position is
        // carried beside it.
        registry.note_provider_failure(run, cide_agents::FailoverReason::RateLimited);
        registry
            .plan_failover(session, 1)
            .expect("a candidate is left");
        let info = registry.log_run_info(session).expect("still the same run");
        assert_eq!(info.model.as_deref(), Some("openrouter/model-1"));
        assert_eq!(info.pool_position.as_deref(), Some("entry 2 of 3"));

        // A session no run stands on is the ordinary case — every shell pane in the application.
        assert!(registry.log_run_info(SessionId::new()).is_none());
    }

    /// With no pool, the model is what the child was forked with, and a custom provider's own
    /// declaration is the one context window cide will state.
    #[test]
    fn a_run_with_no_pool_names_its_single_model_and_only_a_declared_window() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        let session = SessionId::new();
        registry.bind_session(run, session);
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&run).expect("the run");
            live.forked_with = Some(forked_on(Harness::Claude, "sonnet"));
        }
        let info = registry.log_run_info(session).expect("the run");
        assert_eq!(info.model.as_deref(), Some("sonnet"));
        assert_eq!(info.pool_position, None, "no pool, no position");
        assert_eq!(info.usage, None, "nothing has finished a step");

        // A window is stated only where a *custom* provider's declaration names one, matched on
        // the provider and the model as two keys — never by splitting `provider/model`.
        let providers = vec![cide_ipc::LlmProvider::Custom {
            id: "lmstudio".into(),
            label: String::new(),
            enabled: true,
            npm: String::new(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            api_key: String::new(),
            models: vec![cide_ipc::LlmModel {
                id: "openai/gpt-oss-20b".into(),
                label: String::new(),
                context: 32_768,
                output: 4_096,
            }],
        }];
        let entry = pool_entry("lmstudio", "openai/gpt-oss-20b");
        assert_eq!(context_limit(&providers, &entry), Some(32_768));
        // `0` is opencode's "write no limit", so it is an absence here too.
        let entry_zero = pool_entry("lmstudio", "nothing-declared");
        assert_eq!(context_limit(&providers, &entry_zero), None);
        // A catalogued provider's window is the catalog's business and cide keeps no copy.
        let catalogued = vec![cide_ipc::LlmProvider::Catalog {
            id: "openrouter".into(),
            label: String::new(),
            enabled: true,
            api_key: String::new(),
        }];
        assert_eq!(
            context_limit(
                &catalogued,
                &pool_entry("openrouter", "anthropic/claude-sonnet-5")
            ),
            None
        );
    }

    /// The ordinary failover: the next candidate is taken, the run keeps its slot, and no queued
    /// sibling is let into the checkout.
    #[test]
    fn a_qualifying_failure_spends_the_next_candidate_and_releases_nothing() {
        let registry = AgentRegistry::default();
        let (project, run, session) = run_on_a_pool(&registry, 3);
        let held = registry.inner.lock().runs[&run].slot;
        assert!(held, "the fixture's run holds its slot");

        registry.note_provider_failure(run, cide_agents::FailoverReason::RateLimited);
        let planned = registry
            .plan_failover(session, 1)
            .expect("a rate limit with a candidate left is a failover");

        assert_eq!(planned.run, run, "the same run, not a second one");
        let inner = registry.inner.lock();
        let live = &inner.runs[&run];
        assert_eq!(live.pool_index, 1, "advanced by exactly one");
        assert!(live.slot, "the slot is still held — nothing was released");
        assert_eq!(
            live.state,
            RunState::Starting,
            "a working phase, so the checkout gate still counts this run as occupying it"
        );
        assert!(
            live.provider_failure.is_none(),
            "the latch was taken, not read"
        );
        drop(inner);
        assert!(
            wire_note(&registry, project, run)
                .expect("a sentence")
                .contains("rate limited"),
            "the row says what happened and where it went"
        );
    }

    /// Sticky: the index only ever rises, so a run never walks back to a candidate it has spent.
    /// This is also the infinite-loop guard — `pool.len() - 1` failovers, whatever else goes wrong.
    #[test]
    fn the_index_is_monotonic_and_bounded_by_the_pool() {
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 2);
        for _ in 0..5 {
            registry.note_provider_failure(run, cide_agents::FailoverReason::Unreachable);
            let planned = registry.plan_failover(session, 1);
            registry.inner.lock().runs.get_mut(&run).unwrap().state = RunState::Running;
            if planned.is_none() {
                break;
            }
        }
        assert_eq!(
            registry.inner.lock().runs[&run].pool_index,
            1,
            "a two-entry pool affords exactly one failover, however many failures arrive"
        );
    }

    /// Exhaustion ends the run the way any run ends, and the row names every candidate and why.
    /// Deliberately not `Failed`: a child ran and exited, and `death_facts` branches on that.
    #[test]
    fn a_run_that_ran_out_of_models_ends_normally_and_says_which_ones_failed() {
        let registry = AgentRegistry::default();
        let (project, run, session) = run_on_a_pool(&registry, 2);

        registry.note_provider_failure(run, cide_agents::FailoverReason::RateLimited);
        assert!(registry.plan_failover(session, 1).is_some());
        registry.inner.lock().runs.get_mut(&run).unwrap().state = RunState::Running;

        registry.note_provider_failure(run, cide_agents::FailoverReason::Unreachable);
        assert!(
            registry.plan_failover(session, 1).is_none(),
            "the pool is spent, so the exit ends the run"
        );

        let note = wire_note(&registry, project, run).expect("a sentence");
        assert!(note.contains("every model in the pool failed"), "{note}");
        assert!(note.contains("openrouter/model-0"), "{note}");
        assert!(note.contains("openrouter/model-1"), "{note}");
        assert!(
            note.contains("rate limited") && note.contains("unreachable"),
            "{note}"
        );
        assert!(
            !matches!(
                registry.inner.lock().runs[&run].state,
                RunState::Failed { .. }
            ),
            "a child ran and exited; `Failed` is for a run that never reached one"
        );
    }

    // ==========================================================================================
    // Stopping a run is a request before it is a kill (M67). Everything that decides is pure or
    // under the lock and app-free, so all of it is driven directly here; everything that
    // signals, types or spawns is `AgentRegistry::stop`'s.
    // ==========================================================================================

    fn route(state: RunState, force: bool) -> StopRoute {
        stop_route(
            &state,
            Some(SessionId::new()),
            false,
            false,
            false,
            true,
            Duration::from_secs(60),
            force,
        )
    }

    /// The states that cannot be asked, each for its own reason and each naming it.
    ///
    /// The two that matter are `AwaitingPermission` and `Paused`, and both are bugs rather than
    /// taste. Do not "simplify" this table into `matches!(state, terminal) else Ask`.
    #[test]
    fn a_run_that_cannot_be_asked_is_killed_and_the_route_says_why() {
        // **The worst thing in this feature's blast radius.** The permission prompt is a
        // selection list, and a wind-down is text followed by a lone `\r` — the `\r` is what
        // the dialog reads. Asking here would approve the very tool call the stop was meant to
        // prevent.
        assert_eq!(
            route(RunState::AwaitingPermission, false),
            StopRoute::Kill {
                why: Some(AWAITING_PERMISSION)
            }
        );
        // A `SIGSTOP`ped process does not act on a signal and does not read its input; the line
        // would sit in the kernel buffer and be read on resume, out of order with whatever it
        // was doing. `AgentRegistry::resume` states the same rule for the queue's follow-ups.
        assert_eq!(
            route(RunState::Paused { since_unix_ms: 1 }, false),
            StopRoute::Kill { why: Some(PAUSED) }
        );

        // A force is not an obstacle, so it names none: `why: None` is what "we never asked, on
        // purpose" looks like, and inventing a sentence for it would read as an apology.
        assert_eq!(
            route(RunState::Running, true),
            StopRoute::Kill { why: None }
        );
        // A project that set the grace to zero has said the same thing, once, in its config.
        assert_eq!(
            stop_route(
                &RunState::Running,
                Some(SessionId::new()),
                false,
                false,
                false,
                true,
                Duration::ZERO,
                false
            ),
            StopRoute::Kill { why: None }
        );

        // Shutting down: `lifecycle::shutdown` seals before the ladder. A grace per run would
        // turn quitting cide into a minutes-long wait, and there would be nobody left to read
        // the comment it bought.
        assert_eq!(
            stop_route(
                &RunState::Running,
                Some(SessionId::new()),
                false,
                false,
                true,
                true,
                Duration::from_secs(60),
                false
            ),
            StopRoute::Kill { why: Some(SEALED) }
        );
        // A second press is the escalation — it is how a caller reaches a kill without a second
        // argument, and why the tool's answer tells them so.
        assert_eq!(
            stop_route(
                &RunState::Running,
                Some(SessionId::new()),
                true,
                false,
                false,
                true,
                Duration::from_secs(60),
                false
            ),
            StopRoute::Kill {
                why: Some(ALREADY_ASKED)
            }
        );
        // A restart already has this child on the way out; there is nothing to speak to.
        assert_eq!(
            stop_route(
                &RunState::Running,
                Some(SessionId::new()),
                false,
                true,
                false,
                true,
                Duration::from_secs(60),
                false
            ),
            StopRoute::Kill {
                why: Some(RESTARTING)
            }
        );
        // A `Delivery::Respawn` harness whose child died before naming its conversation: there
        // is nothing to continue it from, which is `plan_respawn`'s own refusal, hoisted so the
        // stop says it rather than failing downstream.
        assert_eq!(
            stop_route(
                &RunState::Running,
                Some(SessionId::new()),
                false,
                false,
                false,
                false,
                Duration::from_secs(60),
                false
            ),
            StopRoute::Kill {
                why: Some(NO_CONVERSATION)
            }
        );
        // And a live run with no session at all.
        assert_eq!(
            stop_route(
                &RunState::Running,
                None,
                false,
                false,
                false,
                true,
                Duration::from_secs(60),
                false
            ),
            StopRoute::Kill {
                why: Some(NO_CHILD)
            }
        );
    }

    /// The three roads that were never a kill, and the one that now asks.
    #[test]
    fn the_queue_the_history_and_a_working_run_each_take_their_own_road() {
        assert_eq!(route(RunState::Queued, false), StopRoute::Cancel);
        assert_eq!(
            route(RunState::Queued, true),
            StopRoute::Cancel,
            "force changes nothing here"
        );
        assert_eq!(route(RunState::Interrupted, false), StopRoute::Discard);
        assert_eq!(
            route(RunState::Finished { code: 0 }, false),
            StopRoute::Nothing
        );
        assert_eq!(
            route(RunState::Failed { reason: "x".into() }, false),
            StopRoute::Nothing
        );
        assert_eq!(route(RunState::Running, false), StopRoute::Ask);
        assert_eq!(route(RunState::Idle, false), StopRoute::Ask);
        assert_eq!(route(RunState::Starting, false), StopRoute::Ask);
    }

    fn winding() -> StopHow {
        StopHow::WindingDown {
            saw_a_turn: false,
            deadline_unix_ms: 0,
        }
    }

    /// **Two edges, never one.** The rule that decides whether the agent gets its turn at all.
    #[test]
    fn a_stop_mid_turn_does_not_kill_on_the_turn_it_interrupted() {
        // The interrupted turn handing back. Killing here would end the child a beat *before*
        // it was told anything, which is the whole failure this feature exists to prevent.
        assert_eq!(
            wind_down_step(&winding(), true, &RunState::Idle),
            WindDownStep::Wait
        );
        // The wind-down turn starting.
        assert_eq!(
            wind_down_step(&winding(), false, &RunState::Running),
            WindDownStep::Arm
        );
        // A wind-down turn that stops to ask permission has plainly begun. Refusing to arm on
        // it would make the hand-back that follows read as the interrupted turn's, and nothing
        // but the grace would ever end the run.
        assert_eq!(
            wind_down_step(&winding(), false, &RunState::AwaitingPermission),
            WindDownStep::Arm
        );

        let armed = StopHow::WindingDown {
            saw_a_turn: true,
            deadline_unix_ms: 0,
        };
        assert_eq!(
            wind_down_step(&armed, true, &RunState::Idle),
            WindDownStep::Over
        );
        // Armed once. A second `Running` is not a second arming, and must not be an `Over`.
        assert_eq!(
            wind_down_step(&armed, false, &RunState::Running),
            WindDownStep::Wait
        );

        // **The `Delivery::Respawn` ending.** opencode's and codex's wind-down is a whole new
        // child that answers, comments and exits — there is no hand-back at all, so without
        // this the run would die still flagged `WindingDown` and its epitaph would say it died
        // before it could wind down about the one case where everything worked.
        assert_eq!(
            wind_down_step(&winding(), false, &RunState::Finished { code: 0 }),
            WindDownStep::Completed
        );
        // It is armed or not; the clean exit is the answer either way, because a `Respawn`
        // harness's wind-down child may exit before any observation moved the row to `Running`.
        assert_eq!(
            wind_down_step(&armed, false, &RunState::Finished { code: 0 }),
            WindDownStep::Completed
        );
        // A child that crashed or was killed mid-wind-down really did die before it could
        // finish, and `epitaph`'s `WindingDown` arm is the correct sentence for that.
        assert_eq!(
            wind_down_step(&winding(), false, &RunState::Finished { code: 129 }),
            WindDownStep::Wait
        );
        assert_eq!(
            wind_down_step(
                &winding(),
                false,
                &RunState::Failed {
                    reason: "no child".into()
                }
            ),
            WindDownStep::Wait
        );

        // Nothing outstanding: every other run in the process takes this branch on every edge,
        // so it is the one that has to be cheap and the one that must never act.
        for how in [
            StopHow::WoundDown,
            StopHow::Forced {
                why: RAN_OUT_OF_TIME,
            },
            StopHow::Immediate { why: None },
            StopHow::BeforeStart,
            StopHow::Discarded,
            StopHow::Reclaimed,
        ] {
            assert_eq!(
                wind_down_step(&how, true, &RunState::Idle),
                WindDownStep::Wait,
                "{how:?}"
            );
        }
    }

    /// The line is one line, names the harness's own tool, and asks for no comment it cannot get.
    #[test]
    fn the_wind_down_line_is_one_line_in_the_harnesss_own_dialect() {
        let task = TaskId::from("t-7".to_string());
        let claude = cide_agents::for_kind(Harness::Claude).expect("claude");
        let opencode = cide_agents::for_kind(Harness::Opencode).expect("opencode");

        let line = wind_down_prompt(Some(&task), Some(claude), Some("going the wrong way"));
        // An embedded newline is another Enter into a TUI: it would submit the first line as a
        // turn and feed the rest in as further turns. `opening_prompt`'s rule.
        assert!(!line.contains('\n'), "{line}");
        assert!(line.contains("mcp__cide__cide_task_comment"), "{line}");
        assert!(line.contains("t-7"), "{line}");
        assert!(line.contains("going the wrong way"), "{line}");

        // The scar `continuation_prompt` carries: a hard-coded Claude spelling made every
        // opencode run call a tool it did not have.
        let other = wind_down_prompt(Some(&task), Some(opencode), None);
        assert!(
            other.contains(&opencode.tool_name("cide_task_comment")),
            "{other}"
        );
        assert!(!other.contains("mcp__cide__"), "{other}");
        // No reason given: the clause comes off rather than printing a placeholder.
        assert!(!other.contains("reason given"), "{other}");

        // No bridge: the clause naming a tool comes off, and the useful half stays.
        let unbridged = wind_down_prompt(Some(&task), None, None);
        assert!(!unbridged.contains("cide_task_comment"), "{unbridged}");
        assert!(unbridged.contains("t-7"), "{unbridged}");
        assert!(unbridged.contains("then exit"), "{unbridged}");

        // M40's run with no task works in the project root and has nowhere to report. Telling
        // it to comment on a task it has not got would send it looking for one.
        let rootless = wind_down_prompt(None, Some(claude), None);
        assert!(!rootless.contains("task"), "{rootless}");
        assert!(rootless.contains("then exit"), "{rootless}");

        // A multi-line reason is flattened rather than truncated or refused.
        let messy = wind_down_prompt(None, None, Some("one\ntwo\n\nthree"));
        assert!(!messy.contains('\n'), "{messy}");
        assert!(messy.contains("one two three"), "{messy}");
    }

    /// An idle run whose task is done is ended; one in review, one a pane is showing, and one
    /// somebody already stopped are not. See `AgentRegistry::retire_done`.
    #[test]
    fn only_an_idle_run_on_a_done_task_nobody_is_watching_is_retired() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let idle_on = |task: &str| {
            let run = registry.enqueue(spec(project, "developer", 8, 8));
            let session = SessionId::new();
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&run).expect("the run");
            live.state = RunState::Idle;
            live.task = Some(TaskId::from(task.to_string()));
            live.session = Some(session);
            (run, session)
        };
        let (done, done_session) = idle_on("t-1");
        let (in_review, _) = idle_on("t-2");
        let (watched, watched_session) = idle_on("t-3");
        let (stopped, _) = idle_on("t-4");
        registry.inner.lock().runs.get_mut(&stopped).unwrap().stop = Some(StopRecord {
            by: StopBy::Orchestrator,
            reason: None,
            how: winding(),
            grace_secs: 60,
        });
        let is_done = |task: &TaskId| ["t-1", "t-3", "t-4"].contains(&task.0.as_str());
        let shown = HashSet::from([watched_session]);

        assert_eq!(
            registry.plan_retire(project, is_done, &shown),
            vec![done_session]
        );
        assert!(registry.was_retired(done));
        for run in [in_review, watched, stopped] {
            assert!(!registry.was_retired(run));
        }
        // Marked before the signal, and latched: a second board broadcast before the reaper
        // reports picks nothing, and the exit writes no comment on the closed task.
        assert!(registry.plan_retire(project, is_done, &shown).is_empty());
        registry.inner.lock().runs.get_mut(&done).unwrap().state = RunState::Finished { code: 129 };
        assert!(registry.death_facts(done).is_none());
        // Another project's board touches nothing here.
        assert!(
            registry
                .plan_retire(ProjectId::new(), |_| true, &HashSet::new())
                .is_empty()
        );
    }

    /// The admission gate and the reclaim ask one question, and it is this one.
    #[test]
    fn a_winding_down_run_holds_its_checkout_and_an_idle_one_does_not() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 4, 4));
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&run).expect("the run");
            live.state = RunState::Idle;
            live.checkout = Some("developer-t1".into());
            live.session = Some(SessionId::new());
        }
        // The trade `RunState::Idle`'s doc argues for: a parked child is `bring_up`'s to wind
        // down when the directory is actually claimed.
        assert!(!holds_its_checkout(&registry.inner.lock().runs[&run]));
        assert_eq!(
            registry
                .idle_children_in(project, "developer-t1", RunId::new())
                .len(),
            1
        );

        // Asked to wind down: its child has been told to write one last comment and will be
        // dead within the grace. Reclaiming the directory now buys seconds and costs the whole
        // point of asking — and the run's final comment is never written, with nothing anywhere
        // saying why.
        registry.inner.lock().runs.get_mut(&run).unwrap().stop = Some(StopRecord {
            by: StopBy::Orchestrator,
            reason: None,
            how: winding(),
            grace_secs: 60,
        });
        assert!(holds_its_checkout(&registry.inner.lock().runs[&run]));
        assert!(
            registry
                .idle_children_in(project, "developer-t1", RunId::new())
                .is_empty(),
            "the reclaim and the gate must never disagree about one run"
        );
    }

    /// The row says a run is winding down, and says nothing once it is not.
    #[test]
    fn the_row_names_a_wind_down_and_only_a_wind_down() {
        let winding = stop_note(Some(&StopRecord {
            by: StopBy::Orchestrator,
            reason: Some("going the wrong way".into()),
            how: winding(),
            grace_secs: 60,
        }))
        .expect("a wind-down says so");
        assert!(winding.contains("winding down"), "{winding}");
        assert!(winding.contains("the orchestrator"), "{winding}");
        assert!(winding.contains("60s"), "{winding}");

        // Every terminal spelling is the row's own phase a moment later, and the durable
        // account is the task's comments — a second copy here would be one that drifts.
        for how in [
            StopHow::WoundDown,
            StopHow::Forced {
                why: RAN_OUT_OF_TIME,
            },
            StopHow::Immediate { why: None },
            StopHow::BeforeStart,
            StopHow::Discarded,
            StopHow::Reclaimed,
        ] {
            assert!(
                stop_note(Some(&StopRecord {
                    by: StopBy::User,
                    reason: None,
                    how: how.clone(),
                    grace_secs: 0,
                }))
                .is_none(),
                "{how:?}"
            );
        }
        assert!(stop_note(None).is_none());

        // The three sources join in order and the empty ones vanish — a run with nothing to say
        // must not draw a row of dangling dashes.
        assert_eq!(
            compose_note(Some("a".into()), &Some("b".into()), &Some("c".into())).as_deref(),
            Some("a — b — c")
        );
        assert_eq!(
            compose_note(None, &None, &Some("c".into())).as_deref(),
            Some("c")
        );
        assert_eq!(compose_note(None, &None, &None), None);
        assert_eq!(compose_note(None, &Some(String::new()), &None), None);
    }

    /// A wound-down run exits 0, and that is the one clean exit worth a line on the board.
    #[test]
    fn death_facts_speaks_for_a_stopped_run_that_exited_cleanly() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();

        // Unchanged: a run that decided for itself that it was done says nothing. Widening the
        // gate generally would put a line on every clean run and bury the ones that matter.
        let quiet = registry.enqueue(spec(project, "developer", 4, 4));
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&quiet).expect("the run");
            live.task = Some(TaskId::from("t-1".to_string()));
            live.state = RunState::Finished { code: 0 };
        }
        assert!(registry.death_facts(quiet).is_none());

        // Stopped and wound down. Without this arm the *best* outcome of the whole feature
        // leaves no trace at all.
        let stopped = registry.enqueue(spec(project, "developer", 4, 4));
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&stopped).expect("the run");
            live.task = Some(TaskId::from("t-2".to_string()));
            live.state = RunState::Finished { code: 0 };
            live.stop = Some(StopRecord {
                by: StopBy::Orchestrator,
                reason: Some("the assumption was wrong".into()),
                how: StopHow::WoundDown,
                grace_secs: 60,
            });
        }
        let facts = registry.death_facts(stopped).expect("a stopped run speaks");
        assert_eq!(facts.code, Some(0));
        assert!(matches!(
            facts.stop.as_ref().map(|stop| &stop.how),
            Some(StopHow::WoundDown)
        ));
        // Still once, however many times the end edge fires.
        assert!(registry.death_facts(stopped).is_none());
    }

    /// Four deaths that are not a provider's fault, each of which must not spend a candidate.
    #[test]
    fn nothing_cide_caused_ever_spends_a_candidate() {
        // The user pressed Stop.
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        registry.inner.lock().runs.get_mut(&run).unwrap().stopping = true;
        registry.note_provider_failure(run, cide_agents::FailoverReason::Auth);
        assert!(
            registry.plan_failover(session, 1).is_none(),
            "stopped on purpose"
        );
        assert_eq!(registry.inner.lock().runs[&run].pool_index, 0);

        // The process is going down: `final_snapshot` seals before the ladder runs.
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        registry.snapshot_sealed.store(true, Ordering::SeqCst);
        registry.note_provider_failure(run, cide_agents::FailoverReason::Auth);
        assert!(
            registry.plan_failover(session, 1).is_none(),
            "shutting down"
        );

        // Not a working run: a paused one's child died under the freeze.
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        registry.inner.lock().runs.get_mut(&run).unwrap().state =
            RunState::Paused { since_unix_ms: 1 };
        registry.note_provider_failure(run, cide_agents::FailoverReason::Auth);
        assert!(registry.plan_failover(session, 1).is_none(), "not working");

        // A clean exit: a turn opencode retried internally and finished.
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        registry.note_provider_failure(run, cide_agents::FailoverReason::Auth);
        assert!(
            registry.plan_failover(session, 0).is_none(),
            "exit 0 is a turn that succeeded, whatever it printed on the way"
        );
    }

    /// mimo exits 0 after a fatal provider error, so for it the latch alone decides — and the
    /// latch is still the trigger: a clean mimo exit with nothing classified spends nothing.
    /// (M81)
    #[test]
    fn a_mimo_candidate_fails_over_on_its_clean_exit() {
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        registry.note_harness(run, Harness::Mimo);
        registry.note_provider_failure(run, cide_agents::FailoverReason::Unreachable);
        assert!(
            registry.plan_failover(session, 0).is_some(),
            "mimo reports a dead endpoint with an error line and exit 0"
        );

        let registry = AgentRegistry::default();
        let (_, _run, session) = run_on_a_pool(&registry, 3);
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.values_mut().next().unwrap();
            live.harness = Harness::Mimo;
        }
        assert!(
            registry.plan_failover(session, 0).is_none(),
            "a clean mimo exit with no verdict is a finished turn"
        );
    }

    /// No classified failure is no failover, however the child died. The exit code is a guard,
    /// never the trigger.
    #[test]
    fn an_undiagnosed_death_ends_the_run_as_it_always_did() {
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        assert!(registry.plan_failover(session, 1).is_none());
        assert_eq!(registry.inner.lock().runs[&run].pool_index, 0);
    }

    /// The latch belongs to the child that reported it: a verdict left behind would fail the
    /// *next* candidate over on its own clean exit.
    #[test]
    fn a_stale_verdict_cannot_fail_the_next_candidate_over() {
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        registry.note_provider_failure(run, cide_agents::FailoverReason::RateLimited);
        assert!(registry.plan_failover(session, 1).is_some());
        registry.inner.lock().runs.get_mut(&run).unwrap().state = RunState::Running;
        // The successor exits non-zero having reported nothing.
        assert!(
            registry.plan_failover(session, 1).is_none(),
            "the verdict was taken by the first exit and must not be read twice"
        );
        assert_eq!(registry.inner.lock().runs[&run].pool_index, 1);
    }

    /// A first-turn failure abandons its conversation — nothing is lost, and the next candidate
    /// never sees the failed message. A later one keeps it, because it carries real work.
    #[test]
    fn the_first_turns_failure_starts_fresh_and_a_later_one_continues() {
        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        registry.note_provider_failure(run, cide_agents::FailoverReason::RateLimited);
        let planned = registry.plan_failover(session, 1).expect("a failover");
        assert!(
            planned.resume.is_none(),
            "turn one abandons its conversation"
        );
        assert!(
            registry.inner.lock().runs[&run].harness_session.is_none(),
            "and clears the id, or the run goes on naming a conversation it walked away from"
        );

        let registry = AgentRegistry::default();
        let (_, run, session) = run_on_a_pool(&registry, 3);
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&run).expect("the run");
            live.turns = 2;
        }
        registry.note_provider_failure(run, cide_agents::FailoverReason::RateLimited);
        let planned = registry.plan_failover(session, 1).expect("a failover");
        assert_eq!(
            planned.resume.as_deref(),
            Some("ses_first"),
            "a conversation carrying real work is continued, duplicate prompt and all"
        );
    }

    /// A run with no pool at all behaves exactly as it did before pools existed — which is the
    /// claim that every existing run is untouched by this feature.
    #[test]
    fn a_run_with_no_pool_never_fails_over() {
        let registry = AgentRegistry::default();
        let (project, run, session) = run_on_a_pool(&registry, 0);
        registry.note_provider_failure(run, cide_agents::FailoverReason::RateLimited);
        assert!(
            registry.plan_failover(session, 1).is_none(),
            "there is no candidate to advance to, so the exit ends the run"
        );
        let inner = registry.inner.lock();
        let live = &inner.runs[&run];
        assert_eq!(live.pool_index, 0);
        assert_eq!(
            live.state,
            RunState::Running,
            "and nothing moved it to `Starting`"
        );
        assert!(
            live.pool_note.is_none(),
            "and the row gains no pool sentence"
        );
        drop(inner);
        // The exhaustion sentence is for a pool that was *spent*, not for a run that never had
        // one: a run with no pool must read exactly as it did before.
        assert!(wire_note(&registry, project, run).is_none());
    }

    /// One run's `note` **as the wire carries it** — which is the only place the pause reason
    /// exists. Read through `runs_for` rather than off `LiveRun`, deliberately: the note is
    /// derived at wire time, so reaching into the registry would test a field that is never sent.
    fn wire_note(registry: &AgentRegistry, project: ProjectId, run: RunId) -> Option<String> {
        registry
            .runs_for(project)
            .into_iter()
            .find(|wire| wire.run == run)
            .expect("the run is still listed")
            .note
    }

    fn state_of(registry: &AgentRegistry, run: RunId) -> RunState {
        registry
            .inner
            .lock()
            .runs
            .get(&run)
            .expect("the run is still listed")
            .state
            .clone()
    }

    /// The split `type_submitted_line` makes, pinned pure: exactly one trailing Enter is
    /// peeled and owed separately, and bytes that are not a submitted line pass through whole.
    #[test]
    fn the_enter_is_peeled_exactly_once() {
        assert_eq!(strip_enter(b"hi\r".to_vec()), (b"hi".to_vec(), true));
        assert_eq!(strip_enter(b"hi".to_vec()), (b"hi".to_vec(), false));
        // Only the final one — the harness's own — is peeled; anything deeper is the caller's
        // second Enter, which finding 8 already forbids upstream.
        assert_eq!(strip_enter(b"hi\r\r".to_vec()), (b"hi\r".to_vec(), true));
        assert_eq!(strip_enter(Vec::new()), (Vec::new(), false));
    }

    /// A stuck opening ends the run only while the run is still that stuck opening: the same
    /// child, the slot never released, and no turn since. Every way out of that state — a turn
    /// handed back, a rebind, a stop — turns the late timer into a no-op.
    #[test]
    fn only_a_run_still_waiting_on_its_opening_is_abandoned() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        assert_eq!(registry.take_admissions().len(), 1);
        let session = SessionId::new();
        registry.bind_session(run, session);
        let stuck = |registry: &AgentRegistry, session| {
            opening_never_took(&registry.inner.lock().runs[&run], session)
        };

        assert!(stuck(&registry, session), "Starting, holding its slot");
        // `SessionStart` at a composer nothing submits: the selfcraft stall.
        assert!(registry.set_state(None, run, RunState::Idle));
        assert!(
            stuck(&registry, session),
            "Idle from Starting releases nothing"
        );
        assert!(!stuck(&registry, SessionId::new()), "another child's timer");

        // The opening did land, just late: the turn and its hand-back take it out for good.
        assert!(registry.set_state(None, run, RunState::Running));
        assert!(!stuck(&registry, session), "a turn is in progress");
        assert!(registry.set_state(None, run, RunState::Idle));
        assert!(!stuck(&registry, session), "a turn was handed back");
    }

    /// A lost opening puts the run back at the **front** of its queue with its slot released and
    /// its session unbound, carrying the fork and prompt to start again with — and after
    /// `OPENING_REQUEUES` of those, gives up rather than looping.
    #[test]
    fn a_lost_opening_goes_back_to_the_front_of_the_queue() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        let behind = registry.enqueue(spec(project, "developer", 1, 4));
        assert_eq!(registry.take_admissions().len(), 1);
        let session = SessionId::new();
        registry.bind_session(run, session);
        assert!(registry.set_state(None, run, RunState::Idle));

        let requeue = || Requeued {
            resume: Some(ResumePoint {
                rebind: None,
                conversation: session.to_string(),
            }),
            prompt: "Work on task t-1".into(),
        };
        let outcome = requeue_unsubmitted(&mut registry.inner.lock(), run, requeue());
        assert_eq!(outcome, OpeningOutcome::Requeued(1));
        {
            let inner = registry.inner.lock();
            let live = &inner.runs[&run];
            assert_eq!(live.state, RunState::Queued);
            assert_eq!(
                live.session, None,
                "the dying child's exit must find no run"
            );
            assert!(!live.slot);
        }

        // The slot is free and the lost run is first in line — ahead of the one that queued
        // behind it while it sat — and it is forked from the same conversation, same prompt.
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, run);
        assert_eq!(admitted[0].prompt, "Work on task t-1");
        let point = admitted[0]
            .resume
            .as_ref()
            .expect("the conversation is continued");
        assert_eq!(
            point.rebind, None,
            "a fork under a fresh session, never the old id"
        );
        assert_eq!(point.conversation, session.to_string());
        assert_eq!(state_of(&registry, behind), RunState::Queued);

        // Once more, then the bound: the third loss is a failure for the caller to report.
        assert_eq!(
            requeue_unsubmitted(&mut registry.inner.lock(), run, requeue()),
            OpeningOutcome::Requeued(2)
        );
        assert_eq!(
            requeue_unsubmitted(&mut registry.inner.lock(), run, requeue()),
            OpeningOutcome::GiveUp
        );
    }

    /// When an opening prompt's retry stops pressing Enter. The two states it must keep pressing
    /// through are the selfcraft stall itself: `Idle` is `SessionStart` at a composer the paste
    /// filled, and `Spawning` is a TUI whose hooks have not reported in yet — and `Idle` is also
    /// the state a quick turn ends in, which is why `AwaitingInput`, the one only a `Busy` can
    /// reach, is what proves a turn that finished between two polls.
    #[test]
    fn an_opening_is_settled_only_by_evidence_of_a_turn() {
        for pressing_on in [
            None,
            Some(SessionState::Spawning),
            Some(SessionState::Splash),
            Some(SessionState::Idle),
        ] {
            assert!(!opening_settled(pressing_on), "{pressing_on:?}");
        }
        for settled in [
            SessionState::Busy,
            SessionState::AwaitingInput,
            SessionState::AwaitingPermission,
            SessionState::Exited { code: 0 },
            SessionState::Paused,
        ] {
            assert!(opening_settled(Some(settled)), "{settled:?}");
        }
    }

    /// A role the file sends to one CLI and a local override sends to another is **read** by the
    /// one it was forked with. (M78)
    ///
    /// The bug this pins had no error anywhere in it. `DispatchSpec::harness` is the committed
    /// definition's, `RunPlan::harness` is the override's, and `observe` picks its state machine
    /// from the registry — which held the definition's. A project that redirected every role from
    /// `opencode` to `codex` therefore forked codex children and read them with opencode's
    /// machine: codex's `turn.started` matches none of opencode's five event names, every line
    /// answered `None`, and the run sat in `RunState::Starting` from the fork to the exit while
    /// doing all of its work. The panel draws `starting` as `circle-dashed` — a deliberately
    /// still mark — so what a person saw was a spinner that never span, in one project and not
    /// another.
    ///
    /// Both halves are asserted, because only the pair says where the fault was: the same line,
    /// against the same run, answers nothing before the stamp and `Running` after it.
    /// A line typed into a codex TUI is one paste, wrapped once. (M93)
    #[test]
    fn a_codex_line_is_pasted_once() {
        assert_eq!(
            as_paste(b"a\nb".to_vec()),
            b"\x1b[200~a\nb\x1b[201~".to_vec()
        );
        let wrapped = as_paste(b"x".to_vec());
        assert_eq!(as_paste(wrapped.clone()), wrapped);
    }

    #[test]
    fn a_redirected_run_is_read_by_the_harness_it_was_forked_with() {
        // Since M93 a codex child reports through hooks, as a claude one does: the frame below
        // is the shape its `UserPromptSubmit` hook sends.
        let prompt = cide_claude::HookFrame::new(
            "UserPromptSubmit",
            serde_json::json!({"session_id": "01a0c54b-332d-7461-bdeb-648f2b2f9c10"}),
        );

        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let mut spec = spec(project, "gameplay-dev", 4, 4);
        // What the committed `.cide/agents/gameplay-dev.md` says.
        spec.harness = Harness::Opencode;
        let run = registry.enqueue(spec);
        let mut admitted = Vec::new();
        admit_a_pass(&mut registry.inner.lock(), &mut admitted, &mut Vec::new());
        let session = SessionId::new();
        registry.bind_session(run, session);
        assert_eq!(state_of(&registry, run), RunState::Starting);

        // The registry still holds the file's answer: opencode's machine, which reads no hooks.
        assert_eq!(
            registry.observe(None, session, Observation::Hook(&prompt)),
            None,
        );
        assert_eq!(state_of(&registry, run), RunState::Starting);

        // What `start_child` now does once the override has been resolved.
        registry.note_harness(run, Harness::Codex);
        assert_eq!(
            registry.observe(None, session, Observation::Hook(&prompt)),
            Some((run, RunState::Running)),
        );
        assert_eq!(state_of(&registry, run), RunState::Running);
    }

    /// **And something does the stamping.** (M78)
    ///
    /// The structural half, and the only assertion that could have caught the original defect.
    /// The test above drives `note_harness` by hand, so it goes on passing against a
    /// `start_child` that never calls it — which is precisely the state this crate was in, and
    /// is `the_workspace_flusher_is_started_from_setup`'s lesson arriving in a second place: a
    /// value computed for nobody passes every behavioural test you can write about it.
    ///
    /// Comments stripped first, because the paragraph beside the call site spells the name —
    /// **and the test module cut off before that**, because this assertion lives in the very
    /// file it greps and its own needle is a string literal in it. `without_comments` does not
    /// strip a literal, so the unsliced version passed against a `start_child` with the call
    /// deleted: it was matching itself. That is `check:diff-render`'s trap and `check:docker`'s,
    /// twice in one sitting, arriving in Rust — and `the_workspace_flusher_is_started_from_setup`
    /// only escapes it by greping a different file than the one it lives in.
    #[test]
    fn the_resolved_harness_is_stamped_on_the_run_at_the_fork() {
        let whole = crate::srcgrep::without_comments(include_str!("agents.rs"));
        // The **module**, not the bare attribute: `#[cfg(test)]` also sits on a test-only
        // helper far above the call site, and cutting at the first one hid the very line this
        // is here to find. `find` and not `rfind`, so the identical literal a few lines below
        // cannot move the cut past the module and let this match itself again.
        let cut = whole
            .find("#[cfg(test)]\nmod tests {")
            .expect("this file ends in a test module");
        let source = &whole[..cut];
        assert!(
            source.contains("registry.note_harness(admission.run, resolved.harness)"),
            "nothing writes the resolved harness onto the run, so `observe` reads a redirected \
             child's output with the state machine of the CLI its definition names and the run \
             never leaves `Starting`"
        );
    }

    /// A run nothing redirected keeps the harness it was dispatched with, and the stamp is a
    /// no-op rather than a second write — `note_harness` is called on **every** fork.
    #[test]
    fn a_run_nobody_redirected_is_left_exactly_as_it_was() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let mut spec = spec(project, "reviewer", 4, 4);
        spec.harness = Harness::Opencode;
        let run = registry.enqueue(spec);
        registry.note_harness(run, Harness::Opencode);
        assert_eq!(
            registry
                .inner
                .lock()
                .runs
                .get(&run)
                .expect("the run")
                .harness,
            Harness::Opencode,
        );
    }

    /// **One worktree per agent means one run at a time.**
    ///
    /// The property the whole queue exists for: three dispatches against one role admit exactly
    /// one, and the other two wait rather than starting a second `claude` in the same checkout.
    #[test]
    fn an_agent_admits_one_run_and_queues_the_rest() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        // A role whose `max-concurrent` is 1 — the numeric ceiling, enforced as stamped.
        let runs: Vec<RunId> = (0..3)
            .map(|_| registry.enqueue(spec(project, "developer", 1, 4)))
            .collect();

        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1, "the role's ceiling is one");
        assert_eq!(admitted[0].run, runs[0], "the queue is first in, first out");
        assert_eq!(state_of(&registry, runs[0]), RunState::Starting);
        assert_eq!(state_of(&registry, runs[1]), RunState::Queued);
        assert_eq!(state_of(&registry, runs[2]), RunState::Queued);

        // And asking again while it is still starting admits nothing: the slot is held from
        // admission, not derived from the phase.
        assert!(registry.take_admissions().is_empty());
    }

    /// **Two tasks of one role run in parallel; one checkout never holds two children.**
    ///
    /// The per-task successor to the worktree clamp, driven end to end through admission. The
    /// user's report that motivated it: a role with `max-concurrent: 2` and two assigned tasks
    /// ran one and queued the other, because the role had one checkout and the limit was
    /// clamped to protect it. Now the checkout is per task and the limit means what it says —
    /// but the *gate* has to hold everything that would still collide: a second dispatch into
    /// the same checkout, which since M40 means the same task twice (a run with no task takes
    /// no checkout at all — `spec()`'s `checkout: None` — and `a_role_with_room_for_three`
    /// is the test that such runs are held by nothing but the limits).
    #[test]
    fn two_tasks_parallelise_and_one_checkout_serialises() {
        let with_checkout = |project, name: Option<&str>| DispatchSpec {
            checkout: name.map(str::to_string),
            ..spec(project, "developer", 2, 8)
        };

        // Different tasks, different checkouts: the declared 2 is real.
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(with_checkout(project, Some("developer-t-1")));
        let second = registry.enqueue(with_checkout(project, Some("developer-t-2")));
        let admitted = registry.take_admissions();
        assert_eq!(
            admitted.len(),
            2,
            "two tasks are two checkouts and both slots are free"
        );
        assert_eq!(state_of(&registry, first), RunState::Starting);
        assert_eq!(state_of(&registry, second), RunState::Starting);

        // The same checkout, twice — one task re-dispatched while its first run is still on
        // it: the second waits for the first to *end*, not merely to start — a
        // `Starting`/`Running` child may be standing in that directory.
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(with_checkout(project, Some("developer-t-1")));
        let second = registry.enqueue(with_checkout(project, Some("developer-t-1")));
        assert_eq!(registry.take_admissions().len(), 1);
        assert_eq!(
            state_of(&registry, second),
            RunState::Queued,
            "two dispatches of one task share its checkout and must serialise"
        );
        registry.set_state(None, first, RunState::Running);
        assert!(
            registry.take_admissions().is_empty(),
            "a running child in the checkout still blocks it"
        );
        registry.set_state(None, first, RunState::Finished { code: 0 });
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, second, "the checkout freed with the exit");

        // `Idle` deliberately does not block: the parked child is `bring_up`'s to wind down at
        // the moment the checkout is claimed, and holding the queue on it would leave an idle
        // claude pinning its task's checkout for ever.
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(with_checkout(project, Some("developer")));
        let second = registry.enqueue(with_checkout(project, Some("developer")));
        registry.take_admissions();
        registry.set_state(None, first, RunState::Idle);
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1, "idle releases the checkout to the queue");
        assert_eq!(admitted[0].run, second);

        // `None` — shared isolation's stamp — collides with nothing: N runs in one directory
        // is what that setting says on its face.
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        registry.enqueue(with_checkout(project, None));
        registry.enqueue(with_checkout(project, None));
        assert_eq!(registry.take_admissions().len(), 2);
    }

    /// **The teardown's snapshot is the last word.** The first live restart found the race:
    /// teardown wrote the truth, the ladder killed the children, the reaper marked the runs
    /// `Finished`, and the coalescer's next flush — 120 ms, well inside the ladder's graces —
    /// overwrote the file with the shutdown's own kills recorded as outcomes. Two paused runs
    /// restored as history, and Resume had nothing to resume. The seal is the fix, and this
    /// drives the exact sequence.
    /// **The time cide spent shut is not work, and the work before it is not lost.**
    ///
    /// Two halves of one claim, and they pull in opposite directions — which is why they are
    /// asserted together. The snapshot must *close* the interval a running run had open, or
    /// everything it did since its last state change is thrown away at every restart; and the
    /// restore must *not* reopen one, or the hours between the two launches land in the figure.
    /// Both fall out of the shape rather than out of code: `worked_now` closes, and a restored
    /// run is `Interrupted`, which is not work.
    #[test]
    fn a_restart_keeps_the_work_and_drops_the_hours_cide_was_shut() {
        let dir = std::env::temp_dir().join(format!("cide-run-worked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("agent-runs.json");

        let registry = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        registry.bind_session(run, SessionId::new());
        assert!(registry.set_state(None, run, RunState::Running));

        // Wind the open interval back an hour, so the snapshot has something to close that the
        // test can name. (The stamp is wall clock — `admit_a_pass` and `set_state` read the
        // real one — so it is moved rather than injected.)
        const HOUR: u64 = 3_600_000;
        {
            let mut live = clocked(&registry, run);
            live.worked_ms = 0;
            live.working_since_unix_ms = Some(now_unix_ms() - HOUR);
        }
        registry.write_snapshot_now(&path);

        let after = Arc::new(AgentRegistry::default());
        after.restore_snapshot_from(&path, |_| true, |_| None, |_, _, _| false);
        assert_eq!(state_of(&after, run), RunState::Interrupted);
        let live = clocked(&after, run);
        assert!(
            live.worked_ms.abs_diff(HOUR) < 5_000,
            "the snapshot must close the open interval, or a run loses everything it did since \
             its last state change at every restart: {}",
            live.worked_ms
        );
        assert_eq!(
            live.working_since_unix_ms, None,
            "a restored run is `Interrupted`, which is not work — this is what excludes the \
             hours cide spent shut, with no snapshot timestamp and no code of its own"
        );
        // And the figure does not move, however long the process has been up since.
        let banked = live.worked_ms;
        drop(live);
        assert_eq!(
            worked_now(&clocked(&after, run), now_unix_ms() + 10 * HOUR),
            banked
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A snapshot written before the worked clock existed still loads.**
    ///
    /// The schema rung. `#[serde(default)]` reads a missing `workedMs` as zero, which
    /// understates those runs' figures exactly once and is the only honest answer available —
    /// nothing on disk records what they did. What must not happen is the whole file failing to
    /// parse, which would lose every run in it.
    #[test]
    fn a_snapshot_written_before_the_worked_clock_still_loads() {
        let dir = std::env::temp_dir().join(format!("cide-run-rung-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("agent-runs.json");

        let registry = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        registry.bind_session(run, SessionId::new());
        registry.write_snapshot_now(&path);

        // The field, struck out of the file exactly as a build without it would have written.
        let text = std::fs::read_to_string(&path).expect("written");
        assert!(text.contains("workedMs"), "the field is in a fresh file");
        let old: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("\"workedMs\""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!old.contains("workedMs"));
        std::fs::write(&path, &old).expect("rewrite");

        let after = Arc::new(AgentRegistry::default());
        after.restore_snapshot_from(&path, |_| true, |_| None, |_, _, _| false);
        assert_eq!(
            state_of(&after, run),
            RunState::Interrupted,
            "the run came back at all — the failure this guards is the whole file refusing"
        );
        assert_eq!(clocked(&after, run).worked_ms, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sealed_snapshot_ignores_the_shutdowns_own_kills() {
        let dir = std::env::temp_dir().join(format!("cide-run-seal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("agent-runs.json");

        let registry = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        registry.bind_session(run, SessionId::new());
        registry.inner.lock().paused_projects.insert(project);

        // The teardown: seal, then write. (`final_snapshot` is exactly this plus the screens,
        // aimed at the real path; the seal and the write are what the race is about.)
        assert!(
            !registry
                .snapshot_sealed
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        );
        registry.write_snapshot_now(&path);
        let sealed = std::fs::read(&path).expect("the teardown wrote");

        // The ladder kills the child, the reaper reports, the coalescer flushes once more.
        registry.set_state(None, run, RunState::Finished { code: 143 });
        registry.write_snapshot_to(&path);
        assert_eq!(
            std::fs::read(&path).expect("still there"),
            sealed,
            "a post-seal flush rewrote the snapshot with the shutdown's kills as outcomes"
        );

        // And the restart reads the sealed truth: resumable, not history.
        let after = Arc::new(AgentRegistry::default());
        after.restore_snapshot_from(&path, |_| true, |_| None, |_, _, _| false);
        assert_eq!(state_of(&after, run), RunState::Interrupted);
        assert!(!after.dispatching(project), "the halt survived too");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A run outlives the process, and Resume continues it — through the queue.**
    ///
    /// The whole restart story in one test: a working opencode run is snapshotted, restored
    /// into a fresh registry as `Interrupted`, requeued by Resume, and admitted carrying its
    /// conversation and a continuation prompt — while a run that was still *queued* at quit
    /// comes back with no conversation and its original prompt, because nothing had happened
    /// yet and claiming otherwise would bill a fresh start as a continuation.
    #[test]
    fn an_interrupted_run_resumes_through_the_queue_continuing_its_conversation() {
        let dir = std::env::temp_dir().join(format!("cide-run-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("agent-runs.json");

        let before = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let worked = before.enqueue(opencode_spec(project, "developer"));
        let waiting = before.enqueue(opencode_spec(project, "qa"));
        let admitted = before.take_admissions();
        assert_eq!(admitted.len(), 2, "two roles, two slots");
        let session = SessionId::new();
        before.bind_session(worked, session);
        assert!(before.note_harness_session(worked, "ses_fe6da2c1effe8Yz".into()));
        // `waiting` was admitted too; push it back to Queued to model a run the quit caught
        // before its child existed. (Its slot bookkeeping dies with the process either way.)
        before
            .inner
            .lock()
            .runs
            .get_mut(&waiting)
            .expect("row")
            .state = RunState::Queued;
        // The workstream was halted before the quit, and that must come back too.
        before.inner.lock().paused_projects.insert(project);

        before.write_snapshot_to(&path);

        // A run that ENDED before the quit is history, and history restores as history —
        // with its session dropped, so no Open can aim at a process the old cide took with it.
        let ended = before.enqueue(opencode_spec(project, "artist"));
        {
            let mut inner = before.inner.lock();
            let live = inner.runs.get_mut(&ended).expect("row");
            live.state = RunState::Finished { code: 0 };
            live.session = Some(SessionId::new());
        }
        before.write_snapshot_to(&path);

        // The restart: a fresh registry, the workspace still holding the project.
        let after = Arc::new(AgentRegistry::default());
        after.restore_snapshot_from(&path, |_| true, |_| None, |_, _, _| false);
        assert_eq!(state_of(&after, worked), RunState::Interrupted);
        assert_eq!(state_of(&after, waiting), RunState::Interrupted);
        assert_eq!(state_of(&after, ended), RunState::Finished { code: 0 });
        assert_eq!(
            after.inner.lock().runs[&ended].session,
            None,
            "a restored history row must not carry a session nothing holds"
        );
        assert!(
            !after.dispatching(project),
            "a halted workstream restarted itself"
        );

        // Resume's two halves, exercised app-free: reopen the queue, requeue the interrupted.
        after.inner.lock().paused_projects.remove(&project);
        after
            .requeue_interrupted(project, None, None, &|_| None)
            .expect("nobody is viewing anything in a test without a session registry");
        let mut admissions = after.take_admissions();
        admissions.sort_by_key(|admission| admission.run != worked);
        assert_eq!(admissions.len(), 2);

        let continued = &admissions[0];
        let point = continued
            .resume
            .as_ref()
            .expect("a conversation to continue");
        assert_eq!(point.conversation, "ses_fe6da2c1effe8Yz");
        assert_eq!(
            point.rebind, None,
            "opencode's new child is a new cide session"
        );
        assert!(
            continued.prompt.contains("cide restarted"),
            "{}",
            continued.prompt
        );

        let fresh = &admissions[1];
        assert!(
            fresh.resume.is_none(),
            "nothing had happened; nothing to continue"
        );
        assert_eq!(
            fresh.prompt, "do the thing",
            "the original dispatch, replayed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The claude flavour of the same continuation: the run's own [`SessionId`] is both the
    /// rebind and the conversation, which is what keeps hook routing and the row continuous.
    #[test]
    fn a_claude_runs_continuation_rebinds_its_own_session() {
        let dir = std::env::temp_dir().join(format!("cide-run-snapclaude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("agent-runs.json");

        let before = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let run = before.enqueue(spec(project, "developer", 1, 4));
        before.take_admissions();
        let session = SessionId::new();
        before.bind_session(run, session);
        before.write_snapshot_to(&path);

        let after = Arc::new(AgentRegistry::default());
        after.restore_snapshot_from(&path, |_| true, |_| None, |_, _, _| false);
        after
            .requeue_interrupted(project, Some(run), None, &|_| None)
            .expect("nobody is viewing anything in a test without a session registry");
        let admissions = after.take_admissions();
        assert_eq!(admissions.len(), 1);
        let point = admissions[0].resume.as_ref().expect("resumable");
        assert_eq!(point.rebind, Some(session));
        assert_eq!(point.conversation, session.to_string());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The duplicate rule's truth table.** (M66) A pair is held by a run that has a child or
    /// is about to get one, and by nothing else.
    ///
    /// `Interrupted` is the row that changed, and it is the reason this test is not called
    /// `an_open_run_is_any_run_that_has_not_ended` any more: a run whose child died with a
    /// previous cide is not running, so it doubles nothing — and while it counted, a row left
    /// behind by a restart silently swallowed every re-assignment of that role to that task.
    /// `holds_a_pair` is an exhaustive match so that a state added later is classified by
    /// whoever adds it; this walks all nine so the classification is also *asserted* somewhere a
    /// reader can see the whole set at once.
    #[test]
    fn an_interrupted_run_does_not_hold_its_pair_but_every_live_state_does() {
        let holds: &[(RunState, bool)] = &[
            (RunState::Queued, true),
            (RunState::Starting, true),
            (RunState::Running, true),
            (RunState::AwaitingPermission, true),
            // The child lives and holds the role's only worktree; the way to re-engage it is a
            // retry or a prompt, not a second run queued behind it.
            (RunState::Idle, true),
            // A frozen child has not let go of the checkout either.
            (RunState::Paused { since_unix_ms: 1 }, true),
            (RunState::Interrupted, false),
            (RunState::Finished { code: 0 }, false),
            (
                RunState::Failed {
                    reason: "spawn failed".into(),
                },
                false,
            ),
        ];
        for (state, expected) in holds {
            assert_eq!(
                holds_a_pair(state),
                *expected,
                "{state:?} must {}hold its pair",
                if *expected { "" } else { "not " }
            );
        }

        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let task = TaskId("t-7".into());
        let mut with_task = spec(project, "developer", 1, 4);
        with_task.task = Some(task.clone());
        let run = registry.enqueue(with_task);
        let agent = AgentId("developer".into());

        // Queued holds: the whole point is not stacking a second entry behind it. And the
        // answer carries the state, because the refusal's sentence is spelled from it.
        let held = registry
            .run_holding(project, &agent, &task)
            .expect("queued");
        assert_eq!(held.run, run);
        assert_eq!(held.state, RunState::Queued);
        // A different task, and a different role, hold nothing.
        assert!(
            registry
                .run_holding(project, &agent, &TaskId("t-8".into()))
                .is_none()
        );
        assert!(
            registry
                .run_holding(project, &AgentId("qa".into()), &task)
                .is_none()
        );

        let _ = registry.take_admissions();
        registry.set_state(None, run, RunState::Idle);
        let held = registry.run_holding(project, &agent, &task).expect("idle");
        assert_eq!(
            held.state,
            RunState::Idle,
            "the word the refusal will print"
        );

        // An interrupted row is history, not a holder — this is the M66 change, through the
        // public road as well as the table above.
        registry.set_state(None, run, RunState::Interrupted);
        assert!(registry.run_holding(project, &agent, &task).is_none());

        // Failed closes it too: the pair may be dispatched again.
        registry.fail(None, run, "spawn failed");
        assert!(registry.run_holding(project, &agent, &task).is_none());
    }

    /// **The race, at the only seam that can win it.** (M66)
    ///
    /// No `plan_dispatch` anywhere: this is the atomic door on its own, which is what
    /// `task_triggers`' pre-check cannot be because it spawns between asking and enqueueing.
    #[test]
    fn a_second_run_for_the_same_role_and_task_is_refused_under_the_lock_that_mints_the_id() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let task = TaskId("t-904".into());
        let agent = AgentId("developer".into());

        let mut first = spec(project, "developer", 8, 8);
        first.task = Some(task.clone());
        let run = registry
            .enqueue_unique(first)
            .expect("the first one starts");

        let mut second = spec(project, "developer", 8, 8);
        second.task = Some(task.clone());
        let held = registry
            .enqueue_unique(second)
            .expect_err("the second one is refused");
        assert_eq!(held.run, run, "the refusal names the run already going");
        assert_eq!(held.state, RunState::Queued);

        // And it is refused by *not being there*, which is the only assertion that matters: a
        // guard that answered Err and inserted anyway would pass every other check here.
        assert_eq!(registry.runs_for(project).len(), 1);

        // A different role on the same task is not a duplicate — a mention starting `qa`
        // alongside `developer` is the documented semantics, not an accident.
        let mut other = spec(project, "qa", 8, 8);
        other.task = Some(task.clone());
        assert!(registry.enqueue_unique(other).is_ok());
        assert!(registry.run_holding(project, &agent, &task).is_some());
    }

    /// The guard is not a permanent ban: an ended run frees its pair.
    #[test]
    fn a_run_that_ended_frees_its_pair_for_a_fresh_dispatch() {
        for close in [0u8, 1] {
            let registry = AgentRegistry::default();
            let project = ProjectId::new();
            let task = TaskId("t-7".into());
            let mut first = spec(project, "developer", 8, 8);
            first.task = Some(task.clone());
            let run = registry.enqueue_unique(first).expect("first");
            let _ = registry.take_admissions();
            if close == 0 {
                registry.set_state(None, run, RunState::Finished { code: 0 });
            } else {
                registry.fail(None, run, "spawn failed");
            }

            let mut again = spec(project, "developer", 8, 8);
            again.task = Some(task);
            assert!(
                registry.enqueue_unique(again).is_ok(),
                "a finished run must not wedge its task"
            );
        }
    }

    /// M40's road is untouched: a dispatch with no task contends for nothing.
    ///
    /// Such a run stands in the project root with no worktree, so N of them is what that road
    /// says on its face — and a guard that keyed on the role alone would have quietly closed it.
    #[test]
    fn two_runs_with_no_task_are_never_duplicates_of_each_other() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        assert!(
            registry
                .enqueue_unique(spec(project, "developer", 8, 8))
                .is_ok()
        );
        assert!(
            registry
                .enqueue_unique(spec(project, "developer", 8, 8))
                .is_ok()
        );
        assert_eq!(registry.runs_for(project).len(), 2);
    }

    /// **The duplicate was destructive, not merely wasteful** — the regression this guards. (M66)
    ///
    /// `Idle` does not hold the *checkout* gate (see `admit_a_pass`), deliberately, so before
    /// M66 a second dispatch of the same pair was admitted *beside* an idle first run and
    /// `start_child` then killed that idle child to reclaim the directory. The second run did
    /// not queue behind the first; it ended it.
    #[test]
    fn an_idle_first_run_is_not_displaced_by_a_second_dispatch_of_its_pair() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let task = TaskId("t-904".into());
        let agent = AgentId("developer".into());

        let mut first = spec(project, "developer", 8, 8);
        first.task = Some(task.clone());
        first.checkout = Some("developer-t-904".into());
        let run = registry.enqueue_unique(first).expect("first");
        let _ = registry.take_admissions();
        registry.set_state(None, run, RunState::Idle);

        let mut second = spec(project, "developer", 8, 8);
        second.task = Some(task.clone());
        second.checkout = Some("developer-t-904".into());
        assert!(registry.enqueue_unique(second).is_err());

        // Nothing was admitted, so nothing reached `idle_children_in`, so the first run's child
        // is still the first run's child.
        assert!(registry.take_admissions().is_empty());
        assert_eq!(state_of(&registry, run), RunState::Idle);
        assert_eq!(registry.runs_for(project).len(), 1);
        assert!(registry.run_holding(project, &agent, &task).is_some());
    }

    /// Resume does not recreate the duplicate that excluding `Interrupted` made possible. (M66)
    ///
    /// The whole of that exclusion's cost, in one place: because an interrupted row no longer
    /// holds its pair, the user can start the role on that task again — and a Resume that then
    /// requeued the old row would put the second run on the board that M66 exists to keep off
    /// it. The live run is the one that is going; this row is the conversation the restart
    /// ended, and it says so in its note rather than silently doing nothing.
    #[test]
    fn a_resume_does_not_requeue_an_interrupted_run_whose_pair_is_already_going() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let task = TaskId("t-904".into());

        let mut old = spec(project, "developer", 8, 8);
        old.task = Some(task.clone());
        let stale = registry.enqueue(old);
        registry.set_state(None, stale, RunState::Interrupted);

        // Allowed, and that is the point of the exclusion.
        let mut fresh_spec = spec(project, "developer", 8, 8);
        fresh_spec.task = Some(task.clone());
        let fresh = registry
            .enqueue_unique(fresh_spec)
            .expect("an interrupted row holds nothing");

        registry
            .requeue_interrupted(project, None, None, &|_| None)
            .expect("the project scope skips rather than refuses");
        assert_eq!(
            state_of(&registry, stale),
            RunState::Interrupted,
            "the old row was requeued beside a live run of its own pair"
        );
        let note = registry.inner.lock().runs[&stale]
            .note
            .clone()
            .expect("a skip without a sentence is a button that does nothing");
        assert!(note.contains(&fresh.to_string()), "{note}");
        assert!(note.contains("two runs on one task"), "{note}");

        // The single-run press gets the same fact as a refusal, like the viewed-in-a-pane arm.
        let why = registry
            .requeue_interrupted(project, Some(stale), None, &|_| None)
            .expect_err("resuming one run says why it cannot")
            .to_string();
        assert!(why.contains("already going on this task"), "{why}");

        // And once the live one ends, the old row resumes as it always did.
        registry.set_state(None, fresh, RunState::Finished { code: 0 });
        registry
            .requeue_interrupted(project, Some(stale), None, &|_| None)
            .expect("nothing holds the pair now");
        assert_eq!(state_of(&registry, stale), RunState::Queued);
    }

    /// **P0's truth table**: the registry owns a session for as long as its run has not ended.
    ///
    /// `session_kill` consults this before killing anything, because the debug report's
    /// single-pane deaths (exit 129, one sibling dead while the other lived) were pane closes
    /// reaching a run's PTY. Interrupted used to count as owned, "harmlessly": no registry
    /// child stands behind that state. Since M42 a **pane's** child can — a person re-opening
    /// the interrupted run's conversation under its own session id — and that pane must be
    /// able to end what it started, so Interrupted answers false; the transcript a later
    /// Resume needs is on disk, not in the process, and a kill there destroys nothing.
    #[test]
    fn the_registry_owns_a_session_until_its_run_ends() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        let session = SessionId::new();

        // Enqueued but unbound: no session, nothing owned.
        assert!(!registry.owns_session(session));

        registry.bind_session(run, session);
        assert!(registry.owns_session(session));
        // A session the registry never bound is not owned, whatever else is running.
        assert!(!registry.owns_session(SessionId::new()));

        let _ = registry.take_admissions();
        for state in [RunState::Running, RunState::Idle] {
            registry.set_state(None, run, state);
            assert!(
                registry.owns_session(session),
                "a not-ended run owns its session"
            );
        }
        // Interrupted: no registry child, possibly a pane's — the pane may close it (M42).
        registry.set_state(None, run, RunState::Interrupted);
        assert!(
            !registry.owns_session(session),
            "an interrupted run's session belongs to whichever pane re-opened it"
        );
        registry.set_state(None, run, RunState::Running);
        assert!(registry.owns_session(session));

        // Finished releases it — the child is gone, the pane close may reap the PTY.
        registry.set_state(None, run, RunState::Finished { code: 0 });
        assert!(!registry.owns_session(session));

        // Failed likewise.
        let other = registry.enqueue(spec(project, "qa", 1, 4));
        let other_session = SessionId::new();
        registry.bind_session(other, other_session);
        registry.fail(None, other, "spawn failed");
        assert!(!registry.owns_session(other_session));
    }

    /// **P2's decision table**: an abnormal end with a task and no successor speaks, once.
    ///
    /// The consumer (`agent_rpc::note_death`) only formats what this hands back, so the whole
    /// rule is testable here with no store and no app: exit 0 is silent, an exit code is
    /// carried, `Failed` carries none, a surviving sibling on the pair silences the death for
    /// good, and the latch makes every answer single-shot.
    #[test]
    fn a_death_speaks_once_and_only_with_no_successor() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let task = TaskId("t-9".into());
        let with_task = |agent: &str| {
            let mut spec = spec(project, agent, 2, 8);
            spec.task = Some(task.clone());
            spec
        };

        // Exit 0 is a run that believes it finished; second-guessing it is the orchestrator's
        // review, not an automatic epitaph.
        let clean = registry.enqueue(with_task("developer"));
        let _ = registry.take_admissions();
        registry.set_state(None, clean, RunState::Finished { code: 0 });
        assert!(registry.death_facts(clean).is_none());

        // Two runs of one role on one task: the first to die is silenced by the survivor,
        // and stays silent after the survivor is gone — the survivor's own end speaks.
        let first = registry.enqueue(with_task("developer"));
        let second = registry.enqueue(with_task("developer"));
        let _ = registry.take_admissions();
        registry.set_state(None, first, RunState::Finished { code: 129 });
        assert!(
            registry.death_facts(first).is_none(),
            "the successor owns the story now"
        );
        registry.fail(None, second, "spawn failed");
        assert!(
            registry.death_facts(first).is_none(),
            "the latch outlives the successor"
        );
        let facts = registry
            .death_facts(second)
            .expect("the last run on the task speaks");
        assert_eq!(facts.task, task);
        assert_eq!(facts.agent_label, "developer");
        assert_eq!(facts.code, None, "Failed has no exit code to print");
        assert!(registry.death_facts(second).is_none(), "once");

        // A nonzero exit carries its number — the report's 129 is the sentence's whole point.
        let crashed = registry.enqueue(with_task("qa"));
        let _ = registry.take_admissions();
        registry.set_state(None, crashed, RunState::Finished { code: 129 });
        let facts = registry
            .death_facts(crashed)
            .expect("no successor, so it speaks");
        assert_eq!(facts.code, Some(129));

        // No task, nothing to write on.
        let bare = registry.enqueue(spec(project, "qa", 2, 8));
        let _ = registry.take_admissions();
        registry.fail(None, bare, "spawn failed");
        assert!(registry.death_facts(bare).is_none());
    }

    /// **P1's cap**: a run log stops at its cap with one notice, and never grows past it.
    ///
    /// The cap exists because the log tees every raw line of a run that may loop for hours;
    /// the notice exists because a silently truncated log reads as "the run stopped here",
    /// which is the exact confusion the log was added to end.
    #[test]
    fn a_run_log_caps_with_one_notice_and_goes_quiet() {
        let dir = std::env::temp_dir().join(format!("cide-run-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("r-test.log");
        let mut log = RunLog::at(path.clone(), 64);

        log.append("first line");
        log.append("second line");
        let mid = std::fs::read_to_string(&path).expect("the log was created lazily");
        assert_eq!(mid, "first line\nsecond line\n");

        // Push past the cap: the breaching line is dropped, the notice takes its place,
        // and everything after it is silence.
        log.append("a line long enough to cross the sixty-four byte cap set above");
        log.append("this line must not be recorded");
        log.append("nor this one");
        let full = std::fs::read_to_string(&path).expect("the log survives the cap");
        assert!(
            full.ends_with(&format!("{RUN_LOG_CAPPED}\n")),
            "one notice, at the end: {full:?}"
        );
        assert_eq!(
            full.matches(RUN_LOG_CAPPED).count(),
            1,
            "the notice appears once"
        );
        assert!(!full.contains("must not be recorded"));

        // A fresh RunLog at the same path (a respawn) seeds `written` from the file and
        // stays quiet too — the cap covers the run's whole life, not one child's.
        let mut again = RunLog::at(path.clone(), 64);
        again.append("a respawned child's line");
        let after = std::fs::read_to_string(&path).expect("still there");
        assert_eq!(
            after.matches(RUN_LOG_CAPPED).count(),
            2,
            "the respawn breaches once more at most"
        );
        assert!(!after.contains("respawned child"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A role allowed several slots gets them in one pass, not one per unrelated event.
    ///
    /// The runs here carry no checkout — shared isolation, or (M40) dispatches with no task,
    /// which stand in the project root and are held by nothing but the limits — and the bug it
    /// pins is a single-pass scan: only the front of an agent's queue is eligible, so one pass
    /// starts one of three and leaves the other two waiting for something else to pump.
    #[test]
    fn a_role_with_room_for_three_starts_three() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        for _ in 0..4 {
            registry.enqueue(spec(project, "developer", 3, 8));
        }
        assert_eq!(registry.take_admissions().len(), 3);
        assert!(registry.take_admissions().is_empty());
    }

    /// The project's own ceiling, which is a different limit from the role's.
    #[test]
    fn the_projects_ceiling_holds_across_roles() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(spec(project, "developer", 1, 1));
        let second = registry.enqueue(spec(project, "qa", 1, 1));

        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1, "maxConcurrent is a project-wide number");
        assert_eq!(admitted[0].run, first);
        assert_eq!(state_of(&registry, second), RunState::Queued);
    }

    /// **The `SessionStart` window, closed.**
    ///
    /// `cide_claude::next_state` answers `SessionStart` with `Idle`, so every run is briefly idle
    /// between its child reporting in and its opening prompt landing. A slot released on that
    /// would start a second run of the same role into the same worktree — which is precisely why
    /// the release is an *edge* out of `Running`, not a reading of the current phase.
    #[test]
    fn the_idle_a_starting_child_reports_releases_nothing() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(spec(project, "developer", 1, 4));
        let second = registry.enqueue(spec(project, "developer", 1, 4));

        assert_eq!(registry.take_admissions().len(), 1);
        // The child reports in. `Starting -> Idle`, and the opening prompt has not been read yet.
        assert!(registry.set_state(None, first, RunState::Idle));

        assert!(
            registry.take_admissions().is_empty(),
            "a second run started into the worktree the first is still opening in"
        );
        assert_eq!(state_of(&registry, second), RunState::Queued);
    }

    /// A turn handed back frees the role, and frees it for **exactly one** waiting run.
    #[test]
    fn a_turn_handed_back_starts_exactly_one_queued_run() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(spec(project, "developer", 1, 4));
        let second = registry.enqueue(spec(project, "developer", 1, 4));
        let third = registry.enqueue(spec(project, "developer", 1, 4));
        assert_eq!(registry.take_admissions().len(), 1);

        // The real sequence: the child reports in, the opening prompt is submitted, the turn ends.
        assert!(registry.set_state(None, first, RunState::Idle));
        assert!(registry.set_state(None, first, RunState::Running));
        assert!(registry.set_state(None, first, RunState::Idle));

        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1, "one slot freed, one run started");
        assert_eq!(admitted[0].run, second);
        assert_eq!(state_of(&registry, third), RunState::Queued);

        // The release is latched, so a run that goes idle again cannot free a second slot — the
        // failure that would put two children in one checkout with nothing to arbitrate them.
        assert!(registry.set_state(None, first, RunState::Running));
        assert!(registry.set_state(None, first, RunState::Idle));
        assert!(
            registry.take_admissions().is_empty(),
            "the same run released its slot twice"
        );
    }

    /// **The nudge rides the same edge, so an idle run being observed again is not a re-prompt.**
    ///
    /// [`AgentRegistry::set_state`] is where `crate::agent_rpc::note_run_idle` is called from, and
    /// "once per turn" there is a correctness property rather than a performance one: the nudge
    /// types a line into the user's own conversation. An idle run is *observed* over and over — a
    /// statusline frame arrives about once a second for as long as the child lives — and the
    /// whole of what makes those observations free is this method answering `false` to a state
    /// the run is already in. A nudge hung off "the run is idle" instead of "the run just became
    /// idle" would be a prompt per second.
    #[test]
    fn an_idle_run_observed_again_is_not_a_second_turn_ending() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();

        assert!(registry.set_state(None, run, RunState::Running));
        assert!(
            registry.set_state(None, run, RunState::Idle),
            "the edge itself did not register"
        );
        for _ in 0..20 {
            assert!(
                !registry.set_state(None, run, RunState::Idle),
                "an observation of an already-idle run counted as a turn ending"
            );
        }
    }

    /// `Finished` releases the run, and the row survives to say what happened.
    #[test]
    fn a_finished_run_releases_its_slot_and_stays_listed() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(spec(project, "developer", 1, 4));
        let second = registry.enqueue(spec(project, "developer", 1, 4));
        assert_eq!(registry.take_admissions().len(), 1);

        assert!(registry.set_state(None, first, RunState::Running));
        assert!(registry.set_state(None, first, RunState::Finished { code: 0 }));

        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, second);

        // `RunId`'s doc: a run goes on meaning something after its child dies, and the row a user
        // most wants to read is the one that just failed. So it is still on the wire.
        let listed = registry.runs_for(project);
        assert_eq!(listed.len(), 2);
        let finished = listed
            .iter()
            .find(|run| run.run == first)
            .expect("a finished run is still listed");
        assert_eq!(finished.state, RunState::Finished { code: 0 });
        // And it sorts below the live one, which is the order the panel's groups read.
        assert_eq!(listed[0].run, second);
    }

    /// A failed run releases its slot too — otherwise the first failure stalls the queue silently.
    #[test]
    fn a_run_that_never_started_releases_its_slot() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(spec(project, "developer", 1, 4));
        let second = registry.enqueue(spec(project, "developer", 1, 4));
        assert_eq!(registry.take_admissions().len(), 1);

        assert!(registry.fail(None, first, "no worktree: /repo is not a git repository"));
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, second);
    }

    /// Stopping a queued run takes it off its queue rather than leaving a corpse in the order.
    #[test]
    fn a_cancelled_queued_run_does_not_hold_its_place() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(spec(project, "developer", 1, 4));
        let second = registry.enqueue(spec(project, "developer", 1, 4));
        assert_eq!(registry.take_admissions().len(), 1);

        // What `stop` does to a queued run, without the `AppHandle` it needs for the rest.
        {
            let mut inner = registry.inner.lock();
            let key = inner.runs[&second].key();
            inner
                .queues
                .get_mut(&key)
                .expect("the agent has a queue")
                .retain(|queued| *queued != second);
        }
        registry.fail(None, second, "stopped before it started");
        registry.set_state(None, first, RunState::Running);
        registry.set_state(None, first, RunState::Finished { code: 0 });

        assert!(
            registry.take_admissions().is_empty(),
            "a cancelled run was started anyway"
        );
    }

    /// A run that owns no session is not moved by another session's frames.
    #[test]
    fn an_observation_reaches_only_the_run_that_owns_the_session() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();

        let session = SessionId::new();
        registry.bind_session(run, session);

        assert!(
            registry
                .observe(None, SessionId::new(), Observation::Exit(0))
                .is_none(),
            "a frame for a session no run holds moved something"
        );
        assert_eq!(state_of(&registry, run), RunState::Starting);

        let (moved, state) = registry
            .observe(None, session, Observation::Exit(2))
            .expect("the run that owns the session");
        assert_eq!(moved, run);
        assert_eq!(state, RunState::Finished { code: 2 });
    }

    /// **A burst is one emit.**
    ///
    /// A run reports a transition per tool call plus a statusline frame about once a second, and
    /// every emit carries a whole roster into every window. One flusher is started for the burst,
    /// one project is owed an emit however many times it changed, and nothing is due until the
    /// burst has settled.
    #[test]
    fn a_burst_of_changes_costs_one_emit() {
        let coalescer = Coalescer::default();
        let project = ProjectId::new();

        let flushers = (0..50).filter(|_| coalescer.mark(project)).count();
        assert_eq!(flushers, 1, "a burst must not start a thread per change");
        assert!(
            coalescer.take_due().is_none(),
            "the emit fired while the burst was still arriving"
        );

        std::thread::sleep(COALESCE + Duration::from_millis(40));
        let due = coalescer.take_due().expect("the burst has settled");
        assert_eq!(due, vec![project], "fifty changes, one roster");
        assert!(coalescer.take_due().is_none(), "and it fired twice");
        // The flusher slot is free again, so the next change starts a new one.
        assert!(coalescer.mark(project));
    }

    /// The ceiling, which is what keeps a continuously-working run visible.
    ///
    /// A bare trailing debounce never fires for a run that reports every hundred milliseconds:
    /// each change resets it. The state is poked directly, the way `lsp::Kick`'s own test does,
    /// because the alternative is a test that sleeps for a second.
    #[test]
    fn a_run_that_never_stops_reporting_still_updates() {
        let coalescer = Coalescer::default();
        let project = ProjectId::new();
        coalescer.mark(project);
        {
            let mut state = coalescer.state.lock();
            state.first = Some(Instant::now() - COALESCE_CEILING - Duration::from_millis(10));
            // Still arriving: the trailing debounce alone would say "not yet".
            state.last = Some(Instant::now());
        }
        assert_eq!(
            coalescer.take_due().as_deref(),
            Some([project].as_slice()),
            "the ceiling did not override the trailing debounce"
        );
    }

    // ======================================================================================
    // Pause and resume. (M18)
    // ======================================================================================

    /// **Mark first, signal second — the ordering the whole feature turns on.**
    ///
    /// `take_freezes` is the mark, and nothing is signalled until it has returned. So the state
    /// this asserts is exactly the state a `SIGSTOP` is delivered *into*: the queue already shut,
    /// the rows already `Paused`. The other order leaves a window in which the queue writes a
    /// follow-up into a stopped child's PTY — the write succeeds into the kernel buffer and is
    /// read on resume, out of order with whatever the model was mid-turn on, with nothing
    /// anywhere reporting it.
    #[test]
    fn the_queue_is_shut_before_any_child_is_signalled() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let running = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        registry.bind_session(running, SessionId::new());
        assert!(registry.set_state(None, running, RunState::Running));
        // Dispatched after the admission pass, so it is genuinely waiting on the queue rather
        // than on a slot: `qa` has a free one, and only the pause is keeping it where it is.
        let queued = registry.enqueue(spec(project, "qa", 1, 4));

        let frozen = registry
            .take_freezes(project, None, None, 1_000)
            .expect("a project may always be paused");

        // Everything below happens *before* the caller has sent a single signal.
        assert_eq!(frozen.len(), 1, "one live child to freeze");
        assert_eq!(
            state_of(&registry, running),
            RunState::Paused {
                since_unix_ms: 1_000
            }
        );
        assert!(!registry.dispatching(project), "the queue is still open");
        assert!(
            registry.take_admissions().is_empty(),
            "a run was admitted into a project that is being frozen"
        );
        assert_eq!(state_of(&registry, queued), RunState::Queued);
    }

    /// A queued run has no child, so a project pause holds it in the queue rather than freezing it.
    #[test]
    fn pausing_a_project_leaves_a_queued_run_queued() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let queued = registry.enqueue(spec(project, "developer", 1, 4));

        let frozen = registry
            .take_freezes(project, None, None, 1_000)
            .expect("a project may always be paused");
        assert!(frozen.is_empty(), "something with no child was signalled");
        assert_eq!(
            state_of(&registry, queued),
            RunState::Queued,
            "a queued run has no child to freeze, and `Paused` is not a queue position"
        );
    }

    /// A queued run in a paused project says *why* it is queued.
    ///
    /// The state the terrastrike audit found and could not read from any surface cide has: a probe
    /// run sat `[queued]` for hours with both project slots free, while the roster called its role
    /// `ready` and dispatch had answered "Dispatched" as usual. Nothing was broken — the queue was
    /// shut, deliberately, by a pause taken before a restart — and nothing said so.
    #[test]
    fn a_queued_run_in_a_paused_project_says_the_pause_is_why() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let queued = registry.enqueue(spec(project, "developer", 1, 4));

        // Before the pause it is queued for the ordinary reason, and must not claim otherwise.
        let note = wire_note(&registry, project, queued);
        assert!(
            !note.as_deref().unwrap_or_default().contains("paused"),
            "an open queue says nothing about a pause: {note:?}"
        );

        registry
            .take_freezes(project, None, None, 1_000)
            .expect("a project may always be paused");

        let note = wire_note(&registry, project, queued).expect("a held run says why");
        assert_eq!(note, PAUSED_QUEUE_NOTE);
        // Names the gesture that ends it, not merely the state — a note saying only "paused"
        // leaves the reader to find the button.
        assert!(note.contains("Resume"), "{note}");
        // And never "waiting for a slot": the slots are free, which is exactly what made this
        // state unreadable from the panel.
        assert!(!note.contains("slot"), "{note}");
    }

    /// The note is derived, so a run restored from the snapshot into a paused project carries it.
    ///
    /// The reason it is computed in `runs_for` rather than written at `admit_a_pass`'s `continue`:
    /// a restored run has never been through an admission pass, so a note written there would be
    /// missing on exactly the runs a restart leaves behind — which is the population the audit was
    /// looking at.
    #[test]
    fn only_queued_runs_are_relabelled_by_a_pause() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(spec(project, "developer", 1, 4));
        let second = registry.enqueue(spec(project, "developer", 1, 4));
        // The first takes the role's only slot; the second is genuinely queued behind it.
        registry.take_admissions();
        assert_ne!(state_of(&registry, first), RunState::Queued);

        registry
            .take_freezes(project, None, None, 1_000)
            .expect("a project may always be paused");

        // The running one keeps whatever its own note was: overriding it would replace a fact
        // about that run with one about the project.
        assert_ne!(
            wire_note(&registry, project, first).as_deref(),
            Some(PAUSED_QUEUE_NOTE),
            "a run that is not queued is not being held by the queue"
        );
        assert_eq!(
            wire_note(&registry, project, second).as_deref(),
            Some(PAUSED_QUEUE_NOTE)
        );

        // A different project's queue is untouched — `paused_projects` is per project and the
        // note is read from it, so this is the assertion that the two cannot bleed.
        let other = ProjectId::new();
        let elsewhere = registry.enqueue(spec(other, "developer", 1, 4));
        assert_ne!(
            wire_note(&registry, other, elsewhere).as_deref(),
            Some(PAUSED_QUEUE_NOTE)
        );
    }

    /// Freezing one run by hand is refused when there is no child behind it, with the way out.
    #[test]
    fn pausing_a_run_that_has_not_started_says_what_to_do_instead() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let queued = registry.enqueue(spec(project, "developer", 1, 4));

        let refused = registry
            .take_freezes(project, Some(queued), None, 1_000)
            .expect_err("a queued run has nothing to freeze");
        assert!(
            refused.to_string().contains("has not started yet"),
            "the refusal has to name the alternative; got: {refused}"
        );
        assert!(
            registry.dispatching(project),
            "a refused single-run pause shut the whole project's queue"
        );
    }

    // ==========================================================================================
    // The worked clock. `move_to` is the whole of it, so the arithmetic is driven directly over
    // injected instants; the registry tests below assert the *invariant* it keeps, which is what
    // a wall-clock opening stamp still lets a test see.
    // ==========================================================================================

    /// A run to drive `move_to` over, with its clock wound back to a known zero.
    ///
    /// Minted through the registry rather than built as a literal, so the fields under test are
    /// the ones a dispatch really produces — a hand-built `LiveRun` would go on compiling after
    /// `insert_run` stopped initialising them.
    fn clocked(registry: &AgentRegistry, run: RunId) -> parking_lot::MappedMutexGuard<'_, LiveRun> {
        parking_lot::MutexGuard::map(registry.inner.lock(), |inner| {
            inner.runs.get_mut(&run).expect("the run is listed")
        })
    }

    /// **A pause adds nothing, and the run resumes where its figure left off.**
    ///
    /// The reported bug, as arithmetic. The run works four minutes, is frozen for a day, works
    /// one more minute and ends: the figure is five minutes, not a day and five minutes.
    #[test]
    fn a_pause_adds_nothing_to_the_worked_figure() {
        const MINUTE: u64 = 60_000;
        const DAY: u64 = 24 * 60 * MINUTE;
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 1));
        let mut live = clocked(&registry, run);

        // Dispatched and queued. Nothing is open, because the queue is not work.
        assert_eq!(live.worked_ms, 0);
        assert_eq!(live.working_since_unix_ms, None);

        move_to(&mut live, RunState::Starting, 0);
        assert_eq!(live.working_since_unix_ms, Some(0), "the clock opens here");
        move_to(&mut live, RunState::Running, MINUTE);
        assert_eq!(
            (live.worked_ms, live.working_since_unix_ms),
            (0, Some(0)),
            "work → work leaves the open interval alone; closing and reopening it would be the \
             same total only by accident of the two happening at one instant"
        );

        move_to(
            &mut live,
            RunState::Paused {
                since_unix_ms: 4 * MINUTE,
            },
            4 * MINUTE,
        );
        assert_eq!(live.worked_ms, 4 * MINUTE, "four minutes banked");
        assert_eq!(live.working_since_unix_ms, None, "and nothing left running");

        // A day of freeze. The clock is not running, so there is no interval to subtract — which
        // is why `Frozen::at_unix_ms` keeps its one existing purpose and this needs no
        // arithmetic of its own.
        move_to(&mut live, RunState::Running, 4 * MINUTE + DAY);
        assert_eq!(
            live.worked_ms,
            4 * MINUTE,
            "the day the run spent frozen must not land in its total"
        );

        move_to(&mut live, RunState::Finished { code: 0 }, 5 * MINUTE + DAY);
        assert_eq!(live.worked_ms, 5 * MINUTE);
        assert_eq!(live.working_since_unix_ms, None);
        assert_eq!(
            worked_now(&live, 5 * MINUTE + DAY + 10 * DAY),
            5 * MINUTE,
            "a finished run's figure is final however far the clock moves — the history row's \
             whole claim, and the reason this change needs no end timestamp"
        );
    }

    /// **Queue wait is not work.**
    ///
    /// The third way the single figure overstated, and the quietest: a run that sat behind two
    /// others for twenty minutes did nothing in them, and counting the queue would make a role's
    /// figures depend on how busy the project was rather than on what the role did.
    #[test]
    fn the_wait_for_a_slot_is_not_work() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 1));
        let mut live = clocked(&registry, run);
        // Twenty minutes queued, then admitted.
        move_to(&mut live, RunState::Starting, 20 * 60_000);
        assert_eq!(live.worked_ms, 0);
        assert_eq!(live.working_since_unix_ms, Some(20 * 60_000));
        assert_eq!(
            worked_now(&live, 20 * 60_000),
            0,
            "a run admitted after a long wait has worked none of it"
        );
    }

    /// **A backwards clock reads as no time passed.**
    ///
    /// `freeze_may_have_killed_the_turn`'s rule in a second place. This is wall clock and it
    /// steps — an NTP correction, a suspend and resume — and the failure the other way is
    /// permanent: an enormous interval lands in `worked_ms` and nothing ever takes it out again.
    #[test]
    fn a_clock_that_went_backwards_banks_nothing() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 1));
        let mut live = clocked(&registry, run);
        move_to(&mut live, RunState::Running, 10_000);
        move_to(&mut live, RunState::Idle, 4_000);
        assert_eq!(live.worked_ms, 0, "not a huge number, and not a panic");
        assert_eq!(worked_now(&live, 1_000), 0);
    }

    /// **A frozen run holds no open interval, and a thawed one opens a fresh one.**
    ///
    /// The structural half, through the real freeze and thaw rather than through `move_to` —
    /// which is what proves the two paths call it at all. The instants are injected, so the
    /// claim does not depend on how long the test took to run.
    #[test]
    fn a_freeze_shuts_the_worked_clock_and_a_thaw_restarts_it() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 1));
        registry.take_admissions();
        registry.bind_session(run, SessionId::new());
        assert!(registry.set_state(None, run, RunState::Running));
        assert!(
            clocked(&registry, run).working_since_unix_ms.is_some(),
            "a running run has an interval open"
        );

        let banked = {
            registry
                .take_freezes(project, Some(run), None, 1_000)
                .expect("the run is live");
            let live = clocked(&registry, run);
            assert_eq!(
                live.working_since_unix_ms, None,
                "a frozen run's clock must be shut, or the freeze accrues"
            );
            live.worked_ms
        };

        let thaws = registry
            .plan_thaws(project, Some(run))
            .expect("the run is frozen");
        // An hour later.
        registry.finish_thaws(project, false, &thaws, 1_000 + 3_600_000);
        let live = clocked(&registry, run);
        assert_eq!(live.state, RunState::Running, "restored verbatim");
        assert_eq!(
            live.worked_ms, banked,
            "the hour the run spent frozen is not in its total"
        );
        assert_eq!(
            live.working_since_unix_ms,
            Some(1_000 + 3_600_000),
            "and the new interval starts at the thaw, not at the freeze"
        );
    }

    /// **The slot is not touched by a freeze, in either direction.**
    ///
    /// A pause that released the slot would let the next queued run of the role start into the
    /// worktree the frozen one is still sitting in; a resume that took a second one would let two
    /// start once it finished. The run's claim on its role is a fact about its history, and being
    /// stopped is not part of that history.
    #[test]
    fn a_freeze_and_a_thaw_leave_the_slot_where_it_was() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let first = registry.enqueue(spec(project, "developer", 1, 4));
        let second = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        registry.bind_session(first, SessionId::new());
        assert!(registry.set_state(None, first, RunState::Running));

        registry
            .take_freezes(project, Some(first), None, 1_000)
            .expect("the run is live");
        assert!(
            registry.take_admissions().is_empty(),
            "a freeze released the worktree the frozen run is still sitting in"
        );

        let thaws = registry
            .plan_thaws(project, Some(first))
            .expect("the run is frozen");
        registry.finish_thaws(project, false, &thaws, 2_000);
        assert_eq!(
            state_of(&registry, first),
            RunState::Running,
            "a thawed run has to come back to the state it left"
        );
        assert!(
            registry.take_admissions().is_empty(),
            "a thaw handed out a second slot for one run"
        );

        // And the ordinary release still works afterwards, exactly once.
        assert!(registry.set_state(None, first, RunState::Idle));
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, second);
    }

    /// **Resume reopens the queue and the next pump drains it.**
    ///
    /// The half a signal cannot deliver: a project whose runs are all finished still has a shut
    /// queue, which is why `AgentRoster::Ready::dispatching` is on the wire beside them.
    #[test]
    fn resuming_a_project_starts_what_was_waiting() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        registry
            .take_freezes(project, None, None, 1_000)
            .expect("a project may always be paused");

        let waiting = registry.enqueue(spec(project, "developer", 1, 4));
        assert!(
            registry.take_admissions().is_empty(),
            "a dispatch into a paused project started anyway"
        );

        let thaws = registry
            .plan_thaws(project, None)
            .expect("nothing to refuse");
        registry.finish_thaws(project, true, &thaws, 2_000);
        assert!(registry.dispatching(project));

        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1, "the queue was not drained on resume");
        assert_eq!(admitted[0].run, waiting);
    }

    /// **The stale-turn rule, against injected clock values.**
    ///
    /// Both conditions have to hold, and the arithmetic is `saturating_sub` because the clock is
    /// wall clock: an NTP step backwards during a freeze must read as "no time passed" rather
    /// than as an enormous one that raises a warning on every row.
    #[test]
    fn only_a_long_freeze_over_a_live_turn_is_worth_suspecting() {
        let live = RunState::Running;
        assert!(
            !freeze_may_have_killed_the_turn(&live, 0, STALE_FREEZE_MS - 1),
            "a freeze shorter than the threshold cannot have timed anything out"
        );
        assert!(freeze_may_have_killed_the_turn(&live, 0, STALE_FREEZE_MS));
        assert!(freeze_may_have_killed_the_turn(
            &live,
            1_000,
            1_000 + STALE_FREEZE_MS * 10
        ));

        // `AwaitingPermission` is in, deliberately: the turn around the prompt is unfinished, and
        // the same set `SessionState::is_live` names is the set this one names.
        assert!(freeze_may_have_killed_the_turn(
            &RunState::AwaitingPermission,
            0,
            STALE_FREEZE_MS
        ));

        // And nothing that was not working is ever suspected, however long the freeze.
        for idle in [
            RunState::Idle,
            RunState::Queued,
            RunState::Starting,
            RunState::Finished { code: 0 },
            RunState::Failed {
                reason: "no worktree".into(),
            },
        ] {
            assert!(
                !freeze_may_have_killed_the_turn(&idle, 0, STALE_FREEZE_MS * 100),
                "{idle:?} had no turn in flight to lose"
            );
        }

        // A clock that went backwards under the freeze.
        assert!(!freeze_may_have_killed_the_turn(&live, 10_000, 1));
    }

    /// A long freeze over a live turn is the one that comes back with something to watch.
    #[test]
    fn a_long_freeze_hands_back_a_run_to_watch_and_a_short_one_does_not() {
        for (elapsed, expected) in [(STALE_FREEZE_MS * 2, 1), (1_000, 0)] {
            let registry = AgentRegistry::default();
            let project = ProjectId::new();
            let run = registry.enqueue(spec(project, "developer", 1, 4));
            registry.take_admissions();
            let session = SessionId::new();
            registry.bind_session(run, session);
            registry.set_state(None, run, RunState::Running);

            registry
                .take_freezes(project, Some(run), None, 1_000)
                .expect("the run is live");
            let thaws = registry
                .plan_thaws(project, Some(run))
                .expect("the run is frozen");
            let watch = registry.finish_thaws(project, false, &thaws, 1_000 + elapsed);

            assert_eq!(watch.len(), expected, "a {elapsed} ms freeze");
            if expected == 1 {
                assert_eq!(watch[0], (session, run));
            }
        }
    }

    /// The offer is raised once and reaches the wire.
    ///
    /// The two commands that answer it need an `AppHandle` for their emit, which this build
    /// cannot make — `tauri`'s mock app is behind a feature it does not enable, the wall
    /// `hooks::live_in` documents — so what is under test is the half that can be wrong: the
    /// latch, and the field arriving on `AgentRun`.
    #[test]
    fn the_stale_turn_offer_is_raised_once_and_reaches_the_wire() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        registry.bind_session(run, SessionId::new());
        registry.set_state(None, run, RunState::Running);

        assert!(registry.mark_stale(run));
        assert!(!registry.mark_stale(run), "the offer was raised twice");
        assert!(
            registry
                .runs_for(project)
                .iter()
                .any(|wire| wire.run == run && wire.stale_turn),
            "the offer never reached the wire"
        );

        // What `ack_stale_turn` does to the registry — the *leave it* answer. The flag goes and
        // nothing else moves: acknowledging a suspicion is not a claim about the run.
        registry
            .inner
            .lock()
            .runs
            .get_mut(&run)
            .expect("the run")
            .stale_turn = false;
        assert_eq!(state_of(&registry, run), RunState::Running);
        assert!(
            registry.mark_stale(run),
            "the offer could not be raised again after being acknowledged"
        );
    }

    /// A run that came back to life on its own is never offered a retry.
    #[test]
    fn a_run_that_finished_or_went_idle_under_the_watch_is_not_suspected() {
        for state in [RunState::Idle, RunState::Finished { code: 0 }] {
            let registry = AgentRegistry::default();
            let project = ProjectId::new();
            let run = registry.enqueue(spec(project, "developer", 1, 4));
            registry.take_admissions();
            registry.bind_session(run, SessionId::new());
            registry.set_state(None, run, RunState::Running);
            registry.set_state(None, run, state.clone());

            assert!(
                !registry.mark_stale(run),
                "{state:?} was offered a retry for a turn that had demonstrably ended"
            );
        }
    }

    /// A frozen child that died under the freeze stops being frozen.
    ///
    /// `Observation::Exit` is the one observation that answers from `Paused`, so this is reachable
    /// — and leaving the record behind would put a dead session on `thaw_for_shutdown`'s list and
    /// leave a finished row restorable to a state its process no longer has.
    #[test]
    fn a_run_that_died_under_the_freeze_leaves_no_freeze_behind() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        let session = SessionId::new();
        registry.bind_session(run, session);
        registry.set_state(None, run, RunState::Running);
        registry
            .take_freezes(project, Some(run), None, 1_000)
            .expect("the run is live");
        assert_eq!(registry.inner.lock().frozen_sessions.len(), 1);

        registry
            .observe(None, session, Observation::Exit(9))
            .expect("an exit answers from `Paused`");

        assert_eq!(state_of(&registry, run), RunState::Finished { code: 9 });
        assert!(
            registry.inner.lock().frozen_sessions.is_empty(),
            "a dead session was left on the thaw list"
        );
        assert!(registry.inner.lock().runs[&run].frozen.is_none());
    }

    /// **A real child, stopped and continued.**
    ///
    /// Everything above drives the registry; this drives the kernel. `/bin/sh` rather than
    /// `claude` for `a_real_childs_exit_finishes_its_run`'s reason: what is under test is the
    /// signal reaching the child's process *group*, and the child's identity is irrelevant to it.
    /// The loop with a `sleep` in it is deliberate — `sleep` is a separate process in the same
    /// group, so a signal that reached only the leader would leave it running.
    #[test]
    fn a_stopped_child_makes_no_progress_until_it_is_continued() {
        let pty = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("i=0; while [ $i -lt 10 ]; do i=$((i+1)); sleep 0.1; done; exit 7"),
        )
        .expect("spawn sh");

        // Long enough for the child to be running its loop, and far short of the second it needs
        // to finish it.
        std::thread::sleep(Duration::from_millis(200));
        signal_pty(&pty, PauseSignal::Stop);

        // Twice the child's whole runtime. A child still being scheduled would be long gone.
        std::thread::sleep(Duration::from_millis(2_000));
        assert!(
            !pty.has_exited(),
            "a SIGSTOPped child ran to completion anyway"
        );

        signal_pty(&pty, PauseSignal::Cont);
        let deadline = Instant::now() + Duration::from_secs(10);
        while pty.exit_status().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            pty.exit_status().map(|exit| exit.code),
            Some(7),
            "the thawed child never finished"
        );
    }

    /// **A live run's saved screen holds what the mirror held — and this is the seam that says
    /// so.**
    ///
    /// The reason it exists: on the machine this was reported from, every `run-screens` file the
    /// last teardown wrote was ten bytes — `full_state`'s `ESC \` + leave-alt-screen lead-in
    /// over a mirror with no scrollback and no non-blank row — so a resumed run's pane came back
    /// holding nothing but a separator. Nothing in the suite could have caught it: the write had
    /// no path seam and therefore no test at all.
    ///
    /// What this pins is the *mechanism*: a run whose session mirror holds rendered rows writes
    /// them, and the prune leaves only the runs this registry still holds. A ten-byte file in
    /// the wild is therefore a statement about the mirror at that moment rather than about this
    /// code — which is precisely why `replayed_run_log` is the primary seed now and this is the
    /// fallback: the log is written as the run goes, and cannot be empty for a run that spoke.
    #[test]
    fn a_live_runs_saved_screen_holds_what_its_mirror_held() {
        let dir = std::env::temp_dir().join(format!("cide-run-screens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let registry = Arc::new(AgentRegistry::default());
        let sessions = SessionRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();

        let pty = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("printf 'alpha\\nbeta\\n'; sleep 30")
                .render(cide_pty::LineRender::new(Arc::new(|line: &str| {
                    cide_pty::Rendered::Replace(format!("[{line}]"))
                }))),
        )
        .expect("spawn sh");
        let session = SessionId::new();
        registry.bind_session(run, session);
        registry.set_state(None, run, RunState::Running);
        sessions.insert(session, Arc::clone(&pty));

        // The mirror is fed on the coalescer thread, so wait for it rather than for the child.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && !String::from_utf8_lossy(&pty.screen_state()).contains("[beta]")
        {
            std::thread::sleep(Duration::from_millis(20));
        }

        registry.save_screens_to(&dir, &sessions);
        let saved = std::fs::read(dir.join(format!("{run}.screen"))).expect("a screen was written");
        let text = String::from_utf8_lossy(&saved);
        assert!(
            text.contains("[alpha]") && text.contains("[beta]"),
            "the saved screen lost the rendered rows the mirror held: {text:?}"
        );

        // A run this registry no longer holds leaves nothing behind — the prune, which is also
        // what keeps the directory from growing one file per run for ever.
        let stale = dir.join("11111111-1111-4111-8111-111111111111.screen");
        std::fs::write(&stale, b"old").expect("write a stale screen");
        registry.save_screens_to(&dir, &sessions);
        assert!(!stale.exists(), "a screen for a run nobody holds was kept");

        pty.kill();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`thaw_for_shutdown` is what keeps the ladder's first two rungs meaningful.**
    ///
    /// The ladder's first two rungs are the **catchable** ones — that is their whole purpose, so a
    /// `claude` can finish writing the transcript resume depends on — and a catchable signal
    /// delivered to a stopped process is by definition pending: there is a handler to run and no
    /// scheduled thread to run it on. (The kernel *does* wake a stopped task for a signal whose
    /// disposition is still the default fatal one, which is why the child below installs a `trap`.
    /// A `claude` is a Node process with handlers on both rungs, so the shape that matters is the
    /// one with a handler.)
    ///
    /// Without this call the ladder therefore spends its whole `hup_grace + term_grace` on a child
    /// that cannot answer and then SIGKILLs it — exactly the half-written transcript
    /// `lifecycle.rs` says the ladder exists to prevent. Asserted through the real thing: a
    /// stopped child sits on a SIGTERM it has queued, and dies of it the instant this thaws it.
    #[test]
    fn a_shutdown_thaws_before_it_signals() {
        let registry = Arc::new(AgentRegistry::default());
        let sessions = SessionRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();

        let pty = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("trap 'exit 0' TERM; while :; do sleep 0.05; done"),
        )
        .expect("spawn sh");
        let session = SessionId::new();
        registry.bind_session(run, session);
        registry.set_state(None, run, RunState::Running);
        sessions.insert(session, Arc::clone(&pty));

        registry
            .take_freezes(project, Some(run), None, 1_000)
            .expect("the run is live");
        // Long enough for the shell to have installed its trap. Freezing it before that would
        // leave SIGTERM at its default disposition, which the kernel *does* wake a stopped task
        // to deliver — a different experiment from the one this is running.
        std::thread::sleep(Duration::from_millis(300));
        signal_pty(&pty, PauseSignal::Stop);

        // The rung the ladder reaches for. A stopped process with a handler for it cannot act on
        // it: the signal is queued and the thread that would run the handler is not scheduled.
        signal_pty_raw(&pty, libc::SIGTERM);
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !pty.has_exited(),
            "a stopped child acted on a signal it can only have queued"
        );

        registry.thaw_for_shutdown(&sessions);

        // The SIGTERM was pending the whole time and lands the instant the child is continued.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !pty.has_exited() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            pty.has_exited(),
            "the thaw never reached the child, so the ladder would have SIGKILLed it"
        );
        assert!(
            registry.inner.lock().frozen_sessions.is_empty(),
            "a second teardown pass would signal sessions this one already thawed"
        );
    }

    /// The one-line escape hatch the shutdown test needs: an arbitrary signal to a child's group.
    fn signal_pty_raw(pty: &PtySession, signal: libc::c_int) {
        if let Some(pid) = pty.child_pid() {
            cide_core::child_env::signal_group(pid, signal);
        }
    }

    /// **A real child, reaped, with the status the reaper actually saw.**
    ///
    /// Everything above this line drives the state machine by hand. This one forks, waits for the
    /// exit, and asserts the number arrived — which is the half `Observation::Exit` exists for and
    /// the half a hand-driven test cannot reach: `on_exit` fires on `cide-pty`'s reaper thread,
    /// and a registration that raced the child's death has to fire immediately rather than never.
    ///
    /// `/bin/sh` rather than `claude`, so it runs in CI: what is under test is the wiring between
    /// the reaper and the run, and the child's identity is irrelevant to it. `state.rs` and
    /// `cmd::session` both spawn `/bin/sh` in their own unit tests for the same reason.
    #[test]
    fn a_real_childs_exit_finishes_its_run() {
        let registry = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        let queued = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();

        let session = SessionId::new();
        registry.bind_session(run, session);

        let pty = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("exit 3"),
        )
        .expect("spawn sh");

        let (tx, rx) = std::sync::mpsc::channel();
        registry.watch_exit_with(
            session,
            &pty,
            move |_, run| {
                let _ = tx.send(run);
            },
            // No pool in this fixture, so no failover can be planned; the closure is here to
            // satisfy the signature and asserts nothing.
            |_, _| {},
            |_, _| {},
        );

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(run),
            "the reaper never reached the run"
        );
        assert_eq!(state_of(&registry, run), RunState::Finished { code: 3 });
        // And the exit released the role, so what was waiting behind it can start.
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, queued);
    }

    // ======================================================================================
    // The second harness: a run whose only channel is its own output.
    // ======================================================================================

    /// Two real lines from `opencode run --format json`, as the fixtures in
    /// `cide_agents::harness::opencode` record them.
    const STEP_START: &str = r#"{"type":"step_start","timestamp":1755402000000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_a","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_a","type":"step-start"}}"#;
    const STEP_FINISH: &str = r#"{"type":"step_finish","timestamp":1755402002000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_b","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_a","type":"step-finish","reason":"stop","cost":0,"tokens":{"total":5725,"input":5696,"output":4,"reasoning":25,"cache":{"read":0,"write":0}}}}"#;

    /// What `cide_agents::harness::opencode::capture` does, in four lines.
    ///
    /// The real one is private to that module and tested there; what is under test *here* is the
    /// wiring around it — that a sink splits the child's bytes into lines, hands each to the
    /// harness, and writes the answer down once.
    /// A pass-through display for [`AgentRegistry::stream_hook`] in tests: what is under test
    /// is the raw channel, and the real renderer has tests of its own in `cide-agents`.
    fn test_render(
        _state: &mut RenderState,
        _line: &str,
        _handle: Option<u64>,
    ) -> cide_pty::Rendered {
        cide_pty::Rendered::Keep
    }

    fn test_keep(_line: &str) -> bool {
        false
    }

    fn test_capture(line: &str) -> Option<String> {
        let event: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        Some(event.get("sessionID")?.as_str()?.to_string())
    }

    // ======================================================================================
    // Seeding a resumed run's pane from its own log.
    // ======================================================================================

    /// A renderer that exercises all three [`cide_pty::Rendered`] answers and the marker, so
    /// the replay's loop is asserted over the whole vocabulary rather than the easy third of
    /// it. `drop` draws nothing, `step` puts the live marker up, `end` takes it down, anything
    /// else is replaced with its own text and the handle it was given.
    fn row_render(state: &mut RenderState, line: &str, handle: Option<u64>) -> cide_pty::Rendered {
        match line {
            "drop" => cide_pty::Rendered::Drop,
            "keep" => cide_pty::Rendered::Keep,
            "step" => {
                state.in_step = true;
                state.marker = true;
                cide_pty::Rendered::Replace("▸ working…".into())
            }
            "end" => {
                state.in_step = false;
                state.marker = false;
                cide_pty::Rendered::Replace("done".into())
            }
            "two" => cide_pty::Rendered::Replace("first\nsecond".into()),
            _ => cide_pty::Rendered::Replace(match handle {
                Some(handle) => format!("{line} #{handle}"),
                None => line.to_string(),
            }),
        }
    }

    /// Everything but `drop` is worth keeping, so the handle rule is exercised on most rows.
    fn row_keep(line: &str) -> bool {
        line != "drop"
    }

    /// **Every row the replay emits is terminated, and a dropped line emits nothing.**
    ///
    /// The three `Rendered` answers are the whole contract with `cide_pty::render_lines`, which
    /// this loop is a copy of minus the partial-line plumbing. `Replace`'s own newlines become
    /// `\r\n` because these bytes bypass the pty's output post-processing — the same rule, and
    /// for the same reason, as the live path's.
    #[test]
    fn a_replay_terminates_every_row_and_draws_nothing_for_a_dropped_line() {
        let rows = replay_rows(
            &["alpha", "drop", "keep", "two"],
            row_keep,
            row_render,
            7,
            |_, _| None,
        );
        assert_eq!(
            String::from_utf8_lossy(&rows),
            "alpha\r\nkeep\r\nfirst\r\nsecond\r\n",
            "a row went out unterminated, or a dropped line drew something"
        );
    }

    /// **A log that stops mid-step does not leave the marker claiming the run is working.**
    ///
    /// The marker is erased by the *next* line's rendering, and after a replay the next thing
    /// written is cide's own `— resumed after a cide restart —`. Without the erase that
    /// separator lands under a live marker from a child that died minutes ago.
    #[test]
    fn a_replay_that_ends_mid_step_takes_the_marker_off() {
        let ended = replay_rows(&["step", "end"], row_keep, row_render, 7, |_, _| None);
        assert!(
            !String::from_utf8_lossy(&ended).ends_with(cide_agents::ERASE_MARKER),
            "a replay that ended cleanly erased a marker that was not there"
        );

        let cut = replay_rows(&["alpha", "step"], row_keep, row_render, 7, |_, _| None);
        assert!(
            String::from_utf8_lossy(&cut).ends_with(cide_agents::ERASE_MARKER),
            "a replay that ended mid-step left the marker drawn: {:?}",
            String::from_utf8_lossy(&cut)
        );
    }

    /// **A kept line is recorded once, and the handle the ring minted reaches the rendering.**
    ///
    /// This is what makes a replayed `● bash  …  #7` row's click resolve through
    /// `session_log_detail`: the row and the ring entry are filled in one pass, so the number
    /// on screen is the number the ring answers to. A line `keep` refuses is never recorded.
    #[test]
    fn a_replayed_kept_line_is_recorded_and_carries_its_handle() {
        let recorded = std::cell::RefCell::new(Vec::new());
        let rows = replay_rows(
            &["alpha", "drop", "beta"],
            row_keep,
            row_render,
            7,
            |line, stamp| {
                recorded.borrow_mut().push((line.to_string(), stamp));
                Some(recorded.borrow().len() as u64 - 1)
            },
        );
        assert_eq!(
            String::from_utf8_lossy(&rows),
            "alpha #0\r\nbeta #1\r\n",
            "the ring's handle did not reach the rendering"
        );
        assert_eq!(
            recorded
                .borrow()
                .iter()
                .map(|(line, _)| line.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"],
            "a line `keep` refused was recorded anyway"
        );
    }

    /// **A line is stamped with its own clock; a line that has none takes the log's.**
    ///
    /// opencode stamps every event and codex stamps none, so both arms are real. The fallback
    /// is the moment the log was last written — a bound, and the honest half of a bad pair:
    /// stamping `now` would make every replayed card claim its command ran at the instant
    /// somebody pressed Resume.
    #[test]
    fn a_replayed_line_takes_its_own_clock_or_the_logs() {
        assert_eq!(
            line_stamp(r#"{"type":"text","timestamp":1755402000000}"#),
            Some(1755402000000)
        );
        assert_eq!(
            line_stamp(r#"{"type":"text"}"#),
            None,
            "a clock was invented"
        );
        assert_eq!(line_stamp("not json at all"), None);
        assert_eq!(line_stamp(RUN_LOG_CAPPED), None);

        let stamps = std::cell::RefCell::new(Vec::new());
        replay_rows(
            &[r#"{"type":"text","timestamp":1755402000000}"#, "beta"],
            row_keep,
            row_render,
            999,
            |_, stamp| {
                stamps.borrow_mut().push(stamp);
                None
            },
        );
        assert_eq!(
            *stamps.borrow(),
            vec![1755402000000, 999],
            "a line's own clock was ignored, or the fallback was not the log's"
        );
    }

    /// **The replay is the log's tail, and it says how much it left behind.**
    ///
    /// The budget is [`REPLAY_LINES`], which is the ring's `CAP`: a larger one would draw
    /// handles the ring had already evicted. Saying so — and naming the file — is what keeps
    /// the cut a mirror-and-ring decision rather than a claim that the earlier work is gone.
    #[test]
    fn a_long_log_is_replayed_from_its_tail_and_says_so() {
        let dir = std::env::temp_dir().join(format!("cide-replay-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("run.log");
        let lines: Vec<String> = (0..REPLAY_LINES + 3).map(|n| format!("line{n}")).collect();
        std::fs::write(&path, lines.join("\n")).expect("write the log");

        let replayed = replayed_log_at(&path, row_keep, row_render, |_, _| None)
            .expect("a log with content replays");
        let text = String::from_utf8_lossy(&replayed);
        assert!(
            text.starts_with("\x1b[2m— 3 earlier event(s) not replayed"),
            "the tail did not say what it left behind: {:?}",
            &text[..text.len().min(120)]
        );
        assert!(
            text.contains(&path.display().to_string()),
            "the note did not name the file"
        );
        assert!(
            !text.contains("line2\r\n"),
            "a line past the budget was replayed"
        );
        assert!(
            text.ends_with("line2002\r\n"),
            "the tail did not reach the last line"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Nothing to show is `None`, never an empty preload.**
    ///
    /// Four ways a run has no recorded output: no file at all (a `claude` or `qwen` run, which
    /// is not teed), an empty one, one of only blank lines, and one whose every line the
    /// rendering drops. Each must fall through to the saved screen rather than seeding the
    /// mirror with a bare separator.
    #[test]
    fn a_run_with_nothing_recorded_seeds_nothing() {
        let dir = std::env::temp_dir().join(format!("cide-replay-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let missing = dir.join("absent.log");
        assert!(replayed_log_at(&missing, row_keep, row_render, |_, _| None).is_none());

        for (name, body) in [("empty.log", ""), ("blank.log", "\n\n   \n")] {
            let path = dir.join(name);
            std::fs::write(&path, body).expect("write");
            assert!(
                replayed_log_at(&path, row_keep, row_render, |_, _| None).is_none(),
                "{name} seeded a mirror with nothing in it"
            );
        }

        let dropped = dir.join("dropped.log");
        std::fs::write(&dropped, "drop\ndrop\n").expect("write");
        assert!(
            replayed_log_at(&dropped, row_keep, row_render, |_, _| None).is_none(),
            "a log the rendering draws nothing for still seeded the mirror"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn opencode_spec(project: ProjectId, agent: &str) -> DispatchSpec {
        DispatchSpec {
            harness: Harness::Opencode,
            ..spec(project, agent, 1, 4)
        }
    }

    /// **A run with no hooks moves on its own output, and the queue moves on its exit.**
    ///
    /// The whole liveness story for the second harness, against a real child and the real state
    /// mapping: the id the CLI minted is read off the first line, the run leaves `Starting` on
    /// output, and the role's slot comes back on the **exit** and never on a line. That last
    /// clause used to read the other way — the final `step_finish` answered `Idle` and this test
    /// asserted the queue moved on it — until run 06202dd6, where a mid-turn `step_finish`
    /// released the slot and the admitted sibling's wind-down killed the still-working child;
    /// `cide_agents::harness::opencode::observe` carries the incident. What the queued sibling
    /// here now pins is the repaired order: it is admitted only once the child is genuinely
    /// gone. Nothing here is a hook, because there is no hook to be had.
    ///
    /// `/bin/sh` rather than `opencode`, for `a_real_childs_exit_finishes_its_run`'s reason: what
    /// is under test is the wiring between a child's stdout and a run, and the child's identity is
    /// irrelevant to it. The hook rides the spec into the spawn — production's shape since the
    /// renderer moved it there — so there is no attach-after-print race left to sleep away; the
    /// `sleep` stays only so the two lines arrive as their own late chunk rather than inside the
    /// spawn's first read.
    #[test]
    fn a_harness_bound_run_moves_on_its_own_output() {
        let registry = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let run = registry.enqueue(opencode_spec(project, "developer"));
        let queued = registry.enqueue(opencode_spec(project, "developer"));
        registry.take_admissions();
        assert_eq!(state_of(&registry, run), RunState::Starting);

        let session = SessionId::new();
        registry.bind_session(run, session);

        let pty = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                // Two lines in one write, so the split is exercised rather than assumed.
                .arg(format!(
                    "sleep 0.2; printf '%s\n%s\n' '{STEP_START}' '{STEP_FINISH}'"
                ))
                .render(registry.stream_hook(
                    None,
                    run,
                    session,
                    test_capture,
                    test_keep,
                    test_render,
                    Harness::Opencode,
                )),
        )
        .expect("spawn sh");
        // The reaper's app-free half, exactly as `a_real_childs_exit_finishes_its_run` uses it:
        // the child prints its two lines and exits, and that exit is what ends the turn.
        registry.watch_exit_with(session, &pty, |_, _| {}, |_, _| {}, |_, _| {});

        // Two facts, two threads: the exit arrives on the reaper's watch, the capture on the
        // coalescer's, and nothing orders them — so the wait is on both, or the capture assert
        // races the drain of the child's last chunk.
        let deadline = Instant::now() + Duration::from_secs(10);
        let settled = |registry: &Arc<AgentRegistry>| {
            state_of(registry, run) == (RunState::Finished { code: 0 })
                && registry.inner.lock().runs[&run].harness_session.is_some()
        };
        while !settled(&registry) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }

        // The exit ended the run — no line did.
        assert_eq!(state_of(&registry, run), RunState::Finished { code: 0 });
        assert_eq!(
            registry.inner.lock().runs[&run].harness_session.as_deref(),
            Some("ses_fe6da2c1effe8Yz"),
            "the id the CLI minted is what a follow-up names"
        );

        // And the slot came back with the exit, so the queue moves — with the previous occupant
        // of the role's checkout genuinely gone, not merely presumed done by a JSON field.
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].run, queued);
    }

    /// **A follow-up is the same run, continuing.**
    ///
    /// `plan_respawn` is the decision half of `respawn` (the other half forks, and needs an
    /// `AppHandle` this build cannot make). What it must never do is mint a second run, take a
    /// second slot or ask for a second worktree — a respawn that did any of those would show two
    /// rows for one piece of work and put two children in one checkout.
    #[test]
    fn a_follow_up_continues_the_same_run_without_taking_a_second_slot() {
        let registry = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let run = registry.enqueue(opencode_spec(project, "developer"));
        registry.take_admissions();
        let session = SessionId::new();
        registry.bind_session(run, session);

        // Before the child has said anything there is nothing to continue, and the sentence says
        // what to do instead rather than starting a fresh conversation billed as a continuation.
        let refusal = registry
            .plan_respawn(project, run, "carry on")
            .expect_err("no conversation id yet")
            .to_string();
        assert!(refusal.contains("dispatched again"), "{refusal}");

        assert!(registry.note_harness_session(run, "ses_fe6da2c1effe8Yz".into()));
        // Written once. A continuing child prints the same id back, and a second write is the
        // shape a `--fork` would need — which is not what a follow-up is.
        assert!(!registry.note_harness_session(run, "ses_somebody_else".into()));

        let planned = registry
            .plan_respawn(project, run, "carry on")
            .expect("a continuable run");
        assert_eq!(planned.admission.run, run, "the same run, not a second one");
        assert_eq!(planned.harness_session, "ses_fe6da2c1effe8Yz");
        assert_eq!(
            planned.previous,
            Some(session),
            "the child to wind down first"
        );
        assert_eq!(planned.admission.prompt, "carry on");
        assert_eq!(planned.admission.agent, AgentId("developer".into()));

        // One row, and the role's only slot still held by the run that is being continued: a
        // queued run must not be admitted into the worktree its successor is about to start in.
        assert_eq!(registry.runs_for(project).len(), 1);
        let waiting = registry.enqueue(opencode_spec(project, "developer"));
        assert!(
            registry.take_admissions().is_empty(),
            "a continued run still holds its role's only slot"
        );
        assert_eq!(state_of(&registry, waiting), RunState::Queued);

        // And a run belonging to somebody else's project is not continuable from here.
        assert!(
            registry
                .plan_respawn(ProjectId::new(), run, "carry on")
                .is_err()
        );
    }

    /// Wait for a child the test killed to be reaped — the reaper is another thread.
    fn wait_exited(pty: &PtySession) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !pty.has_exited() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(pty.has_exited(), "the child never exited");
    }

    /// **Open's ladder** (M42): a queued run has nothing to show; a live child is mirrored, with
    /// the conversation carried so the pane can re-open it later; an ended one is re-opened in
    /// the real harness from the run's directory when the transcript is there; a retained exited
    /// session is still mirrored when it is not; and with nothing at all the sentence names the
    /// directory the transcript was filed under.
    #[test]
    fn open_mirrors_a_live_child_and_continues_the_conversation_once_it_is_gone() {
        let registry = Arc::new(AgentRegistry::default());
        let sessions = SessionRegistry::default();
        let project = ProjectId::new();
        let root = std::env::temp_dir().join(format!("cide-open-{}", uuid::Uuid::new_v4()));

        let run = registry.enqueue(spec(project, "developer", 1, 4));
        let plan = registry
            .open_plan_with(project, run, &sessions, &root, true, |_, _, _| true)
            .expect("the run exists");
        assert!(
            matches!(plan, RunOpen::Unavailable { .. }),
            "a queued run has no conversation: {plan:?}"
        );

        registry.take_admissions();
        let session = SessionId::new();
        registry.bind_session(run, session);
        let cwd = cide_git::worktree::path_of(&root, "developer-t-1");
        registry.note_cwd(run, &cwd);

        let pty = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("sleep 30"),
        )
        .expect("spawn sh");
        sessions.insert(session, Arc::clone(&pty));

        // Live: a mirror, whatever the filesystem says, carrying the conversation for later.
        match registry
            .open_plan_with(project, run, &sessions, &root, true, |_, _, _| false)
            .expect("the run exists")
        {
            RunOpen::Mirror {
                session: mirrored,
                continues,
            } => {
                assert_eq!(mirrored, session);
                let conversation = continues.expect("the conversation rides along");
                assert_eq!(conversation.harness, Harness::Claude);
                assert_eq!(conversation.id, session.to_string());
                assert_eq!(conversation.cwd, cwd, "the run's worktree, not the root");
            }
            other => panic!("a live child is mirrored, got {other:?}"),
        }

        // Ended, with the transcript under the worktree: the real harness, from there.
        pty.kill();
        wait_exited(&pty);
        registry.set_state(None, run, RunState::Finished { code: 0 });
        let asked = std::sync::Mutex::new(Vec::new());
        match registry
            .open_plan_with(project, run, &sessions, &root, true, |_, dir, s| {
                asked.lock().unwrap().push((dir.to_path_buf(), s));
                true
            })
            .expect("the run exists")
        {
            RunOpen::Continue { conversation } => {
                assert_eq!(conversation.harness, Harness::Claude);
                assert_eq!(conversation.id, session.to_string());
                assert_eq!(conversation.cwd, cwd);
            }
            other => panic!("an ended run with a transcript continues, got {other:?}"),
        }
        assert_eq!(
            asked.into_inner().unwrap(),
            vec![(cwd.clone(), session)],
            "the transcript is looked for under the run's directory, and only there"
        );

        // Transcript gone, session retained: what Open showed before — the kept screen.
        assert!(matches!(
            registry
                .open_plan_with(project, run, &sessions, &root, true, |_, _, _| false)
                .expect("the run exists"),
            RunOpen::Mirror { .. }
        ));

        // Nothing retained either: the sentence names where the transcript was.
        let none = SessionRegistry::default();
        match registry
            .open_plan_with(project, run, &none, &root, true, |_, _, _| false)
            .expect("the run exists")
        {
            RunOpen::Unavailable { reason } => assert!(
                reason.contains(&cwd.display().to_string()),
                "the sentence names the directory: {reason}"
            ),
            other => panic!("nothing to show, got {other:?}"),
        }
        match registry
            .open_plan_with(project, run, &none, &root, false, |_, _, _| false)
            .expect("the run exists")
        {
            RunOpen::Unavailable { reason } => {
                assert!(reason.contains("--resume"), "the switch is named: {reason}")
            }
            other => panic!("nothing to show, got {other:?}"),
        }
    }

    /// An opencode run's conversation is the `ses_…` it printed, re-opened in the TUI from the
    /// worktree the row stamped — and it is openable with **no cide session at all**, which is
    /// why `openable` is a wire field and not a rule the panel derives from `session`. (M42)
    #[test]
    fn open_puts_the_opencode_tui_on_a_finished_runs_conversation() {
        let registry = Arc::new(AgentRegistry::default());
        let sessions = SessionRegistry::default();
        let project = ProjectId::new();
        let root = PathBuf::from("/p");

        let run = registry.enqueue(opencode_spec(project, "qa"));
        registry.take_admissions();
        let session = SessionId::new();
        registry.bind_session(run, session);
        registry
            .inner
            .lock()
            .runs
            .get_mut(&run)
            .expect("row")
            .checkout = Some("qa-t-2".into());
        registry.set_state(None, run, RunState::Finished { code: 0 });

        // Ended before naming its conversation: nothing to open, and the sentence says so.
        match registry
            .open_plan_with(project, run, &sessions, &root, true, |_, _, _| true)
            .expect("the run exists")
        {
            RunOpen::Unavailable { reason } => {
                assert!(reason.contains("conversation id"), "{reason}")
            }
            other => panic!("no id, nothing to open, got {other:?}"),
        }
        // The wire still offers Open: the row has a session the registry *knew*, and whether
        // it is retained is the click's question, not the broadcast's — a stale `true` costs
        // the sentence above, never a pane. The withheld case is the restored row below.
        assert!(registry.runs_for(project)[0].openable);

        assert!(registry.note_harness_session(run, "ses_0a1b2c".into()));
        match registry
            .open_plan_with(project, run, &sessions, &root, true, |_, _, _| false)
            .expect("the run exists")
        {
            RunOpen::Continue { conversation } => {
                assert_eq!(conversation.harness, Harness::Opencode);
                assert_eq!(conversation.id, "ses_0a1b2c");
                assert_eq!(
                    conversation.cwd,
                    cide_git::worktree::path_of(&root, "qa-t-2"),
                    "no cwd was recorded in this process, so the stamped checkout names it"
                );
            }
            other => panic!("a named conversation continues in the TUI, got {other:?}"),
        }

        // The restored shape: no session, an openable conversation — and, with the id taken
        // away again, nothing: the withheld-not-disabled rule through `openable`.
        registry
            .inner
            .lock()
            .runs
            .get_mut(&run)
            .expect("row")
            .session = None;
        let row = registry.runs_for(project).remove(0);
        assert!(row.session.is_none());
        assert!(row.openable, "Open is on offer without a session");
        {
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&run).expect("row");
            live.harness_session = None;
            live.reopenable = false;
        }
        assert!(!registry.runs_for(project)[0].openable);
    }

    /// The registry never starts a second harness process on a conversation a pane is driving
    /// (M42): a single-run Resume is refused with the sentence, the project scope skips the run
    /// and writes the sentence on its row, and once the pane's child is gone the run continues.
    #[test]
    fn a_conversation_open_in_a_pane_is_not_continued_by_the_registry() {
        let registry = Arc::new(AgentRegistry::default());
        let sessions = SessionRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        let session = SessionId::new();
        registry.bind_session(run, session);
        registry.set_state(None, run, RunState::Interrupted);

        // A person re-opened the conversation: `session_spawn` recorded the viewer, and the
        // session registry holds the pane's live child under the same id (`--resume` keeps it).
        let pty = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("sleep 30"),
        )
        .expect("spawn sh");
        sessions.insert(session, Arc::clone(&pty));
        registry.note_viewer(
            &HarnessSession {
                harness: Harness::Claude,
                id: session.to_string(),
                cwd: PathBuf::from("/p/.cide/worktrees/developer-t-1"),
            },
            session,
        );
        // And that child is the pane's to close — `session_kill` must not refuse it.
        assert!(!registry.owns_session(session));

        let refused = registry
            .requeue_interrupted(project, Some(run), Some(&sessions), &|_| None)
            .expect_err("viewed");
        assert!(refused.to_string().contains("open in a pane"), "{refused}");
        assert_eq!(state_of(&registry, run), RunState::Interrupted);

        registry
            .requeue_interrupted(project, None, Some(&sessions), &|_| None)
            .expect("the project scope skips rather than refuses");
        assert_eq!(state_of(&registry, run), RunState::Interrupted);
        let note = registry.inner.lock().runs[&run].note.clone();
        assert!(
            note.as_deref()
                .is_some_and(|note| note.contains("open in a pane")),
            "the row says why it was skipped: {note:?}"
        );

        // The pane's child ends: `report_exit` forgets the viewer, and the run may continue.
        pty.kill();
        wait_exited(&pty);
        registry.forget_viewer(session);
        registry
            .requeue_interrupted(project, Some(run), Some(&sessions), &|_| None)
            .expect("free again");
        assert_eq!(state_of(&registry, run), RunState::Queued);
    }

    /// A finished `claude` row keeps its session across a restart exactly when its transcript
    /// is still where the worktree filed it — that is what makes Open re-open the real harness
    /// on it — and drops it otherwise, so the withheld-not-disabled rule holds through
    /// `openable`. (M42)
    #[test]
    fn a_finished_run_keeps_its_session_across_a_restart_while_its_transcript_survives() {
        let path = std::env::temp_dir().join(format!("cide-runs-{}.json", uuid::Uuid::new_v4()));
        let before = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let run = before.enqueue(spec(project, "developer", 1, 4));
        before.take_admissions();
        let session = SessionId::new();
        before.bind_session(run, session);
        {
            let mut inner = before.inner.lock();
            let live = inner.runs.get_mut(&run).expect("row");
            live.checkout = Some("developer-t-9".into());
            live.state = RunState::Finished { code: 0 };
        }
        before.write_snapshot_to(&path);

        let root = PathBuf::from("/p");
        let worktree = cide_git::worktree::path_of(&root, "developer-t-9");

        let after = Arc::new(AgentRegistry::default());
        after.restore_snapshot_from(
            &path,
            |_| true,
            |_| Some(root.clone()),
            |_, cwd, s| cwd == worktree && s == session,
        );
        let row = after
            .runs_for(project)
            .into_iter()
            .find(|row| row.run == run)
            .expect("restored as history");
        assert!(matches!(row.state, RunState::Finished { code: 0 }));
        assert_eq!(row.session, Some(session), "the id Open needs");
        assert!(row.openable);

        let bare = Arc::new(AgentRegistry::default());
        bare.restore_snapshot_from(&path, |_| true, |_| Some(root.clone()), |_, _, _| false);
        let row = bare
            .runs_for(project)
            .into_iter()
            .find(|row| row.run == run)
            .expect("restored as history");
        assert!(
            row.session.is_none(),
            "nothing to re-open, nothing to aim at"
        );
        assert!(!row.openable);

        let _ = std::fs::remove_file(&path);
    }

    // ==========================================================================================
    // Resume on the settings as they stand. The decision half (`plan_restarts`) and the exit
    // half (`plan_restart`) are both under the lock and app-free, so they are driven directly;
    // the fork is `fork_failover`'s, the same road a pool failover takes.
    // ==========================================================================================

    fn forked_on(harness: Harness, model: &str) -> cide_agents::overrides::ChildSettings {
        cide_agents::overrides::ChildSettings {
            harness,
            pool: Vec::new(),
            model: Some(model.into()),
            effort: None,
            permission_mode: None,
            providers: None,
            opencode: None,
        }
    }

    /// A claude run forked on `model`, running, and then frozen by a project pause — with a
    /// queued sibling behind it in a one-slot role, so a slot given away shows up as an
    /// admission.
    fn a_frozen_run(
        registry: &AgentRegistry,
        project: ProjectId,
        model: &str,
    ) -> (RunId, SessionId) {
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        let _waiting = registry.enqueue(spec(project, "developer", 1, 4));
        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1, "one slot, one admission");
        let session = SessionId::new();
        registry.bind_session(run, session);
        registry
            .inner
            .lock()
            .runs
            .get_mut(&run)
            .expect("the run")
            .forked_with = Some(forked_on(Harness::Claude, model));
        assert!(registry.set_state(None, run, RunState::Running));
        registry
            .take_freezes(project, None, None, 1_000)
            .expect("a project may always be paused");
        assert_eq!(
            state_of(registry, run),
            RunState::Paused {
                since_unix_ms: 1_000
            }
        );
        (run, session)
    }

    /// **A changed setting restarts the frozen run; nothing else about it moves.** The run is
    /// rebound to a fresh session before any signal, reads `Starting`, keeps its slot, and the
    /// old session carries the restart the exit will act on: a claude fork of the conversation
    /// with the continuation line as its prompt.
    #[test]
    fn a_resume_after_a_settings_change_restarts_the_frozen_run_on_a_fresh_session() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let (run, old) = a_frozen_run(&registry, project, "sonnet");

        let thaws = registry.plan_thaws(project, None).expect("frozen");
        let restarts = registry.plan_restarts(project, None, &thaws, &|_| {
            Some(forked_on(Harness::Claude, "opus"))
        });
        assert_eq!(restarts, vec![old], "the old child is what gets wound down");

        let pending = {
            let inner = registry.inner.lock();
            let live = &inner.runs[&run];
            assert_eq!(
                live.state,
                RunState::Starting,
                "a working phase across the swap"
            );
            assert!(
                live.frozen.is_none(),
                "the freeze was taken here, not by finish_thaws"
            );
            assert!(live.slot, "the slot is still this run's");
            assert_ne!(live.session, Some(old), "rebound before any signal is sent");
            assert_eq!(
                live.turns, 2,
                "a continuation line is a prompt into real work"
            );
            assert_eq!(live.note.as_deref(), Some(RESTARTING_NOTE));
            assert!(
                !inner.frozen_sessions.contains_key(&old),
                "nothing left for the shutdown thaw to find"
            );
            inner.restarts[&old].clone()
        };
        assert_eq!(pending.run, run);
        assert_eq!(pending.resume.as_deref(), Some(old.to_string().as_str()));
        assert_eq!(pending.previous, Some(old));
        assert_ne!(pending.session, Some(old));
        assert_eq!(pending.session, registry.inner.lock().runs[&run].session);
        assert_eq!(pending.why, Carried::Settings);
        assert!(
            pending.prompt.contains("new model settings"),
            "{}",
            pending.prompt
        );

        // The thaw's bookkeeping has nothing to restore on this run and nothing to watch.
        let watch = registry.finish_thaws(project, true, &thaws, 2_000);
        assert!(watch.iter().all(|(session, _)| *session != old));
        assert_eq!(state_of(&registry, run), RunState::Starting);
        assert!(
            registry.take_admissions().is_empty(),
            "the restart gave the role's one slot to the queued sibling"
        );

        // The exit half: taken exactly once, from the old child's exit.
        let forked = registry.plan_restart(old).expect("the pending restart");
        assert_eq!(forked.run, run);
        assert!(registry.plan_restart(old).is_none(), "taken, not read");
        assert!(
            registry.plan_failover(old, 1).is_none(),
            "no run owns the old session any more, so nothing else can read its exit"
        );
    }

    /// **Unchanged settings are a plain thaw** — the in-flight turn survives a short pause, which
    /// is the whole reason the comparison exists.
    #[test]
    fn a_resume_with_unchanged_settings_thaws_in_place() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let (run, old) = a_frozen_run(&registry, project, "sonnet");

        let thaws = registry.plan_thaws(project, None).expect("frozen");
        let restarts = registry.plan_restarts(project, None, &thaws, &|_| {
            Some(forked_on(Harness::Claude, "sonnet"))
        });
        assert!(restarts.is_empty());
        // A role that cannot be resolved now — deleted, or its override refused — is not a
        // change either.
        assert!(
            registry
                .plan_restarts(project, None, &thaws, &|_| None)
                .is_empty()
        );

        registry.finish_thaws(project, true, &thaws, 2_000);
        let inner = registry.inner.lock();
        let live = &inner.runs[&run];
        assert_eq!(live.state, RunState::Running, "back exactly where it was");
        assert_eq!(live.session, Some(old));
        assert!(inner.restarts.is_empty());
    }

    /// A frozen child that had not begun its turn has nothing worth continuing: the dispatch is
    /// replayed fresh on the new settings, under a new conversation.
    #[test]
    fn a_frozen_child_that_never_began_is_replayed_fresh() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 1, 4));
        registry.take_admissions();
        let old = SessionId::new();
        registry.bind_session(run, old);
        registry
            .inner
            .lock()
            .runs
            .get_mut(&run)
            .expect("the run")
            .forked_with = Some(forked_on(Harness::Claude, "sonnet"));
        assert_eq!(state_of(&registry, run), RunState::Starting);
        registry
            .take_freezes(project, None, None, 1_000)
            .expect("paused");

        let thaws = registry.plan_thaws(project, None).expect("frozen");
        let restarts = registry.plan_restarts(project, None, &thaws, &|_| {
            Some(forked_on(Harness::Claude, "opus"))
        });
        assert_eq!(restarts, vec![old]);
        let pending = registry.inner.lock().restarts[&old].clone();
        assert!(pending.resume.is_none(), "nothing to continue");
        assert_eq!(pending.prompt, "do the thing", "the dispatch, replayed");
        let inner = registry.inner.lock();
        let live = &inner.runs[&run];
        assert_eq!(live.turns, 1, "a replay is not a second prompt");
        assert_eq!(live.note.as_deref(), Some(RESTARTING_FRESH_NOTE));
    }

    /// An opencode run continues its harness conversation, and its pool is re-stamped only when
    /// the pool itself changed — an unchanged pool keeps the run's place in it.
    #[test]
    fn an_opencode_restart_continues_the_conversation_and_restamps_only_a_changed_pool() {
        let pool: Vec<cide_ipc::PoolEntry> = (0..3)
            .map(|i| pool_entry("openrouter", &format!("model-{i}")))
            .collect();
        let forked = |pool: &[cide_ipc::PoolEntry], effort: Option<&str>| {
            cide_agents::overrides::ChildSettings {
                harness: Harness::Opencode,
                pool: pool.to_vec(),
                model: None,
                effort: effort.map(str::to_string),
                permission_mode: None,
                providers: Some(Vec::new()),
                opencode: None,
            }
        };
        let frozen_on_the_pool = |registry: &AgentRegistry, project: ProjectId| {
            let run = registry.enqueue(opencode_spec(project, "developer"));
            registry.take_admissions();
            let old = SessionId::new();
            registry.bind_session(run, old);
            {
                let mut inner = registry.inner.lock();
                let live = inner.runs.get_mut(&run).expect("the run");
                live.pool = pool.clone();
                live.pool_index = 1;
                live.harness_session = Some("ses_first".into());
                live.forked_with = Some(forked(&pool, None));
            }
            assert!(registry.set_state(None, run, RunState::Running));
            registry
                .take_freezes(project, None, None, 1_000)
                .expect("paused");
            (run, old)
        };

        // The pool was edited: the successor starts at the top of the new list.
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let (run, old) = frozen_on_the_pool(&registry, project);
        let thaws = registry.plan_thaws(project, None).expect("frozen");
        let edited = vec![pool_entry("anthropic", "claude-sonnet-4-5")];
        let restarts =
            registry.plan_restarts(project, None, &thaws, &|_| Some(forked(&edited, None)));
        assert_eq!(restarts, vec![old]);
        let pending = registry.inner.lock().restarts[&old].clone();
        assert_eq!(
            pending.resume.as_deref(),
            Some("ses_first"),
            "opencode's conversation is the id it printed"
        );
        {
            let inner = registry.inner.lock();
            let live = &inner.runs[&run];
            assert_eq!(
                live.pool, edited,
                "restamped onto the current pool, which admission and the fork both read"
            );
            assert_eq!(live.pool_index, 0);
            assert_eq!(live.harness_session.as_deref(), Some("ses_first"));
        }

        // Only the effort moved: the run keeps its place on the pool it settled into.
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let (run, old) = frozen_on_the_pool(&registry, project);
        let thaws = registry.plan_thaws(project, None).expect("frozen");
        let restarts = registry.plan_restarts(project, None, &thaws, &|_| {
            Some(forked(&pool, Some("high")))
        });
        assert_eq!(restarts, vec![old]);
        let inner = registry.inner.lock();
        let live = &inner.runs[&run];
        assert_eq!(live.pool, pool);
        assert_eq!(
            live.pool_index, 1,
            "sticky: the list is the one the run was arranged on"
        );
    }

    /// A conversation a pane is driving is thawed as it stands, and the row says why (M42's
    /// rule): two harness processes must not write one transcript.
    #[test]
    fn a_conversation_open_in_a_pane_keeps_the_settings_it_started_with() {
        let registry = AgentRegistry::default();
        let sessions = SessionRegistry::default();
        let project = ProjectId::new();
        let (run, old) = a_frozen_run(&registry, project, "sonnet");

        let pty = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("sleep 30"),
        )
        .expect("spawn sh");
        sessions.insert(old, Arc::clone(&pty));
        registry.note_viewer(
            &HarnessSession {
                harness: Harness::Claude,
                id: old.to_string(),
                cwd: PathBuf::from("/p/.cide/worktrees/developer-t-1"),
            },
            old,
        );

        let thaws = registry.plan_thaws(project, None).expect("frozen");
        let restarts = registry.plan_restarts(project, Some(&sessions), &thaws, &|_| {
            Some(forked_on(Harness::Claude, "opus"))
        });
        assert!(
            restarts.is_empty(),
            "not restarted underneath the person typing into it"
        );
        {
            let inner = registry.inner.lock();
            let live = &inner.runs[&run];
            assert_eq!(live.session, Some(old));
            assert!(live.frozen.is_some(), "left for finish_thaws");
            assert!(
                live.note
                    .as_deref()
                    .is_some_and(|note| note.contains("open in a pane")),
                "{:?}",
                live.note
            );
        }
        registry.finish_thaws(project, true, &thaws, 2_000);
        assert_eq!(state_of(&registry, run), RunState::Running);

        pty.kill();
        wait_exited(&pty);
    }

    /// Stop pressed while the old child is being wound down: the successor is not forked and
    /// the run ends, releasing its slot — it would otherwise hold it for ever under a row
    /// nothing can press.
    #[test]
    fn stop_during_a_pending_restart_ends_the_run_instead_of_forking() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let (run, old) = a_frozen_run(&registry, project, "sonnet");
        let thaws = registry.plan_thaws(project, None).expect("frozen");
        registry.plan_restarts(project, None, &thaws, &|_| {
            Some(forked_on(Harness::Claude, "opus"))
        });
        registry.finish_thaws(project, true, &thaws, 2_000);

        // What `stop` does under the lock before it signals.
        registry
            .inner
            .lock()
            .runs
            .get_mut(&run)
            .expect("the run")
            .stopping = true;

        assert!(registry.plan_restart(old).is_none(), "no fork into a Stop");
        let inner = registry.inner.lock();
        let live = &inner.runs[&run];
        assert!(
            matches!(&live.state, RunState::Failed { reason } if reason.contains("stopped")),
            "{:?}",
            live.state
        );
        assert!(!live.slot, "released, so the queued sibling may start");
        assert!(inner.restarts.is_empty());
    }

    /// A restart the old exit has not yet carried out is snapshotted under the **old** session:
    /// the conversation is filed there, and a cide restart in that window must resume it, not
    /// the fresh id nothing was ever written under.
    #[test]
    fn a_pending_restart_is_snapshotted_under_the_old_session() {
        let dir = std::env::temp_dir().join(format!(
            "cide-pending-restart-{}-{}",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("agent-runs.json");

        let registry = Arc::new(AgentRegistry::default());
        let project = ProjectId::new();
        let (run, old) = a_frozen_run(&registry, project, "sonnet");
        let thaws = registry.plan_thaws(project, None).expect("frozen");
        registry.plan_restarts(project, None, &thaws, &|_| {
            Some(forked_on(Harness::Claude, "opus"))
        });
        assert_ne!(registry.inner.lock().runs[&run].session, Some(old));

        registry.write_snapshot_to(&path);
        let file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("written")).expect("json");
        let saved = file["runs"]
            .as_array()
            .expect("runs")
            .iter()
            .find(|saved| saved["run"] == serde_json::json!(run.to_string()))
            .expect("the run");
        assert_eq!(saved["session"], serde_json::json!(old.to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Interrupted road makes the same choice about a pool edited while cide was down: the
    /// restored position is into a list that no longer exists, so the resume re-stamps; an
    /// unchanged pool keeps the place the snapshot preserved.
    #[test]
    fn resuming_an_interrupted_run_restamps_a_pool_edited_while_cide_was_down() {
        let pool: Vec<cide_ipc::PoolEntry> = (0..3)
            .map(|i| pool_entry("openrouter", &format!("model-{i}")))
            .collect();
        let interrupted_on = |registry: &AgentRegistry, project: ProjectId| {
            let run = registry.enqueue(opencode_spec(project, "developer"));
            registry.take_admissions();
            registry.bind_session(run, SessionId::new());
            registry.set_state(None, run, RunState::Interrupted);
            let mut inner = registry.inner.lock();
            let live = inner.runs.get_mut(&run).expect("the run");
            live.pool = pool.clone();
            live.pool_index = 2;
            run
        };
        let settings = |pool: &[cide_ipc::PoolEntry]| cide_agents::overrides::ChildSettings {
            harness: Harness::Opencode,
            pool: pool.to_vec(),
            model: None,
            effort: None,
            permission_mode: None,
            providers: Some(Vec::new()),
            opencode: None,
        };

        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = interrupted_on(&registry, project);
        let edited = vec![pool_entry("anthropic", "claude-sonnet-4-5")];
        registry
            .requeue_interrupted(project, None, None, &|_| Some(settings(&edited)))
            .expect("requeued");
        assert_eq!(state_of(&registry, run), RunState::Queued);
        assert_eq!(registry.inner.lock().runs[&run].pool, edited);
        assert_eq!(registry.inner.lock().runs[&run].pool_index, 0);

        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let run = interrupted_on(&registry, project);
        registry
            .requeue_interrupted(project, None, None, &|_| Some(settings(&pool)))
            .expect("requeued");
        assert_eq!(registry.inner.lock().runs[&run].pool, pool);
        assert_eq!(registry.inner.lock().runs[&run].pool_index, 2);
    }

    /// The two openings name different events, because the model is about to reconcile a
    /// conversation with a tree and "cide restarted" on a machine that never went down is a
    /// false lead.
    #[test]
    fn the_continuation_line_says_which_kind_of_restart_it_was() {
        let task = TaskId("t-7".into());
        let after_cide = continuation_prompt(Some(&task), Some("Tabs"), None, Restarted::Cide);
        assert!(
            after_cide.starts_with("cide restarted while you were working."),
            "{after_cide}"
        );
        let after_settings =
            continuation_prompt(Some(&task), Some("Tabs"), None, Restarted::Settings);
        assert!(
            after_settings.contains("new model settings"),
            "{after_settings}"
        );
        for line in [&after_cide, &after_settings] {
            assert!(line.contains("t-7 (Tabs)"), "{line}");
            assert!(line.contains("git status"), "{line}");
        }
    }
}
