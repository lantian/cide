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
use cide_ipc::{ProjectId, TaskRow, TaskStatus};
use tauri::{AppHandle, Manager};

use crate::running::{counts_for, rows_of};
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
    /// Runs that hold a child or are about to get one. See [`crate::running::run_is_busy`].
    pub live_runs: usize,
    /// Claude panes of this project whose session is mid-turn, blocked, or still starting.
    /// See [`crate::running::pane_is_busy`].
    ///
    /// Both numbers come from [`crate::running::counts_for`], which is also what the header's
    /// project-tab badge draws. One arithmetic on purpose: a timer and a badge that disagreed
    /// about whether a project is working would each be evidence against the other.
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
    // One clone of the tree for the whole pass, rather than a `with` per project. `facts` reads
    // three registries and this module's own rule is that no lock is held across another, so the
    // workspace has to be let go of before any of them — and at a thirty-second poll a snapshot
    // taken at the top of the tick is indistinguishable from one taken per project.
    let ws = state.snapshot();
    let projects: Vec<ProjectId> = ws.projects.keys().copied().collect();
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
        let Some(quiet) = facts(app, &state, &ws, project, now) else {
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
                if let Err(error) = wake(&app, project, &root, &config, Caller::Timer) {
                    tracing::info!(%project, %error, "the quiet project was not woken");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%project, %error, "could not start the thread that wakes the project");
        }
    }
}

/// Who is asking [`wake`] to plan. The two differ in how much the milestone gate may say.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Caller {
    /// The spinner's own timer. Runs a stale gate first, and stands down on a pass when the
    /// milestone has nothing left open under it.
    Timer,
    /// The Tasks panel's **Plan tasks** button. Never runs the gate and never stands down on it:
    /// see [`plan_now`].
    Button,
}

/// Open the planning tab, having first asked the gate. (M83)
///
/// With milestones, the planning turn is told where the active one stands — facts cide computed,
/// in one line after the prompt. For the timer, a gate that has not run against what is checked
/// out now is run first, so the plan is made from today's answer rather than yesterday's; a gate
/// that passes there moves the milestone to review (`milestones::announce_met`), and **if nothing
/// is left open or undecided under the milestone's goal** nothing is planned: that was the
/// question the planning turn would have been asking.
///
/// A pass with open tasks under the goal is *not* "nothing to plan". Until this was split, any
/// pass refused, and a milestone whose gate went green before its last tasks were closed — in
/// selfcraft, `slice` with tasks still in todo, doing and review — could not be planned at all,
/// not even from the button, which answered "waiting for you in review" while the board said
/// otherwise. The gate is a check the plan is made *from*; the open tasks are what it is made of.
///
/// The button ([`Caller::Button`]) skips the gate entirely: it reads whatever verdict is already
/// known, for the facts line only, and never refuses on it.
///
/// `Ok(())` once the tab is open; `Err` with a sentence for why none was, which the timer logs and
/// the Tasks panel's **Plan tasks** button shows ([`plan_now`]).
fn wake(
    app: &AppHandle,
    project: ProjectId,
    root: &std::path::Path,
    config: &AgentsConfig,
    caller: Caller,
) -> Result<(), String> {
    let plan = cide_agents::config::load_milestones(root);
    let mut prompt = config.spin_prompt().to_string();
    if let Some(current) = plan.current() {
        let rows = app
            .try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>()
            .and_then(|stores| stores.get(project))
            .map(|store| store.list())
            .unwrap_or_default();
        let checks = app.try_state::<std::sync::Arc<crate::milestones::Checks>>();
        let known = || checks.as_ref().and_then(|c| c.last_gate(root, &current.id));
        let gate = match caller {
            // A verdict about an older commit is still the best thing to tell the planner, and
            // the facts line says pass or fail rather than "at HEAD", so it is not a false claim.
            Caller::Button => known(),
            Caller::Timer => {
                let head = cide_core::check::head_of(root);
                let stale = checks
                    .as_ref()
                    .is_none_or(|c| c.gate_is_stale(root, &current.id, head.as_deref()));
                if stale {
                    crate::milestones::run_gate_now(app, project)
                } else {
                    known()
                }
            }
        };
        if caller == Caller::Timer
            && gate_blocks_planning(
                gate.as_ref().is_some_and(|g| g.passed),
                open_under_goal(&rows, current),
                undecided_under_goal(&rows, current),
            )
        {
            tracing::info!(%project, milestone = %current.id, "the gate passes; not planning past it");
            return Err(format!(
                "Milestone {} already passes its gate, with nothing open and nothing undecided \
                 under it, and is waiting for you in review, so there is nothing to plan until \
                 it is accepted.",
                current.id
            ));
        }
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
/// **The gate is not run and cannot refuse** (see [`Caller::Button`]). It used to be: a stale
/// gate was run first, which held the button for minutes, and a pass refused the plan outright
/// even with open tasks under the milestone. Pressing the button is the ask to plan now; whether
/// the milestone is finished is the gate's question and the user's, not a reason to say no.
/// Still called on the blocking pool (`cmd::agents::agents_plan_now`): opening a tab forks.
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
    wake(app, project, &root, &config, Caller::Button)
}

/// Gather one project's facts, and update its dwell.
///
/// `None` when the project has gone between the snapshot and here, which a close can do.
///
/// The two counts are **not computed here**: [`crate::running::counts_for`] owns them, because
/// the header's project-tab badge draws the same two numbers and a badge that disagreed with
/// this timer would make both untrustworthy. What stays here is everything the badge has no
/// opinion about — the board's open work, the milestone gate, and the dwell itself.
fn facts(
    app: &AppHandle,
    state: &WorkspaceState,
    ws: &cide_ipc::Workspace,
    project: ProjectId,
    now: Instant,
) -> Option<Quiet> {
    let registry = app.try_state::<std::sync::Arc<crate::agents::AgentRegistry>>()?;
    // `get`, never `ensure` — `note_death`'s rule, and `running::board_of` is where it now lives.
    // A project whose tracker has never been opened has no board on screen and nothing to plan
    // from, and parsing the file on nobody's behalf would be disk work on every tick for every
    // project for ever. Such a project reads as having no open work and is never spun, which is
    // correct: cide has not been asked to look.
    let board = crate::running::board_of(app, project);
    let tasks = rows_of(&board);
    let dispatching = registry.dispatching(project);

    // Wall clock rather than the `Instant` above, because the grace on a silent session is
    // compared against `SessionRegistry::started`, which is a `SystemTime`.
    // `Claimed`, which is this module's own question and the wider of the two: a queued run, a
    // frozen one and a console at a splash all mean *do not open a planning tab here*, even
    // though none of them is executing and the header's chip therefore does not count them.
    // `Busy`'s doc carries the split.
    let running = counts_for(app, ws, project, tasks, crate::running::Busy::Claimed);
    let live_runs = running.runs as usize;
    let busy_panes = running.panes as usize;

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
        .is_some_and(|root| milestone_ready(app, &root, tasks));

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
///
/// **And nothing left open or undecided under the milestone's goal** ([`gate_blocks_planning`]).
/// A green gate with tasks still in todo, doing or review is a milestone with work to plan
/// around, and parking it on the user would leave those tasks idle until someone accepted by
/// hand; a green gate with an inbox row under the goal is a milestone with a decision to make,
/// and the planning turn is what makes it.
fn milestone_ready(app: &AppHandle, root: &std::path::Path, rows: &[TaskRow]) -> bool {
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
    if !gate_blocks_planning(
        true,
        open_under_goal(rows, current),
        undecided_under_goal(rows, current),
    ) {
        return false;
    }
    let head = cide_core::check::head_of(root);
    !checks.gate_is_stale(root, &current.id, head.as_deref())
}

/// Tasks in todo, doing or review under the milestone's goal task. A milestone with no goal task
/// has nothing the board can say is under it, so it counts none.
fn open_under_goal(rows: &[TaskRow], milestone: &cide_ipc::Milestone) -> usize {
    milestone
        .task
        .as_ref()
        .map(|goal| cide_agents::milestones::open_under(rows, goal))
        .unwrap_or(0)
}

/// Inbox tasks under the milestone's goal task — decisions owed, not work in flight. See
/// `cide_agents::milestones::inbox_under` for why they are counted apart from the open ones.
fn undecided_under_goal(rows: &[TaskRow], milestone: &cide_ipc::Milestone) -> usize {
    milestone
        .task
        .as_ref()
        .map(|goal| cide_agents::milestones::inbox_under(rows, goal))
        .unwrap_or(0)
}

/// Whether a milestone's gate stops the timer from planning: it passed, **and** nothing is left
/// open under the milestone, **and** nothing under it waits in the inbox. One rule for [`wake`]
/// and [`milestone_ready`], so the timer that decides to wake and the wake that decides to plan
/// cannot disagree about the same board.
///
/// The inbox arm is M99, from selfcraft's `slice`: its gate went green with every subtask done
/// but one row left in the inbox under the goal — a thing a run noticed and nobody ruled on. The
/// project parked itself on the user, Accept offered, with an undecided item hanging under the
/// goal Accept would mark done. Deciding it is exactly a planning turn's job — keep it and do it
/// under this milestone, or unlink it and let it wait in the inbox — so the timer plans instead
/// of standing down, and [`crate::milestones`]' Accept is not offered until the board is clear.
/// Only the inbox *under this goal* counts; the rest of the inbox is somebody else's milestone.
fn gate_blocks_planning(
    gate_passed: bool,
    open_under_goal: usize,
    undecided_under_goal: usize,
) -> bool {
    gate_passed && open_under_goal == 0 && undecided_under_goal == 0
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

    /// A green gate stands the timer down only when the milestone has nothing left open and
    /// nothing left undecided; a red or unknown one never does. Two cases were wrong here: a
    /// gate passing with tasks in todo, doing and review, and the planner refusing as if the
    /// milestone were finished; and (M99) a gate passing with one inbox row under the goal,
    /// which is a decision owed and the reason to plan, not a reason not to.
    #[test]
    fn a_passing_gate_blocks_only_an_empty_milestone() {
        assert!(gate_blocks_planning(true, 0, 0));
        assert!(
            !gate_blocks_planning(true, 3, 0),
            "open tasks under the goal are work to plan"
        );
        assert!(
            !gate_blocks_planning(true, 0, 1),
            "an inbox task under the goal is a decision to make, and only a turn can make it"
        );
        assert!(!gate_blocks_planning(false, 0, 0));
        assert!(!gate_blocks_planning(false, 3, 2));
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
