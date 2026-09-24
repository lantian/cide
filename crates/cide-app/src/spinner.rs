//! Waking a project that has gone quiet with work still on the board. (M79)
//!
//! Everything else in cide's orchestration reacts to something somebody *did*: a task was
//! assigned (`cide_agents::autodispatch`), a run ended (`crate::agent_rpc::note_run_over`), a
//! stop was asked for. This reacts to **nothing happening**. After a project has been quiet for
//! `AgentsConfig::auto_spin_after_secs` — no live run, no working Claude pane, the queue open,
//! and at least one task not `Done` — cide opens a Claude tab in the project's unattended
//! permission mode and hands it the project's prompt, which tells it to survey what happened,
//! judge it, and put the next tasks on the roles.
//!
//! **Not plan mode**, which it was from M79 until M88. Plan mode asks before every MCP call —
//! the survey *is* MCP calls — and ends at an `ExitPlanMode` approval no hook can answer, so
//! cide read that prompt off the screen and typed the answer. Two workarounds for a mode whose
//! only job here, *survey before you act*, the prompt already states in its first sentence.
//!
//! That is a billed process started by a timer on a machine whose owner may be elsewhere, which
//! is why `auto_spin` is the only key in `AgentsConfig` besides `enabled` that ships **off**.
//!
//! # A dwell, not an interval
//!
//! [`Quiet::quiet_for`] measures how long the project has *looked* quiet, and the clock restarts
//! the moment it does not. A plain interval would eventually land in the middle of a burst
//! settling — the run-end nudge alone coalesces for up to ten seconds after the last child exits
//! — and plan from a board that is between two states. `MIN_SPIN_AFTER_SECS` is the floor for
//! that reason and is not cosmetic.
//!
//! # The decision is pure, the tick is not
//!
//! [`should_spin`] takes facts and answers a bool, `cide_agents::autodispatch::trigger`'s shape,
//! so every rule below is a table row in the tests rather than a claim about a live registry
//! somebody would have to reproduce by waiting fifteen minutes. [`tick`] is the impure half: it
//! reads the workspace, the registry, the hook states, the board and the committed config, and
//! then calls the pure one.
//!
//! # Where this runs
//!
//! One named daemon thread, `crate::tasks_state::TasksStores::start_flusher`'s shape — which is
//! the model rather than `WorkspaceState::start_flusher` because that one puts its body in a
//! separate `tick` and holds an `AppHandle`, which is exactly what is needed here. There is no
//! app tick in cide and this does not add one: *"three stores, three threads, no tick — which is
//! the arrangement to keep, because each one's timer is chosen for what its own file is worth."*
//!
//! It must not be the GTK loop and must not be a Tauri runtime worker, because
//! `claude_tab::open_with_prompt` enters the runtime with `block_on` to fork the child.

use std::collections::HashMap;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use cide_agents::config::AgentsConfig;
use cide_ipc::{ProjectId, RunState, SessionId, SessionState, TaskBoard, TaskStatus};
use tauri::{AppHandle, Manager};

use crate::workspace_state::WorkspaceState;

/// How often the dwell is re-examined.
///
/// Thirty seconds, which is coarse on purpose. The thing being measured is "nothing has happened
/// for a quarter of an hour", so the resolution that matters is minutes; polling every second
/// would cost a registry lock, a hook-state walk and a board read per project per second, for a
/// number that changes on the scale of a coffee break. It also bounds how far past the dwell a
/// spin can land, which nobody can perceive at this scale.
const POLL: Duration = Duration::from_secs(30);

/// What one project looks like at the moment of the check.
///
/// Facts and not decisions — deliberately. A `bool` called `can_spin` computed by the caller
/// would move the rules out of [`should_spin`] and back into the thing the tests cannot drive.
#[derive(Debug, Clone, PartialEq, Eq)]
///
/// There is deliberately **no `enabled` here**. That fact lives in the committed config, which
/// [`should_spin`] already takes whole — carrying a copy would be two sources for one key, and
/// the copy would be the one read a tick earlier.
pub(crate) struct Quiet {
    /// `AgentRegistry::dispatching`: is the queue open, or has somebody pressed Pause.
    pub dispatching: bool,
    /// Runs that hold a child or are about to get one. See [`counts_as_busy`].
    pub live_runs: usize,
    /// Claude panes of this project whose session is mid-turn, blocked, or still starting.
    pub busy_panes: usize,
    /// Tasks whose status is not [`TaskStatus::Done`].
    pub open_tasks: usize,
    /// How long every one of the above has continuously looked quiet.
    pub quiet_for: Duration,
    /// The active milestone's gate passed against what is checked out now, so the milestone
    /// waits for the user to accept it. (M83) Read off the gate's verdict, **not** off the
    /// milestone task being in `review` — see [`milestone_ready`].
    pub milestone_waiting: bool,
}

/// Should cide wake this project up?
///
/// The five conditions, each of which earned its place:
///
/// * **`enabled`.** A project that has not opted into subagents is never spun. The spinner's
///   whole output is *dispatch work to roles*, which such a project cannot do.
/// * **`auto_spin`.** The feature's own switch, off by default.
/// * **`dispatching`.** The user's "not paused". cide has no global pause — `paused_projects` is
///   per project, and it is what the Agents panel's Pause button sets (queue shut, every child
///   `SIGSTOP`ped, the console frozen too). Since the spinner is itself per project, that is the
///   right scope, and a paused project is exactly one that said *start nothing new here*.
/// * **No live run and no busy pane.** Both halves, and the second is the one that is easy to
///   leave out: a spinner that only watched runs would open a planning tab over the shoulder of
///   somebody typing in their own console. `Idle` runs count as busy on purpose — an idle run's
///   child is alive and holds its role's only worktree (`holds_a_pair`'s argument), so planning
///   around it would plan around work that is merely between turns.
/// * **Open work.** At least one task not `Done`. Firing on an empty board would spend a turn
///   asking a model to invent work, which is the one thing nobody wants a timer to do
///   unattended; the board being empty is a decision for a person.
///
/// * **No milestone waiting on the user.** (M83) When the active milestone's gate has passed, the
///   milestone waits until somebody accepts it, and that is the one moment the next step is a
///   person's: planning more work then either re-does what is met or starts on a milestone the
///   user has not agreed to. The gate is what a project is *for*; a spinner that ran past it
///   would make the goal decorative. *Passed* is the gate's verdict, never the milestone task's
///   status — see [`milestone_ready`].
///
/// And then the dwell. `quiet_for` is compared against `AgentsConfig::spin_after`, which clamps
/// — this function deliberately reads the clamped value rather than the stored one, so a
/// hand-edited `5` cannot make the timer fire inside the nudge coalescer's settling window.
pub(crate) fn should_spin(quiet: &Quiet, config: &AgentsConfig) -> bool {
    config.enabled
        && config.auto_spin
        && quiet.dispatching
        && quiet.live_runs == 0
        && quiet.busy_panes == 0
        && quiet.open_tasks > 0
        && !quiet.milestone_waiting
        && quiet.quiet_for >= config.spin_after()
}

/// Whether a run means "this project is working".
///
/// Everything with a child or a claim on one: `Queued` has no child yet but has a place in the
/// queue, `Paused` is frozen but holds its worktree, `Idle` is alive between turns. Only the
/// three that are genuinely over — `Finished`, `Failed`, and `Interrupted`, whose child died
/// with a previous cide — say nothing about now.
///
/// Wider than `RunState::counts_as_work`, and that is the point: that predicate answers *is this
/// run accruing time*, which is a billing question. This one answers *is there anything in
/// flight*, and an `Idle` run is not accruing time and very much is in flight.
///
/// # Except an idle run whose task is in review
///
/// A claude run hands its turn back, its task goes to `review`, and the child stays alive at
/// its prompt so a send-back has somewhere to land — `AgentRegistry::retire_done` ends it only
/// once the task is `done`. A review nobody finishes therefore left an `Idle` run alive for the
/// rest of the session, and the project never looked quiet: one stuck task switched the
/// *Quiet for* timer off without a word. Such a run has finished its work; waiting on it is
/// waiting on a person, which is exactly the situation the wake exists for, and its prompt
/// tells the planner to read what is sitting in review. `task_in_review` is read off the board
/// at the tick.
fn counts_as_busy(state: &RunState, task_in_review: bool) -> bool {
    match state {
        RunState::Finished { .. } | RunState::Failed { .. } | RunState::Interrupted => false,
        RunState::Idle => !task_in_review,
        _ => true,
    }
}

/// Whether a Claude pane's session means somebody is working.
///
/// The inverse of `agent_rpc::may_be_typed_into`, widened by `Exited`: a pane whose child has
/// gone is not work in progress, it is an empty pane. `Spawning` counts as busy, which also
/// closes the window between [`tick`] opening a tab and the new child's first hook frame —
/// `HookServer::state` answers `Spawning` for a session it has never seen, so the tab this
/// function just created makes its own project busy immediately, and no second tab can follow
/// it. `a_just_spun_project_is_busy_by_its_own_new_pane` is the test for that.
///
/// # Only a live child that is not a run's
///
/// That same `Spawning`-for-the-unknown answer is also what the hook server says about every
/// session it will **never** hear from, and reading it as busy there kept a project busy for the
/// rest of the process — the *Quiet for* timer simply never fired, with nothing logged. Two
/// shapes of it were on a real board:
///
/// * **A pane whose child has exited.** `lifecycle::report_exit` calls `HookServer::forget`, so
///   an exited session has no entry and reads as `Spawning`, not `Exited`. A Claude pane left
///   open over a dead conversation made its project permanently busy.
/// * **A run's session shown in a Claude pane.** terrastrike's third pane was an opencode run's
///   mirror, and opencode sends no Claude hooks at all. Runs are already counted — with the right
///   state machine — by `live_runs`, so a session the agent registry owns is left to that count
///   rather than read a second time through hooks that do not describe it.
///
/// `alive` is asked of the `SessionRegistry` at the tick, never remembered, and `owned_by_run`
/// of `AgentRegistry::owns_session`. The just-spun tab is still covered: `claude_tab` forks the
/// child before the pane exists (session first, tab second), so it is alive and unowned, and
/// reads `Spawning` until its first hook.
fn pane_is_busy(alive: bool, owned_by_run: bool, state: SessionState) -> bool {
    alive
        && !owned_by_run
        && !matches!(
            state,
            SessionState::Idle | SessionState::AwaitingInput | SessionState::Exited { .. }
        )
}

/// When each project was last seen *not* quiet.
///
/// A `std::sync::Mutex` and not `parking_lot`'s: this is touched once every [`POLL`] by one
/// thread, so there is nothing to tune, and the standard one needs no import to justify.
static SINCE: Mutex<Option<HashMap<ProjectId, Instant>>> = Mutex::new(None);

/// Start the spinner. Called once, from `setup`.
///
/// A failure to spawn costs the feature and not the launch — `start_flusher`'s rule, and the
/// right trade for something that is off by default in every project that has not asked for it.
pub fn start(app: AppHandle) {
    thread::Builder::new()
        .name("cide-spinner".into())
        .spawn(move || {
            loop {
                thread::sleep(POLL);
                tick(&app);
            }
        })
        .map(|_| ())
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "no spinner thread; projects will not be woken automatically");
        });
}

/// One pass over every open project.
///
/// Reads are ordered so that no lock is held across another: the project list comes out of the
/// workspace and the lock is dropped, then each project's facts are gathered, then the config is
/// read off the disk, and only then is anything opened. `WorkspaceState::with`'s closure must not
/// block or re-enter, and `cide_agents::config::load` is a file read.
fn tick(app: &AppHandle) {
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return;
    };
    let projects: Vec<ProjectId> = state.with(|ws| ws.projects.keys().copied().collect());
    if projects.is_empty() {
        return;
    }

    // Forget projects that are no longer open, so a session that opens and closes a hundred
    // of them does not accumulate a hundred instants. Cheap, and it also makes the dwell
    // **restart** for a project closed and reopened — which is right: the facts that make a
    // project quiet are only observable while cide is watching it, and a project that was shut
    // for an hour was not being watched for that hour.
    forget_all_but(&projects);

    let now = Instant::now();
    for project in projects {
        let Some(quiet) = facts(app, &state, project, now) else {
            continue;
        };
        // Read fresh, per project, per tick. `.cide/config.json` is committed, so a `git
        // checkout` can switch the spinner off under a running app — `nudge_allowed`'s rule, and
        // it matters more here, because the thing being switched off starts a billed process.
        let Ok(root) = crate::tasks_state::project_root(&state, project) else {
            continue;
        };
        let config = cide_agents::config::load(&root).agents;
        if !should_spin(&quiet, &config) {
            continue;
        }

        // Cleared **before** the tab is opened, not after. The open takes a fork and a workspace
        // mutation, during which the next tick can begin; a project still holding an old
        // `quiet_for` at that moment would satisfy the dwell a second time and spin twice. After
        // this line the project's dwell restarts from zero whatever happens next, including a
        // failure — a project whose spin cannot be opened should wait out another full dwell
        // rather than retry every thirty seconds.
        mark_busy(project, now);

        tracing::info!(%project, "the project has been quiet with open work; waking it");
        // On a thread of its own since M83, because it may first run the milestone gate, which
        // takes minutes, and this loop serves every project.
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("cide-spin".into())
            .spawn(move || {
                if let Err(error) = wake(&app, project, &root, &config) {
                    tracing::info!(%project, %error, "the quiet project was not woken");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%project, %error, "could not start the thread that wakes the project");
        }
    }
}

/// Open the planning tab, having first asked the gate. (M83)
///
/// With milestones, the planning turn is told where the active one stands — facts cide computed,
/// in one line after the prompt — and a gate that has not run against what is checked out now is
/// run first, so the plan is made from today's answer rather than yesterday's. A gate that passes
/// here moves the milestone to review (`milestones::announce_met`) and nothing is planned: that
/// was the question the planning turn would have been asking.
///
/// `Ok(())` once the tab is open; `Err` with a sentence for why none was, which the timer logs and
/// the Tasks panel's **Plan tasks** button shows ([`plan_now`]).
fn wake(
    app: &AppHandle,
    project: ProjectId,
    root: &std::path::Path,
    config: &AgentsConfig,
) -> Result<(), String> {
    let plan = cide_agents::config::load_milestones(root);
    let mut prompt = config.spin_prompt().to_string();
    if let Some(current) = plan.current() {
        let checks = app.try_state::<std::sync::Arc<crate::milestones::Checks>>();
        let head = cide_core::check::head_of(root);
        let stale = checks
            .as_ref()
            .is_none_or(|c| c.gate_is_stale(root, &current.id, head.as_deref()));
        let gate = if stale {
            crate::milestones::run_gate_now(app, project)
        } else {
            checks.as_ref().and_then(|c| c.last_gate(root, &current.id))
        };
        if gate.as_ref().is_some_and(|g| g.passed) {
            tracing::info!(%project, milestone = %current.id, "the gate passes; not planning past it");
            return Err(format!(
                "Milestone {} already passes its gate and is waiting for you in review, so \
                 there is nothing to plan until it is accepted.",
                current.id
            ));
        }
        let rows = app
            .try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>()
            .and_then(|stores| stores.get(project))
            .map(|store| store.list())
            .unwrap_or_default();
        if let Some(facts) = cide_agents::milestones::facts_line(&plan, &rows, gate.as_ref()) {
            // One line: this is typed into a terminal, where a newline is another Enter.
            let facts = facts.split_whitespace().collect::<Vec<_>>().join(" ");
            prompt = format!("{prompt} {facts}");
        }
    }
    // Dated, and the same string names the tab and the session: see `plan_title`.
    let title = cide_agents::config::plan_title_now();
    crate::claude_tab::open_with_prompt(
        app,
        project,
        &title,
        Some(&title),
        &prompt,
        // The same stance as every other unattended child cide starts (`agents.permissionMode`,
        // `auto` by default): a planner parked on a permission prompt nobody answers is the
        // failure that field documents.
        crate::claude_tab::TabMode {
            unattended: config.unattended(),
            // Raised: an idle project has nothing on screen to interrupt. See the field.
            behind: false,
        },
        // The project root, and not a worktree: this one is planning for the whole board,
        // not reviewing one branch, and the root is the checkout its survey is about.
        None,
    )
    .map(|_| ())
    .map_err(|error| {
        tracing::warn!(%project, %error, "could not wake the project");
        error.to_string()
    })
}

/// The Tasks panel's **Plan tasks** button: the timer's planning tab, opened now. (M88)
///
/// The same road as the timer ([`wake`]), so the tab, its prompt, its permission mode and the
/// milestone facts appended to it are one decision and cannot drift into two. What it skips is
/// everything in [`should_spin`] that is about *a timer deciding on its own*: `auto_spin`, the
/// dwell, live runs, busy panes, an empty board. A person pressed the button, which is the
/// consent all of those stand in for. `enabled` is kept, because the whole output of the tab is
/// work put on roles, and a project with subagents off has none to put it on.
///
/// **Blocking, and minutes long at worst**: a stale milestone gate is run before the plan is
/// made. The caller is a command on the blocking pool; see `cmd::agents::agents_plan_now`.
///
/// Restarts the timer's dwell, or a project whose owner just planned by hand would be planned
/// again by the timer a minute later.
pub(crate) fn plan_now(app: &AppHandle, project: ProjectId) -> Result<(), String> {
    let state = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| "the workspace is going away".to_string())?;
    let root = crate::tasks_state::project_root(&state, project).map_err(|e| e.to_string())?;
    let config = cide_agents::config::load(&root).agents;
    if !config.enabled {
        return Err(
            "Subagents are off for this project, so a planner would have no roles to put work \
             on. Turn them on in Settings → Agents first."
                .into(),
        );
    }
    mark_busy(project, Instant::now());
    wake(app, project, &root, &config)
}

/// Gather one project's facts, and update its dwell.
///
/// `None` when the project has gone between the snapshot and here, which a close can do.
fn facts(
    app: &AppHandle,
    state: &WorkspaceState,
    project: ProjectId,
    now: Instant,
) -> Option<Quiet> {
    let registry = app.try_state::<std::sync::Arc<crate::agents::AgentRegistry>>()?;
    // `get`, never `ensure` — `note_death`'s rule. A project whose tracker has never been opened
    // has no board on screen and nothing to plan from, and parsing the file on nobody's behalf
    // would be disk work on every tick for every project for ever. Such a project reads as
    // having no open work and is never spun, which is correct: cide has not been asked to look.
    let board = app
        .try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>()
        .and_then(|stores| stores.get(project))
        .map(|store| store.board());
    let tasks: &[cide_ipc::TaskRow] = match &board {
        Some(TaskBoard::Ready { tasks, .. }) => tasks,
        _ => &[],
    };
    let in_review = |task: &Option<cide_ipc::TaskId>| {
        task.as_ref().is_some_and(|id| {
            tasks
                .iter()
                .any(|row| &row.id == id && row.status == TaskStatus::Review)
        })
    };
    let live_runs = registry
        .runs_for(project)
        .iter()
        .filter(|run| counts_as_busy(&run.state, in_review(&run.task)))
        .count();
    let dispatching = registry.dispatching(project);

    // Every Claude session this project draws, including detached panes — a pane torn into its
    // own window is still this project's conversation and still somebody working.
    let sessions: Vec<SessionId> = state.with(|ws| {
        let Ok(project) = cide_core::workspace::project(ws, project) else {
            return Vec::new();
        };
        project
            .tabs
            .iter()
            .flat_map(|tab| tab.tree.panes.values())
            .chain(project.detached.values())
            .filter(|pane| pane.kind == cide_ipc::PaneKind::Claude)
            .filter_map(|pane| pane.session)
            .collect()
    });
    let ptys = app.try_state::<crate::state::SessionRegistry>();
    let alive = |session: SessionId| {
        ptys.as_ref()
            .and_then(|ptys| ptys.get(session))
            .is_some_and(|pty| !pty.has_exited())
    };
    let busy_panes = match app.try_state::<crate::hooks::HookServer>() {
        Some(hooks) => sessions
            .iter()
            .filter(|session| {
                pane_is_busy(
                    alive(**session),
                    registry.owns_session(**session),
                    hooks.state(**session),
                )
            })
            .count(),
        // No hook server means no session state for anything, and `HookServer::state`'s own
        // unknown answer is `Spawning`. Counting every live pane as busy is the same
        // conservative direction: a build that cannot tell does not spin.
        None => sessions
            .iter()
            .filter(|session| alive(**session) && !registry.owns_session(**session))
            .count(),
    };

    let open_tasks = tasks
        .iter()
        // The inbox is not open work (M83): a project whose only undone tasks are things
        // somebody noticed has nothing planned, and waking it to plan from them is how a board
        // ends up growing faster than it closes.
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Todo | TaskStatus::Doing | TaskStatus::Review
            )
        })
        .count();

    // The dwell is driven by the **live** facts alone, never by the config: a project whose
    // `autoSpin` was switched on five minutes ago should not have to wait a fresh fifteen when
    // it has already been quiet for an hour. The config decides whether to act on the dwell,
    // not whether the dwell was accruing.
    let looks_quiet = live_runs == 0 && busy_panes == 0 && dispatching;
    let quiet_for = dwell(project, now, looks_quiet);

    // Read per tick like the config: the plan is committed and the gate's verdict is in memory.
    let milestone_waiting = crate::tasks_state::project_root(state, project)
        .ok()
        .is_some_and(|root| milestone_ready(app, &root));

    Some(Quiet {
        dispatching,
        live_runs,
        busy_panes,
        open_tasks,
        quiet_for,
        milestone_waiting,
    })
}

/// Whether the active milestone is ready to be accepted: its gate's last verdict passed, and
/// that verdict is about the commit checked out now.
///
/// Until this, the fact was *the milestone's task is in `review`*, on the reasoning that
/// `milestones::announce_met` is what puts it there. It is not the only thing that can. In
/// selfcraft a planning turn moved `slice`'s task `todo → review` by hand while the gate was
/// red, and from then on the spinner read the project as waiting on the user and never woke it
/// again — no log line, just a *Quiet for* that never fired. A status any agent can set is a
/// claim; the gate's result is the evidence, so the spinner reads the evidence and ignores
/// review entirely.
///
/// **Current HEAD only.** A pass at an older commit says nothing about what is checked out now,
/// and treating it as ready would park the project until something happened to re-run the gate.
/// Letting it through is safe: [`wake`] re-runs a stale gate before planning and stops there
/// if it passes. The `git rev-parse` is only paid when the last verdict passed, which is rare.
fn milestone_ready(app: &AppHandle, root: &std::path::Path) -> bool {
    let plan = cide_agents::config::load_milestones(root);
    let Some(current) = plan.current() else {
        return false;
    };
    let Some(checks) = app.try_state::<std::sync::Arc<crate::milestones::Checks>>() else {
        return false;
    };
    if !checks
        .last_gate(root, &current.id)
        .is_some_and(|gate| gate.passed)
    {
        return false;
    }
    let head = cide_core::check::head_of(root);
    !checks.gate_is_stale(root, &current.id, head.as_deref())
}

/// How long this project has looked quiet, updating the record.
///
/// `looks_quiet: false` restarts the clock and answers zero.
fn dwell(project: ProjectId, now: Instant, looks_quiet: bool) -> Duration {
    let mut guard = SINCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    if !looks_quiet {
        map.insert(project, now);
        return Duration::ZERO;
    }
    // First time seen quiet: the dwell starts now rather than at process start, so launching
    // cide onto a project that was already idle does not spin it in the first tick.
    let since = map.entry(project).or_insert(now);
    now.saturating_duration_since(*since)
}

/// Drop the dwell of every project not in `open`. See [`tick`].
fn forget_all_but(open: &[ProjectId]) {
    let mut guard = SINCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(map) = guard.as_mut() {
        map.retain(|project, _| open.contains(project));
    }
}

/// Restart a project's dwell. Called when a spin is about to happen. See [`tick`].
fn mark_busy(project: ProjectId, now: Instant) {
    let mut guard = SINCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.get_or_insert_with(HashMap::new).insert(project, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project that satisfies every condition. Each test below breaks exactly one.
    fn ready() -> Quiet {
        Quiet {
            dispatching: true,
            live_runs: 0,
            busy_panes: 0,
            open_tasks: 3,
            quiet_for: Duration::from_secs(10_000),
            milestone_waiting: false,
        }
    }

    fn asking() -> AgentsConfig {
        AgentsConfig {
            enabled: true,
            auto_spin: true,
            ..AgentsConfig::default()
        }
    }

    #[test]
    fn a_quiet_project_with_open_work_is_woken() {
        assert!(should_spin(&ready(), &asking()));
    }

    /// The table, and it is the whole specification: every condition, broken one at a time,
    /// against a fixture that is otherwise ready. Written this way rather than as six tests
    /// because the interesting property is that each one is *individually* sufficient to refuse
    /// — a condition silently folded into another would still pass six separate tests.
    #[test]
    fn every_condition_refuses_on_its_own() {
        let cases: Vec<(&str, Quiet, AgentsConfig)> = vec![
            (
                "a project that has not opted into subagents at all",
                ready(),
                AgentsConfig {
                    enabled: false,
                    ..asking()
                },
            ),
            (
                "the feature's own switch, which ships off",
                ready(),
                AgentsConfig {
                    auto_spin: false,
                    ..asking()
                },
            ),
            (
                "agents paused — the queue is shut and nothing new may start",
                Quiet {
                    dispatching: false,
                    ..ready()
                },
                asking(),
            ),
            (
                "a subagent is already running",
                Quiet {
                    live_runs: 1,
                    ..ready()
                },
                asking(),
            ),
            (
                "somebody is working in a Claude pane of this project",
                Quiet {
                    busy_panes: 1,
                    ..ready()
                },
                asking(),
            ),
            (
                "the board has nothing open, so there is nothing to plan around",
                Quiet {
                    open_tasks: 0,
                    ..ready()
                },
                asking(),
            ),
            (
                "the milestone's gate passed and it waits for the user to accept it",
                Quiet {
                    milestone_waiting: true,
                    ..ready()
                },
                asking(),
            ),
            (
                "it has not been quiet for long enough yet",
                Quiet {
                    quiet_for: Duration::from_secs(5),
                    ..ready()
                },
                asking(),
            ),
        ];
        for (why, quiet, config) in cases {
            assert!(!should_spin(&quiet, &config), "spun anyway: {why}");
        }
    }

    /// **The dwell is measured against the clamped number, not the stored one.**
    ///
    /// `.cide/config.json` is hand-editable and committed, so a `5` can arrive in it — from a
    /// typo, from a teammate, from a model that was allowed near the file. Read literally that
    /// would fire inside the run-end nudge's own ten-second settling window and plan from a
    /// board between two states. `spin_after` clamps up to `MIN_SPIN_AFTER_SECS`, and this is
    /// the assertion that `should_spin` goes through it rather than reading the field.
    #[test]
    fn a_hand_written_five_seconds_is_still_the_floor() {
        let config = AgentsConfig {
            auto_spin_after_secs: 5,
            ..asking()
        };
        let just_past_five = Quiet {
            quiet_for: Duration::from_secs(6),
            ..ready()
        };
        assert!(
            !should_spin(&just_past_five, &config),
            "the stored number was read instead of the clamped one"
        );
        let past_the_floor = Quiet {
            quiet_for: Duration::from_secs(u64::from(cide_agents::config::MIN_SPIN_AFTER_SECS)),
            ..ready()
        };
        assert!(should_spin(&past_the_floor, &config));
    }

    /// An `Idle` run is a live child holding its role's only worktree, so the project is not
    /// quiet — `counts_as_busy`'s argument, asserted rather than left to the reader.
    #[test]
    fn only_the_three_states_with_no_child_read_as_quiet() {
        let busy = [
            RunState::Queued,
            RunState::Starting,
            RunState::Running,
            RunState::Idle,
            RunState::AwaitingPermission,
            RunState::Paused { since_unix_ms: 1 },
        ];
        for state in busy {
            assert!(
                counts_as_busy(&state, false),
                "{state:?} should hold the spinner off"
            );
        }
        let over = [
            RunState::Finished { code: 0 },
            RunState::Failed { reason: "x".into() },
            RunState::Interrupted,
        ];
        for state in over {
            assert!(
                !counts_as_busy(&state, false),
                "{state:?} is over and says nothing about now"
            );
        }
    }

    /// An idle run whose task sits in review has done its work and is waiting on a person; a
    /// review nobody finishes must not switch the *Quiet for* timer off for the session. Only
    /// `Idle` gets the exemption — a run still working on a task that says `review` is working.
    #[test]
    fn an_idle_run_on_a_task_in_review_does_not_hold_the_spinner_off() {
        assert!(!counts_as_busy(&RunState::Idle, true));
        assert!(counts_as_busy(&RunState::Running, true));
        assert!(counts_as_busy(&RunState::AwaitingPermission, true));
        assert!(counts_as_busy(&RunState::Paused { since_unix_ms: 1 }, true));
    }

    /// **The window between opening a tab and its child's first hook frame is closed by
    /// `Spawning` counting as busy.**
    ///
    /// `HookServer::state` answers `Spawning` for a session it has never seen, which is exactly
    /// what the tab this module just opened looks like for the first second or so of its life.
    /// If that read as quiet, the next tick — thirty seconds later, with the dwell already
    /// satisfied — would open a second planning tab over the first, and a third after that.
    /// `mark_busy` is the other half of the guard; this is the half that does not depend on it.
    #[test]
    fn a_just_spun_project_is_busy_by_its_own_new_pane() {
        let busy = |state| pane_is_busy(true, false, state);
        assert!(busy(SessionState::Spawning));
        assert!(busy(SessionState::Splash));
        assert!(busy(SessionState::Busy));
        assert!(busy(SessionState::AwaitingPermission));
        assert!(busy(SessionState::Paused));
        // And the three that genuinely mean nobody is working.
        assert!(!busy(SessionState::Idle));
        assert!(!busy(SessionState::AwaitingInput));
        assert!(!busy(SessionState::Exited { code: 0 }));
    }

    /// The *Quiet for* timer that never fired. `HookServer::state` answers `Spawning` for a
    /// session it has no entry for — which is every exited one (`report_exit` forgets it) and
    /// every opencode run's (no hooks at all) — so a pane over either kept its project busy for
    /// ever. Neither may count, whatever the hook server says.
    #[test]
    fn a_pane_nobody_is_behind_does_not_keep_the_project_busy() {
        // A Claude pane whose child has gone: forgotten, so it reads as `Spawning`.
        assert!(!pane_is_busy(false, false, SessionState::Spawning));
        // An opencode run's mirror: alive, silent to hooks, owned by the registry — which
        // counts it through `live_runs` instead.
        assert!(!pane_is_busy(true, true, SessionState::Spawning));
        // A claude run's mirror mid-turn is the registry's to count too, not a second time here.
        assert!(!pane_is_busy(true, true, SessionState::Busy));
    }

    /// A closed project's dwell is dropped, so reopening it starts the clock again rather than
    /// inheriting an hour of "quiet" during which cide was not watching.
    #[test]
    fn a_closed_project_forgets_how_long_it_was_quiet() {
        let project = ProjectId::new();
        let start = Instant::now();

        assert_eq!(dwell(project, start, true), Duration::ZERO);
        let later = start + Duration::from_secs(600);
        assert_eq!(dwell(project, later, true), Duration::from_secs(600));

        forget_all_but(&[]);
        assert_eq!(
            dwell(project, later, true),
            Duration::ZERO,
            "a reopened project inherited a dwell from before it was closed"
        );
    }

    /// The dwell restarts on any busy tick, and does not start at process start.
    ///
    /// The second half is the one worth a test: launching cide onto a project that has been
    /// idle for a week must not spin it in the first pass, because the facts that make a
    /// project quiet are only observable from the moment cide is watching.
    #[test]
    fn the_dwell_starts_when_the_quiet_is_first_seen() {
        let project = ProjectId::new();
        let start = Instant::now();

        assert_eq!(
            dwell(project, start, true),
            Duration::ZERO,
            "first sighting"
        );
        let later = start + Duration::from_secs(300);
        assert_eq!(dwell(project, later, true), Duration::from_secs(300));

        // One busy tick, and the five minutes are gone: the map now holds *the moment the
        // project was last busy*, and quiet is measured forward from there. So a minute later
        // the answer is one minute and not six — which is the whole property, because a dwell
        // that kept accumulating across a busy tick would be an interval wearing a dwell's name.
        assert_eq!(dwell(project, later, false), Duration::ZERO);
        let after = later + Duration::from_secs(60);
        assert_eq!(
            dwell(project, after, true),
            Duration::from_secs(60),
            "restarted"
        );
    }
}
