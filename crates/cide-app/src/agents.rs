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
    /// The directory the child was started in — the worktree, or the root under shared
    /// isolation — once it has been. `None` while queued: nothing has forked, so nothing can
    /// have connected to the agent socket and asked. (M39)
    ///
    /// Not on [`cide_ipc::AgentRun`]: the panel has no use for an absolute path, and the one
    /// consumer is `agent_rpc`, which resolves a relative path in `cide_task_attach` against
    /// it. [`Self::note`] carries the same fact as a sentence for a person; this is the value.
    cwd: Option<std::path::PathBuf>,
    /// The ordered candidates this run may fall down, stamped at its **first fork**. (M45)
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
    /// **Monotonic**: `0` at the first fork, `+= 1` on a qualifying provider failure, and never
    /// decreased by anything. That single property is the whole of "sticky for the run" *and* the
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
    stopping: bool,
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
    pub code: Option<i32>,
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
        Harness::Opencode | Harness::Codex => live.harness_session.clone(),
    }?;
    Some(HarnessSession {
        harness: live.harness,
        id,
        cwd: cwd.to_path_buf(),
    })
}

/// The pane session driving `live`'s conversation right now, if a person has one open and its
/// child is still alive in `sessions`. See [`Inner::viewers`]. (M42)
fn viewed_by(inner: &Inner, sessions: &SessionRegistry, live: &LiveRun) -> Option<SessionId> {
    let id = match live.harness {
        Harness::Claude | Harness::Qwen => live.session?.to_string(),
        Harness::Opencode | Harness::Codex => live.harness_session.clone()?,
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

    fn wire(&self) -> AgentRun {
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
            notify: self.notify.clone(),
            stale_turn: self.stale_turn,
            // The pool's sentence and the run's, in that order: which model this run is actually
            // on outranks which worktree it holds. Composed here rather than by overwriting
            // `note`, because `note_cwd` rewrites that field whenever a child comes up.
            note: match (&self.pool_note, &self.note) {
                (Some(pool), Some(note)) => Some(format!("{pool} — {note}")),
                (Some(pool), None) => Some(pool.clone()),
                (None, note) => note.clone(),
            },
            openable: self.session.is_some() || self.reopenable,
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
    /// Put a run on its agent's queue and answer with its id.
    ///
    /// **Never spawns and never blocks**, which is this whole module's version of the
    /// `openDiff` rule: that tool blocking an agent's turn is a documented invariant precisely
    /// because it is dangerous, and a dispatch that waited on the run would let one wedged
    /// subagent freeze whoever called it — including the orchestrating session, over the MCP
    /// socket, in the middle of its own turn.
    pub fn enqueue(&self, spec: DispatchSpec) -> RunId {
        let run = RunId::new();
        let mut inner = self.inner.lock();
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
                prompt: spec.prompt,
                note: (ahead > 0).then(|| {
                    format!("{ahead} ahead of it in this role's queue; runs start as slots free")
                }),
                // Stamped at the first fork, not here: resolving needs the settings and the
                // overrides, which `facts` reads on the forking thread. See `LiveRun::pool`.
                pool: Vec::new(),
                pool_index: 0,
                provider_failure: None,
                spent: Vec::new(),
                pool_note: None,
                forked_with: None,
                stopping: false,
                // The opening prompt is turn one, so a provider failure before any answer is a
                // first-turn failure — the case that can abandon its conversation for free.
                turns: 1,
                slot: false,
                agent_limit: spec.agent_limit.max(1),
                project_limit: spec.project_limit.max(1),
                checkout: spec.checkout,
                notify: spec.notify,
                cwd: None,
                seq,
                frozen: None,
                stale_turn: false,
                harness_session: None,
                continuing: false,
                death_noted: false,
                reopenable: false,
            },
        );
        inner.queues.entry(key).or_default().push_back(run);
        run
    }

    /// Take every queued run that may start now, marking each `Starting` and holding its slot.
    ///
    /// The admission decision and the slot it takes happen under one lock, which is what makes
    /// two concurrent pumps safe: whichever gets the lock first takes the slot, and the second
    /// sees a full one.
    fn take_admissions(&self) -> Vec<Admission> {
        let mut inner = self.inner.lock();
        let mut admitted = Vec::new();

        // A pass admits at most one run per agent, because only the *front* of an agent's queue
        // is ever eligible — that is what makes a queue serial. So it repeats until a pass admits
        // nothing: under `Isolation::Shared` a role may legitimately hold several slots, and a
        // single pass would start one of three and leave the other two waiting for an unrelated
        // event to call this again.
        loop {
            let before = admitted.len();
            admit_a_pass(&mut inner, &mut admitted);
            if admitted.len() == before {
                break;
            }
        }

        admitted
    }
}

/// One pass of the admission scan: at most one run per agent, oldest dispatch first.
fn admit_a_pass(inner: &mut Inner, admitted: &mut Vec<Admission>) {
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
                        && matches!(
                            other.state,
                            RunState::Starting
                                | RunState::Running
                                | RunState::AwaitingPermission
                                | RunState::Paused { .. }
                        )
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
            live.state = RunState::Starting;
            live.slot = true;
            live.note = None;
            // A resumed interrupted run continues its old conversation; one that never had a
            // conversation to continue (it was still queued when cide quit) starts fresh with
            // its original prompt, which is the honest reading of "nothing happened yet".
            let resume = match live.continuing {
                true => resume_point(live),
                false => None,
            };
            live.continuing = false;
            let prompt = match resume.is_some() {
                true => continuation_prompt(
                    live.task.as_ref(),
                    live.task_title.as_deref(),
                    // Both facts the sentence needs: whether a bridge will be attached at all,
                    // and what this run's CLI calls the tool. `start_child` refuses a bridgeless
                    // run outright, so the `None` arm is a belt-and-braces answer rather than a
                    // state a user reaches.
                    cide_hook_binary().and_then(|_| cide_agents::harness::for_kind(live.harness)),
                    Restarted::Cide,
                ),
                false => live.prompt.clone(),
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
            });
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
        Harness::Opencode | Harness::Codex => {
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

/// Clear a run's pool stamp so its next fork stamps the pool as configured now. The one place
/// the stickiness rule on [`LiveRun::pool`] yields, and only to a person's Resume — never to a
/// failover or a follow-up, which continue on the list the run was arranged on.
fn restamp(live: &mut LiveRun) {
    live.pool.clear();
    live.pool_index = 0;
    live.spent.clear();
    live.pool_note = None;
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
        live.state = next;
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

    /// Whether any run of `agent` on `task` is still open — queued, starting, live, idle or
    /// paused. Only `Finished` and `Failed` close a run for this question.
    ///
    /// The auto-dispatch dedupe (see [`crate::task_triggers`]): a repeated assignment or a
    /// second mention of a role while its run lives must not stack a second queue entry for the
    /// same pair. `Idle` counts as open on purpose — an idle run still holds a live child in the
    /// role's only worktree, and the way to re-engage it is a retry or a prompt, not a duplicate
    /// run queued behind it.
    pub fn has_open_run(&self, project: ProjectId, agent: &AgentId, task: &TaskId) -> bool {
        self.inner.lock().runs.values().any(|run| {
            run.project == project
                && &run.agent == agent
                && run.task.as_ref() == Some(task)
                && !matches!(
                    run.state,
                    RunState::Finished { .. } | RunState::Failed { .. }
                )
        })
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
        let code = match live.state {
            RunState::Failed { .. } => None,
            RunState::Finished { code } if code != 0 => Some(code),
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
                Harness::Opencode | Harness::Codex => true,
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
                Harness::Opencode | Harness::Codex => {
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
    live.state = RunState::Paused {
        since_unix_ms: now_ms,
    };
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
            live.state = frozen.state.clone();
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
            live.state = RunState::Starting;
            if pool_changed {
                restamp(live);
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
            live.state = RunState::Queued;
            live.continuing = true;
            live.note = Some(
                "resuming after a cide restart; a new child continues the same conversation"
                    .to_string(),
            );
            if !live.pool.is_empty()
                && settings_now(&live.agent).is_some_and(|now| now.pool != live.pool)
            {
                restamp(live);
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
    fn plan_failover(&self, session: SessionId, code: i32) -> Option<Failover> {
        let mut inner = self.inner.lock();
        let live = inner
            .runs
            .values_mut()
            .find(|live| live.session == Some(session))?;

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
        if live.stopping
            || self.snapshot_sealed.load(Ordering::SeqCst)
            || !matches!(live.state, RunState::Starting | RunState::Running)
            || code == 0
        {
            return None;
        }

        let spent = live.pool.get(live.pool_index).cloned();
        let next = live.pool_index + 1;
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
        // **Monotonic**, and that is both the stickiness and the loop guard: a run fails over at
        // most `pool.len() - 1` times for its entire life.
        live.pool_index = next;
        debug_assert!(live.pool_index < live.pool.len());
        live.pool_note = Some(format!(
            "{} {} — now on {} ({} of {})",
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
        live.state = RunState::Starting;

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
        let fresh = live.turns <= 1;
        let resume = match fresh {
            true => {
                live.harness_session = None;
                None
            }
            false => live.harness_session.clone(),
        };

        Some(Failover {
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
        })
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
        live.forked_with = Some(settings);
        if live.pool.is_empty() && live.pool_index == 0 {
            live.pool.clone_from(&resolved.pool);
            if let Some(name) = &resolved.pool_name
                && !resolved.pool.is_empty()
            {
                live.pool_note = Some(format!(
                    "pool {name}: {} of {}",
                    live.pool_index + 1,
                    resolved.pool.len()
                ));
            }
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
        let file = {
            let inner = self.inner.lock();
            let mut paused: Vec<ProjectId> = inner.paused_projects.iter().copied().collect();
            paused.sort();
            // Terminal runs ride along — the panel's tail is a history, and a history that
            // ends at every restart is a scratchpad. [`Self::forget_old`] has already capped
            // them at [`RECENT_KEPT`] per project, so this is bounded by construction.
            let mut runs: Vec<&LiveRun> = inner.runs.values().collect();
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
                        agent_limit: live.agent_limit,
                        project_limit: live.project_limit,
                        pool: live.pool.clone(),
                        pool_index: live.pool_index,
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
                Harness::Opencode | Harness::Codex => saved.harness_session.is_some(),
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
                    // Not persisted. A restored run's next child is a resume that re-reads the
                    // settings and stamps this afresh; there is no frozen child whose argv this
                    // could be compared against.
                    forked_with: None,
                    stopping: false,
                    // A resumed run has had at least one prompt, so its next failure is a
                    // mid-conversation one and must not abandon the transcript.
                    turns: 1,
                    state,
                    started_unix_ms: saved.started_unix_ms,
                    prompt: saved.prompt,
                    note,
                    slot: false,
                    agent_limit: saved.agent_limit.max(1),
                    project_limit: saved.project_limit.max(1),
                    checkout: saved.checkout,
                    notify: saved.notify,
                    // A restored run has no child until it is resumed, and the resume calls
                    // `note_cwd` like a first start does.
                    cwd: None,
                    seq,
                    frozen: None,
                    stale_turn: false,
                    harness_session: saved.harness_session,
                    continuing: false,
                    death_noted: false,
                    reopenable,
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
        let dir = screens_dir();
        if let Err(error) = std::fs::create_dir_all(&dir) {
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
        if let Ok(entries) = std::fs::read_dir(&dir) {
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
    /// The providers and pools this fork's child is configured with. (M45)
    ///
    /// Read here with everything else, on the thread that already holds the lock, because a
    /// provider edited under a queued run must not change what that run is already doing — the
    /// rule `skip_permissions` states beside the `RunPlan` literal below.
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
    let (root, theme, proxy, claude, llm) = state.with(|ws| {
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
        // Read after the workspace lock is released — it is a file, and `facts` holds that lock
        // for as short a time as it can.
        overrides: cide_core::persist::load_agent_overrides(
            &cide_core::persist::agent_overrides_path(),
        )
        .project(&root_key),
        llm,
        opencode: std::cell::OnceCell::new(),
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
    /// opencode's resolved configuration for this project, read once. See the field.
    fn opencode(&self) -> Option<&cide_agents::harness::opencode::UserConfig> {
        self.opencode
            .get_or_init(|| match cide_agents::harness::opencode::user_config(&self.root) {
                Ok(config) => Some(config),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "opencode could not say what it resolves as this project's configuration; \
                         its children will run on whatever it picks, and a Resume cannot compare it"
                    );
                    None
                }
            })
            .as_ref()
    }

    /// The role as it resolves here, with opencode's own default folded in where cide names no
    /// model — `Resolved::with_default_model`, fed from the one place that can read it.
    fn resolve(&self, agent: &cide_agents::LoadedAgent) -> cide_agents::overrides::Resolved {
        let resolved = cide_agents::overrides::resolve(agent, &self.overrides, &self.llm);
        match resolved.harness {
            Harness::Opencode => {
                let default = self.opencode().and_then(|config| config.model.clone());
                resolved.with_default_model(default)
            }
            Harness::Claude | Harness::Codex | Harness::Qwen => resolved,
        }
    }

    /// What a child of `resolved` is forked with, for the record every fork keeps and the
    /// comparison a Resume makes. See `LiveRun::forked_with`.
    fn child_settings(
        &self,
        resolved: &cide_agents::overrides::Resolved,
    ) -> cide_agents::overrides::ChildSettings {
        let opencode = match resolved.harness {
            Harness::Opencode => self.opencode(),
            Harness::Claude | Harness::Codex | Harness::Qwen => None,
        };
        resolved.child_settings(&self.llm, opencode)
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
        for admission in self.take_admissions() {
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
        if let Some(opening) = started.opening {
            type_submitted_line(&app, session_id, &started.session, opening);
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
            if let Some(failover) = registry.plan_failover(session, exit.code) {
                over_to(&registry, failover);
                return;
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
            // Kept before it is rendered and only when the harness says a person may want it
            // whole — `json_log_render`'s rule, one hook over. The raw line is what the card
            // shows; the rendering is what the pane shows.
            let handle = keep(line)
                .then(|| ring.as_ref().map(|ring| ring.record(session, line.trim())))
                .flatten();
            render(&mut state.lock(), line, handle)
        }))
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
    fn project_of(&self, run: RunId) -> Option<ProjectId> {
        self.inner.lock().runs.get(&run).map(|live| live.project)
    }

    /// Stop a run: cancel it if it is queued, kill its child if it has one.
    ///
    /// Idempotent on a run that has already ended, deliberately — a second press of a Stop button
    /// on a row that finished while the pointer was travelling is not an error to show anybody.
    pub fn stop(self: &Arc<Self>, app: &AppHandle, project: ProjectId, run: RunId) -> Result<()> {
        let (state, session) = {
            let mut inner = self.inner.lock();
            let live = inner
                .runs
                .get_mut(&run)
                .filter(|live| live.project == project)
                .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
            // Latched **before** any signal, so the exit this is about to cause cannot be read as
            // a provider's fault and spend a pool candidate restarting the run underneath the
            // person who just pressed Stop. (M45)
            live.stopping = true;
            (live.state.clone(), live.session)
        };

        match state {
            RunState::Queued => {
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
            }
            RunState::Finished { .. } | RunState::Failed { .. } => {}
            // No child to kill and no reaper to report one, so the fall-through below would
            // move nothing and the row would be unstoppable, silently. Stopping an interrupted
            // run means "do not resume this": the row closes into Recent, the conversation
            // stays on disk, and a fresh dispatch remains available.
            RunState::Interrupted => {
                self.fail(Some(app), run, "discarded without resuming");
            }
            _ => {
                // The child, through the one registry that owns it. The run's own state moves
                // when the reaper reports, not here: a state written on the way *into* a kill
                // would be a claim about a process that is still running.
                if let (Some(session), Some(sessions)) =
                    (session, app.try_state::<SessionRegistry>())
                    && let Some(pty) = sessions.get(session)
                {
                    pty.kill();
                }
            }
        }

        self.mark_changed(app, project);
        self.pump(app);
        Ok(())
    }

    /// Note that this project's roster changed. Coalesced; see the module header.
    pub fn mark_changed(self: &Arc<Self>, app: &AppHandle, project: ProjectId) {
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
                    crate::emit::agents_changed(&app, project, &roster);
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
    let agent = project.get(&admission.agent).ok_or_else(|| {
        CoreError::Io(format!(
            "no role named `{}` in this project",
            admission.agent
        ))
    })?;
    if let Some(why) =
        cide_agents::dispatch_refusal(agent, &project.config.agents, facts.hook_bin.as_deref())
    {
        return Err(CoreError::Io(why));
    }

    // Where the run stands. `run_checkout` is the one rule, shared with `plan_dispatch`'s stamp
    // so the admission gate and this fork cannot disagree. Read off the same fresh `project.get`
    // as the refusal, so a role's `worktree: false` flipped mid-queue is honoured at the fork —
    // and a run with no task lands in the project root under every isolation (M40).
    let cwd = match cide_agents::run_checkout(
        agent,
        &project.config.agents,
        admission.task.as_ref(),
    ) {
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
        None => facts.root.clone(),
    };

    // What this role actually runs as *here*: the committed definition folded with this
    // machine's local override. Pure, and computed before anything is forked or written, so its
    // one refusal costs nothing when it fires. (M45)
    let resolved = facts.resolve(agent);
    // The only refusal this fold produces: an override naming a pool that is not configured
    // here. Refused rather than degraded, because falling back would answer with a model nobody
    // chose and bill for it — and people build a pool to *cap* spend. See `overrides::resolve`.
    if let Some(refusal) = resolved.refusal {
        return Err(CoreError::Io(refusal));
    }
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
        // A headless run has no pane and therefore no measurement. `PaneRestore`'s "only the
        // frontend knows a pane's size" is why the default is written down as a decision: a run
        // later opened into a pane is resized then, through the path a re-docked pane uses.
        geometry: Geometry::default(),
        claude: facts.claude.clone(),
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
        // From the same fresh `load_project` the refusal above used, so the flag a child runs
        // with is the file as it stood at this fork — a `git checkout` flipping
        // `agents.skipPermissions` is honoured from the next dispatch, never cached past it.
        skip_permissions: project.config.agents.skip_permissions,
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
    // A resumed run's pane opens onto what the old child last showed, above the continuation —
    // the same mechanism a restored shell pane uses (`SpawnSpec::preload`: mirror only, never
    // the child), fed from the screen the last teardown saved. Absent file, absent preload:
    // a crash keeps the runs and loses the screens, and blank-above-continuation is honest.
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
            .and_then(|_| saved_screen(admission.run))
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
    let (text, enter) = strip_enter(bytes);
    pty.write(text);
    if !enter {
        return;
    }

    let app = app.clone();
    let for_thread = Arc::clone(pty);
    let spawned = std::thread::Builder::new()
        .name("cide-type-enter".into())
        .spawn(move || {
            let deadline = Instant::now() + ENTER_BOOT_DEADLINE;
            while Instant::now() < deadline {
                let state = app
                    .try_state::<crate::hooks::HookServer>()
                    .map(|hooks| hooks.state(session));
                if state != Some(SessionState::Spawning) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            std::thread::sleep(ENTER_GAP);
            for_thread.write(b"\r".to_vec());
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
        }
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
        }
    }

    /// A run admitted, forked, and standing on the first of `n` candidates.
    fn run_on_a_pool(registry: &AgentRegistry, n: usize) -> (ProjectId, RunId, SessionId) {
        let project = ProjectId::new();
        let run = registry.enqueue(spec(project, "developer", 4, 4));
        let mut admitted = Vec::new();
        admit_a_pass(&mut registry.inner.lock(), &mut admitted);
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

    /// The auto-dispatch dedupe: a `(agent, task)` pair with a run in any not-yet-closed state
    /// answers open, so a repeated assignment or mention stacks nothing — and only `Finished`/
    /// `Failed` reopen it.
    #[test]
    fn an_open_run_is_any_run_that_has_not_ended() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let task = TaskId("t-7".into());
        let mut with_task = spec(project, "developer", 1, 4);
        with_task.task = Some(task.clone());
        let run = registry.enqueue(with_task);
        let agent = AgentId("developer".into());

        // Queued counts: the whole point is not stacking a second entry behind it.
        assert!(registry.has_open_run(project, &agent, &task));
        // A different task, and a different role, do not.
        assert!(!registry.has_open_run(project, &agent, &TaskId("t-8".into())));
        assert!(!registry.has_open_run(project, &AgentId("qa".into()), &task));

        // Idle still counts — the child lives and holds the role's worktree.
        let _ = registry.take_admissions();
        registry.set_state(None, run, RunState::Idle);
        assert!(registry.has_open_run(project, &agent, &task));

        // Failed closes it: the pair may be dispatched again.
        registry.fail(None, run, "spawn failed");
        assert!(!registry.has_open_run(project, &agent, &task));
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
        registry.watch_exit_with(session, &pty, |_, _| {}, |_, _| {});

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
            assert!(
                live.pool.is_empty(),
                "cleared, so the fork stamps the current pool"
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
        assert!(registry.inner.lock().runs[&run].pool.is_empty());
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
