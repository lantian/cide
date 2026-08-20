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
//! # Concurrency and the worktree are the same lock
//!
//! With [`Isolation::Worktree`] an agent has exactly **one** checkout — `.cide/worktrees/<agent>`
//! on branch `cide/<agent>` — so a second concurrent run of that role would be a second process
//! editing one checkout with nothing arbitrating between them.
//! [`cide_agents::effective_max_concurrent`] therefore clamps a role's own `max-concurrent` to 1,
//! and this module *records* that clamped number on the run at dispatch rather than recomputing
//! it later: see [`LiveRun::agent_limit`]. A dispatch for a busy agent **queues**; it does not
//! spawn.
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cide_agents::{Delivery, Isolation, Observation, RunPlan, SessionBinding};
use cide_claude::HookFrame;
use cide_core::CoreError;
use cide_ipc::{
    AgentId, AgentRun, ClaudeSettings, Geometry, Harness, PaneRole, ProjectId, ProxySettings,
    RunId, RunState, SessionId, SessionState, TaskId, Theme,
};
use cide_pty::{PtySession, SinkId};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

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
}

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
            stale_turn: self.stale_turn,
            note: self.note.clone(),
        }
    }
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
    pub prompt: String,
    pub agent_limit: u16,
    pub project_limit: u16,
}

/// One run the queue has decided to start. Produced under the lock, acted on outside it.
#[derive(Debug, Clone)]
struct Admission {
    run: RunId,
    project: ProjectId,
    agent: AgentId,
    task: Option<TaskId>,
    task_title: Option<String>,
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
                state: RunState::Queued,
                started_unix_ms: now_unix_ms(),
                prompt: spec.prompt,
                note: (ahead > 0).then(|| {
                    format!(
                        "{ahead} ahead of it in this role's queue; a role runs one task at a time"
                    )
                }),
                slot: false,
                agent_limit: spec.agent_limit.max(1),
                project_limit: spec.project_limit.max(1),
                seq,
                frozen: None,
                stale_turn: false,
                harness_session: None,
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

            inner.agent_slots.insert(key.clone(), agent_held + 1);
            inner.project_slots.insert(project, project_held + 1);
            if let Some(queue) = inner.queues.get_mut(&key) {
                queue.pop_front();
            }
            let live = inner.runs.get_mut(&run).expect("just read above");
            live.state = RunState::Starting;
            live.slot = true;
            live.note = None;
            admitted.push(Admission {
                run,
                project,
                agent: key.1,
                task: live.task.clone(),
                task_title: live.task_title.clone(),
                prompt: live.prompt.clone(),
            });
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

    /// Note which directory a run is working in, for the row.
    fn note_cwd(&self, run: RunId, cwd: &std::path::Path) {
        if let Some(live) = self.inner.lock().runs.get_mut(&run) {
            live.note = Some(cwd.display().to_string());
        }
    }

    /// Move a run to a new state, releasing its slot if this is the transition that ends its
    /// claim on the agent's worktree. Answers whether anything changed.
    ///
    /// The release rule is the module header's, and the *edge* is the whole of it: only a
    /// transition out of `Running`/`AwaitingPermission` into `Idle` counts as "the turn was
    /// handed back". `Starting → Idle` is the `SessionStart` window and releases nothing.
    ///
    /// # That same edge is the product-owner loop's only trigger
    ///
    /// A run reports back through `.cide/tasks.json` and nothing watches that file on the
    /// orchestrator's behalf, so the moment a turn is handed back is the moment the project's
    /// primary session has something new to read. [`crate::agent_rpc::note_run_idle`] is called
    /// from **here** and from nowhere else, precisely because this is the only place in the
    /// process that can see a *transition* rather than a state: an idle run is observed over and
    /// over — every statusline frame is an observation — and a nudge per observation would be a
    /// typed prompt per observation. The edge happens once per turn.
    ///
    /// # `app: Option<&AppHandle>`, and what the `None` arm costs
    ///
    /// `None` means *this caller cannot nudge*, and there is exactly one such caller:
    /// [`Self::watch_exit_with`], which is deliberately app-free so that a real child's exit can
    /// be driven by a test — `tauri`'s mock app is behind a feature this build does not enable.
    /// It costs nothing, and that is a fact about the state machine rather than a hope:
    /// [`Observation::Exit`] is the sole producer of `RunState::Finished` and produces nothing
    /// else, so an exit can never *be* the handed-back edge. Every path that can reach the edge —
    /// a hook frame through [`note_hook`] — carries an app.
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
            if handed_back {
                nudge = Some(project);
            }
            if held {
                release(&mut inner, &key, project);
            }
        }
        drop(inner);
        self.forget_old();
        if let (Some(app), Some(project)) = (app, nudge) {
            crate::agent_rpc::note_run_idle(app, project, run);
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
    fn observe(
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
    pub fn runs_for(&self, project: ProjectId) -> Vec<AgentRun> {
        let inner = self.inner.lock();
        let mut runs: Vec<&LiveRun> = inner
            .runs
            .values()
            .filter(|run| run.project == project)
            .collect();
        runs.sort_by_key(|run| (group_rank(&run.state), run.seq));
        runs.iter().map(|run| run.wire()).collect()
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

    /// The live session behind a run, for a caller that wants to attach to it.
    pub fn session_of(&self, project: ProjectId, run: RunId) -> Result<SessionId> {
        let inner = self.inner.lock();
        let live = inner.runs.get(&run).filter(|live| live.project == project);
        // `CoreError::Io` because there is no tagged variant for "no such run" and adding one is
        // a `cide-core` change outside this slice — the same trade `cmd::agents::worktree_refusal`
        // takes, and for the same reason: nothing branches on the tag, the sentence is the answer.
        let live =
            live.ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
        live.session.ok_or_else(|| {
            CoreError::Io(
                "this run has not started yet, so there is no conversation to open. A queued run \
                 has no child — it is waiting for its role's worktree."
                    .into(),
            )
        })
    }

    /// Sessions of this agent's runs that are idle with a child still alive.
    ///
    /// The queue's answer to a real hazard: an `Idle` run has released its slot but its `claude`
    /// is still sitting in the role's only worktree, so starting the next run there would put two
    /// processes in one checkout — exactly what the isolation exists to prevent.
    /// [`cide_ipc::RunState::Idle`]'s doc names the two ways out ("delivers the next turn into it
    /// or winds it down first"); this is the wind-down, taken only at the moment the checkout is
    /// actually needed, so an idle run that nothing is queued behind stays open to be read.
    fn idle_children_of(
        &self,
        project: ProjectId,
        agent: &AgentId,
        except: RunId,
    ) -> Vec<SessionId> {
        let inner = self.inner.lock();
        inner
            .runs
            .values()
            .filter(|run| {
                run.project == project
                    && &run.agent == agent
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
    pub fn resume(
        self: &Arc<Self>,
        app: &AppHandle,
        project: ProjectId,
        run: Option<RunId>,
    ) -> Result<()> {
        let thaws = self.plan_thaws(project, run)?;

        for thaw in &thaws {
            signal_session(app, thaw.session, PauseSignal::Cont);
        }

        let watch = self.finish_thaws(project, run.is_none(), &thaws, now_unix_ms());

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
                pty.write(bytes);
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
        let inner = self.inner.lock();
        let live = inner
            .runs
            .get(&run)
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
        Ok(Continuation {
            // **The same `RunId`.** Not a new one, which is what makes this a follow-up rather
            // than a second dispatch — see the four rules on `respawn`.
            admission: Admission {
                run,
                project,
                agent: live.agent.clone(),
                task: live.task.clone(),
                task_title: live.task_title.clone(),
                prompt: prompt.to_string(),
            },
            previous: live.session,
            harness_session,
        })
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
                .bring_up(app, admission, session, Some(harness_session))
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

    /// `SIGCONT` every session this registry froze, so the shutdown ladder's rungs can land.
    ///
    /// **Called from `lifecycle::shutdown` before the ladder, and it is not optional.** A
    /// `SIGSTOP`ped process does not *act* on SIGHUP or SIGTERM — they go pending — so the ladder
    /// would spend its whole `hup_grace + term_grace` on a child that cannot answer and then
    /// SIGKILL it, which is exactly the half-written transcript `lifecycle.rs`'s header says the
    /// ladder exists to prevent. One signal per paused session buys back both rungs.
    ///
    /// This is also why **pause does not survive a restart**, which is stated rather than
    /// attempted: the ladder ends the process either way, and a stopped-then-killed child is just
    /// a killed child. Nothing about a freeze is written to disk.
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

/// Which of the panel's groups a state belongs to. See `AgentRoster::Ready::runs`.
fn group_rank(state: &RunState) -> u8 {
    match state {
        RunState::Running | RunState::AwaitingPermission => 0,
        RunState::Starting => 1,
        RunState::Paused { .. } => 2,
        RunState::Idle => 3,
        RunState::Queued => 4,
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
    hook_bin: Option<PathBuf>,
    hook_sock: Option<PathBuf>,
    agent_sock: Option<PathBuf>,
}

/// Read the workspace and the two sockets. One lock acquisition, released before anything forks.
fn facts(app: &AppHandle, project: ProjectId) -> Result<Facts> {
    let state = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .ok_or_else(|| CoreError::Io("the workspace is not available".into()))?;
    let (root, theme, proxy, claude) = state.with(|ws| {
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
        ))
    })?;

    Ok(Facts {
        root,
        theme,
        proxy,
        claude,
        hook_bin: cide_hook_binary(),
        hook_sock: app
            .try_state::<crate::hooks::HookServer>()
            .map(|server| server.socket().to_path_buf()),
        agent_sock: app
            .try_state::<crate::agent_rpc::AgentRpcServer>()
            .map(|server| server.socket().to_path_buf()),
    })
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
fn cide_hook_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let hook = exe.parent()?.join("cide-hook");
    hook.exists().then_some(hook)
}

/// What a successful start produced.
struct Started {
    session: Arc<PtySession>,
    opening: Option<Vec<u8>>,
    cwd: PathBuf,
}

/// How many bytes of a single unterminated output line [`AgentRegistry::watch_stream`] will hold.
///
/// Generous rather than tight, because one legitimate event line is not small: a `tool_use` event
/// carries the tool's own state, and a file read is inside it. The cap is not a size limit on
/// what a run may print — it is the answer to a child that prints a megabyte with **no newline in
/// it at all**, which would otherwise grow this buffer for the life of the run. Over the cap the
/// buffer is dropped and the scanner resynchronises at the next newline, losing the one line it
/// was in the middle of.
const LINE_CAP: usize = 1024 * 1024;

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
        let session_id = SessionId::new();
        self.bind_session(admission.run, session_id);
        self.bring_up(app, admission, session_id, None).await;
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
    ) {
        let run = admission.run;
        let project = admission.project;

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
        // `HarnessSpawn::opening`.
        if let Some(opening) = started.opening {
            started.session.write(opening);
        }

        self.note_cwd(run, &started.cwd);
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
        self.watch_exit_with(session, pty, move |registry, run| {
            after_transition(&app, registry, run)
        });
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
    ) {
        let registry = Arc::clone(self);
        pty.on_exit(move |exit| {
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

    /// Read one run's output as the harness-bound channel it is: the id, then the state.
    ///
    /// # Why this exists at all, and only for some runs
    ///
    /// A `claude` run reports through the hook socket: `cide-hook` echoes `CIDE_SESSION` back on
    /// ten hook points, `note_hook` applies them, and nothing has to read a byte of the child's
    /// output. `opencode` has no hooks and no `--settings`, so **its `--format json` stream is
    /// the only channel it has**: without this sink an opencode run's row sits at `Starting`
    /// until the process dies, and a follow-up has no conversation id to name.
    ///
    /// So this is attached exactly when the harness said its identity arrives that way
    /// ([`SessionBinding::Harness`]), and a claude run pays nothing for it — no sink, no parse,
    /// no allocation.
    ///
    /// # What it owes the session it attaches to
    ///
    /// **Every byte is acknowledged, and never from this thread.** `cide-pty` gives each sink a
    /// credit budget, and a sink that never pays it back is choked at 256 KiB — after which it
    /// stops receiving *output* and starts receiving rendered screens, which is right for a pane
    /// catching up and fatal for a scanner. There is no backpressure to apply here anyway; the
    /// work is one pass over a byte slice. But the acknowledgement cannot be made here either,
    /// and [`ack_credit`] carries that whole argument: it is a re-entrant lock that parks the
    /// coalescer for the entire process.
    ///
    /// It holds a [`std::sync::Weak`] to the session rather than an `Arc`, because an `Arc` here
    /// would be a cycle — the session owns the sink list, the sink would own the session — and
    /// the run's child would never be dropped.
    ///
    /// # `app: Option<&AppHandle>`
    ///
    /// [`Self::set_state`]'s convention, with [`Self::watch_exit_with`]'s motive: `None` means
    /// *this caller cannot nudge or admit*, and the only such caller is a test — `tauri`'s mock
    /// app is behind a feature this build does not enable, so a sink that required one could be
    /// exercised only by running the application.
    ///
    /// Production passes `Some`, and that is not a detail either: the `Running → Idle` edge this
    /// sink produces **is** the product-owner nudge for a harness with no hooks, so a `None` here
    /// in earnest would be an opencode run that finishes its turn with nothing typed into the
    /// orchestrator's console and nothing started off its queue.
    fn watch_stream(
        self: &Arc<Self>,
        app: Option<&AppHandle>,
        run: RunId,
        session: SessionId,
        pty: &Arc<PtySession>,
        capture: fn(&str) -> Option<String>,
    ) {
        /// The partial line one sink is carrying between deliveries.
        ///
        /// **Only the buffer is behind the mutex.** Whether the id has been captured is an
        /// [`AtomicBool`] beside it rather than a third field here, and that is a bug fix rather
        /// than a preference: reading it inside an `if` whose body then writes it takes this
        /// lock twice, and a `parking_lot::Mutex` is not reentrant — the second acquisition
        /// deadlocks **`cide-pty`'s coalescer thread**, which is the thread every byte of every
        /// session in the process flows through. Nothing anywhere reports that; every terminal
        /// in the application simply stops painting.
        #[derive(Default)]
        struct Scan {
            /// Bytes of a line that has not ended yet.
            buffer: Vec<u8>,
            /// Dropping the remains of a line that went past [`LINE_CAP`], until a newline.
            resyncing: bool,
        }

        let registry = Arc::clone(self);
        let app = app.cloned();
        // See the doc: an `Arc` would be a cycle through the session's own sink list.
        let weak = Arc::downgrade(pty);
        // The id is minted by `attach`, which cannot be called until the sink exists, so the
        // first delivery may land before this is set. That costs one unacknowledged chunk out of
        // a 256 KiB budget.
        let id: Arc<std::sync::OnceLock<cide_pty::SinkId>> = Arc::new(std::sync::OnceLock::new());
        let for_sink = Arc::clone(&id);
        let scan = Mutex::new(Scan::default());
        // A one-way latch, so the scan stops parsing for an id once it has one.
        let captured = AtomicBool::new(false);

        let attached = pty.attach(Arc::new(move |bytes: &[u8]| -> bool {
            // **Never `pty.ack(..)` from here.** See `ack_credit`: this callback runs with
            // `cide-pty`'s sink list locked, and re-entering that lock parks the coalescer.
            if let Some(id) = for_sink.get().copied() {
                ack_credit(&weak, id, bytes.len());
            }

            // The split happens under the lock; everything that can block — a registry lock, an
            // emit, a nudge that writes into another PTY — happens after it is dropped. This runs
            // on `cide-pty`'s coalescer thread, which is the thread every byte of every session
            // in the process flows through.
            let mut lines: Vec<String> = Vec::new();
            {
                let mut scan = scan.lock();
                let mut rest = bytes;
                if scan.resyncing {
                    match rest.iter().position(|b| *b == b'\n') {
                        Some(end) => {
                            scan.resyncing = false;
                            rest = &rest[end + 1..];
                        }
                        None => return true,
                    }
                }
                scan.buffer.extend_from_slice(rest);

                let mut start = 0;
                for end in 0..scan.buffer.len() {
                    if scan.buffer[end] == b'\n' {
                        lines.push(String::from_utf8_lossy(&scan.buffer[start..end]).into_owned());
                        start = end + 1;
                    }
                }
                scan.buffer.drain(..start);
                if scan.buffer.len() > LINE_CAP {
                    scan.buffer.clear();
                    scan.resyncing = true;
                }
            }

            for line in lines {
                // Until it answers, and never again: a respawn continues the same conversation
                // and prints the same id, so this is one write per run rather than one per line.
                if !captured.load(Ordering::Acquire)
                    && let Some(harness_session) = capture(&line)
                {
                    registry.note_harness_session(run, harness_session);
                    captured.store(true, Ordering::Release);
                }
                if let Some((moved, _)) =
                    registry.observe(app.as_ref(), session, Observation::Line(&line))
                    && let Some(app) = app.as_ref()
                {
                    after_transition(app, &registry, moved);
                }
            }
            true
        }));
        let _ = id.set(attached);
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
            let inner = self.inner.lock();
            let live = inner
                .runs
                .get(&run)
                .filter(|live| live.project == project)
                .ok_or_else(|| CoreError::Io(format!("no such run in this project: {run}")))?;
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
            return;
        }
    }
}

/// Prepare the worktree, build the argv and fork. **On the blocking pool** — every line of it
/// touches a disk or a process.
/// Acknowledge a sink's credit from a thread that is **not** the one that delivered it.
///
/// # Why this is not `pty.ack(id, n)` at the delivery site
///
/// `cide-pty`'s `broadcast` calls `Sink::deliver` **with the sink list locked** — it is a
/// `retain` over that list, which is what lets a sink that answers `false` be dropped in place.
/// So a sink calling [`PtySession::ack`] inside its own delivery takes that same
/// `parking_lot::Mutex` a second time on the same thread, and parks the **coalescer**: every
/// terminal in the process stops painting, every screen mirror stops advancing, and nothing
/// anywhere reports it. That is not a theory — it is what the first version of
/// [`AgentRegistry::watch_stream`] did, and the only symptom was a run whose row never moved.
///
/// # And why a scanner that simply never acks is not the answer either
///
/// At 256 KiB of unacknowledged output `cide-pty` chokes a sink, and a choked sink stops
/// receiving the *stream* and starts receiving rendered screens. For a pane that is a catch-up;
/// for a line scanner it is the channel quietly turning into terminal noise, a quarter of a
/// megabyte into a run that will print far more than that.
///
/// # One thread for the process
///
/// Created on the first harness-bound run and never joined, like `child_env::on_spawn_thread`'s.
/// It holds nothing while it waits, and each message carries a [`Weak`] so a queued
/// acknowledgement cannot keep a dead run's session alive. A send that finds the thread gone is
/// dropped: the cost of a lost acknowledgement is credit, and the watchdog forgives that anyway.
fn ack_credit(pty: &Weak<PtySession>, sink: SinkId, bytes: usize) {
    type Credit = (Weak<PtySession>, SinkId, usize);
    static ACKS: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<Credit>>> =
        std::sync::OnceLock::new();

    let sender = ACKS.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Credit>();
        let spawned = std::thread::Builder::new()
            .name("cide-agents-credit".into())
            .spawn(move || {
                for (pty, sink, bytes) in rx {
                    if let Some(pty) = pty.upgrade() {
                        pty.ack(sink, bytes);
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "no thread to acknowledge agent output; runs may be throttled");
        }
        Mutex::new(tx)
    });
    let _ = sender.lock().send((pty.clone(), sink, bytes));
}

fn start_child(
    app: &AppHandle,
    registry: &Arc<AgentRegistry>,
    facts: &Facts,
    admission: &Admission,
    session: SessionId,
    resume: Option<&str>,
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
    if let Some(why) = cide_agents::dispatch_refusal(agent, &project.config.agents) {
        return Err(CoreError::Io(why));
    }

    let cwd = match project.config.agents.isolation {
        Isolation::Worktree => {
            // Idempotent, and called before every dispatch rather than once per agent: a user can
            // delete `.cide/worktrees/<agent>` between two runs, and `ensure` repairs a
            // registration whose directory has gone.
            //
            // The run's cwd *is* its resume identity — `claude` files its transcript under the
            // directory it started in — so this is not merely where the work happens.
            let tree = cide_git::worktree::ensure(&facts.root, &admission.agent.0)
                .map_err(|error| CoreError::Io(error.to_string()))?;

            // One checkout, one process. An `Idle` run of this role has released its slot but its
            // child is still sitting in that directory, so it is wound down at the one moment the
            // checkout is actually needed. See `idle_children_of`.
            if let Some(sessions) = app.try_state::<SessionRegistry>() {
                for idle in
                    registry.idle_children_of(admission.project, &admission.agent, admission.run)
                {
                    if let Some(pty) = sessions.get(idle) {
                        tracing::info!(session = %idle, agent = %admission.agent, "winding down an idle run to reclaim its worktree");
                        pty.kill();
                    }
                }
            }
            tree.path
        }
        // Nothing separates two runs here, which is what the setting says on its face and what
        // `agents_config_set` refuses to arrange by accident.
        Isolation::Shared => facts.root.clone(),
    };

    let plan = RunPlan {
        run: admission.run,
        session,
        agent,
        cwd: cwd.clone(),
        project: admission.project,
        task: admission.task.clone(),
        task_title: admission.task_title.clone(),
        prompt: admission.prompt.clone(),
        hook_bin: facts.hook_bin.clone(),
        hook_sock: facts.hook_sock.clone(),
        agent_sock: facts.agent_sock.clone(),
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
    };

    let harness = cide_agents::for_kind(agent.def.harness).ok_or_else(|| {
        CoreError::Io(format!(
            "this build has no implementation for the `{:?}` harness",
            agent.def.harness
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

    let binding = spawn.binding;
    let pty = PtySession::spawn(spawn.spec).map_err(|error| CoreError::Io(error.to_string()))?;

    // Attached here rather than after this function returns, and the difference is real: the
    // caller resumes on another thread after an `await`, and a run whose whole liveness channel
    // is its own output cannot afford to have the first lines of it delivered to nobody.
    if let SessionBinding::Harness { capture } = binding {
        registry.watch_stream(Some(app), admission.run, session, &pty, capture);
    }

    Ok(Started {
        session: pty,
        opening: spawn.opening,
        cwd,
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
fn after_transition(app: &AppHandle, registry: &Arc<AgentRegistry>, run: RunId) {
    if let Some(project) = registry.project_of(run) {
        registry.mark_changed(app, project);
    }
    registry.pump(app);
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
    fn spec(project: ProjectId, agent: &str, agent_limit: u16, project_limit: u16) -> DispatchSpec {
        DispatchSpec {
            project,
            agent: AgentId(agent.into()),
            agent_label: agent.to_string(),
            harness: Harness::Claude,
            task: None,
            task_title: None,
            prompt: "do the thing".into(),
            agent_limit,
            project_limit,
        }
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

    /// **One worktree per agent means one run at a time.**
    ///
    /// The property the whole queue exists for: three dispatches against one role admit exactly
    /// one, and the other two wait rather than starting a second `claude` in the same checkout.
    #[test]
    fn an_agent_admits_one_run_and_queues_the_rest() {
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        // 1 is what `effective_max_concurrent` answers under worktree isolation, whatever the
        // role's own `max-concurrent` says.
        let runs: Vec<RunId> = (0..3)
            .map(|_| registry.enqueue(spec(project, "developer", 1, 4)))
            .collect();

        let admitted = registry.take_admissions();
        assert_eq!(admitted.len(), 1, "a second run would share a worktree");
        assert_eq!(admitted[0].run, runs[0], "the queue is first in, first out");
        assert_eq!(state_of(&registry, runs[0]), RunState::Starting);
        assert_eq!(state_of(&registry, runs[1]), RunState::Queued);
        assert_eq!(state_of(&registry, runs[2]), RunState::Queued);

        // And asking again while it is still starting admits nothing: the slot is held from
        // admission, not derived from the phase.
        assert!(registry.take_admissions().is_empty());
    }

    /// A role allowed several slots gets them in one pass, not one per unrelated event.
    ///
    /// Only reachable under `Isolation::Shared` — worktree isolation clamps a role to 1 — and the
    /// bug it pins is a single-pass scan: only the front of an agent's queue is eligible, so one
    /// pass starts one of three and leaves the other two waiting for something else to pump.
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
        registry.watch_exit_with(session, &pty, move |_, run| {
            let _ = tx.send(run);
        });

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

    /// **A run with no hooks moves on its own output, and the queue moves with it.**
    ///
    /// The whole liveness story for the second harness, against a real child and the real state
    /// mapping: the id the CLI minted is read off the first line, the run leaves `Starting`, and
    /// the turn ending hands the role's slot back — which is what lets the next queued run start.
    /// Nothing here is a hook, because there is no hook to be had.
    ///
    /// `/bin/sh` rather than `opencode`, for `a_real_childs_exit_finishes_its_run`'s reason: what
    /// is under test is the wiring between a child's stdout and a run, and the child's identity is
    /// irrelevant to it. The `sleep` is not a hedge against slowness — it closes a real race in
    /// the *test*: the sink is attached after the spawn, and a child that had already printed and
    /// been flushed would have written to nobody.
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
                )),
        )
        .expect("spawn sh");
        registry.watch_stream(None, run, session, &pty, test_capture);

        let deadline = Instant::now() + Duration::from_secs(10);
        while state_of(&registry, run) != RunState::Idle && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }

        // The turn was handed back — and **not** `Finished { code: 0 }`, which would be an exit
        // status for a process that has not exited.
        assert_eq!(state_of(&registry, run), RunState::Idle);
        assert_eq!(
            registry.inner.lock().runs[&run].harness_session.as_deref(),
            Some("ses_fe6da2c1effe8Yz"),
            "the id the CLI minted is what a follow-up names"
        );

        // And the slot came back with the turn, so the queue moves.
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
}
