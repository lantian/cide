//! Waking a project that has gone quiet with work still on the board. (M79)
//!
//! Everything else in cide's orchestration reacts to something somebody *did*: a task was
//! assigned (`cide_agents::autodispatch`), a run ended (`crate::agent_rpc::note_run_over`), a
//! stop was asked for. This reacts to **nothing happening**. After a project has been quiet for
//! `AgentsConfig::auto_spin_after_secs` — no live run, no working Claude pane, the queue open,
//! and at least one task not `Done` — cide opens a Claude tab in plan mode and hands it the
//! project's prompt, which tells it to survey what happened, judge it, and put the next tasks on
//! the roles.
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
fn counts_as_busy(state: &RunState) -> bool {
    !matches!(
        state,
        RunState::Finished { .. } | RunState::Failed { .. } | RunState::Interrupted
    )
}

/// Whether a Claude pane's session means somebody is working.
///
/// The inverse of `agent_rpc::may_be_typed_into`, widened by `Exited`: a pane whose child has
/// gone is not work in progress, it is an empty pane. `Spawning` counts as busy, which also
/// closes the window between [`tick`] opening a tab and the new child's first hook frame —
/// `HookServer::state` answers `Spawning` for a session it has never seen, so the tab this
/// function just created makes its own project busy immediately, and no second tab can follow
/// it. `a_just_spun_project_is_busy_by_its_own_new_pane` is the test for that.
fn pane_is_busy(state: SessionState) -> bool {
    !matches!(
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

        let title = "Plan";
        tracing::info!(%project, "the project has been quiet with open work; waking it");
        if let Err(error) = crate::claude_tab::open_with_prompt(
            app,
            project,
            title,
            config.spin_prompt(),
            crate::claude_tab::TabMode::Plan {
                accept: config.auto_spin_accept_plan,
            },
            // The project root, and not a worktree: this one is planning for the whole board,
            // not reviewing one branch, and the root is the checkout its survey is about.
            None,
        ) {
            tracing::warn!(%project, %error, "could not wake the project");
        }
    }
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
    let live_runs = registry
        .runs_for(project)
        .iter()
        .filter(|run| counts_as_busy(&run.state))
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
    let busy_panes = match app.try_state::<crate::hooks::HookServer>() {
        Some(hooks) => sessions
            .iter()
            .filter(|session| pane_is_busy(hooks.state(**session)))
            .count(),
        // No hook server means no session state for anything, and `HookServer::state`'s own
        // unknown answer is `Spawning`. Counting every pane as busy is the same conservative
        // direction: a build that cannot tell does not spin.
        None => sessions.len(),
    };

    // `get`, never `ensure` — `note_death`'s rule. A project whose tracker has never been opened
    // has no board on screen and nothing to plan from, and parsing the file on nobody's behalf
    // would be disk work on every tick for every project for ever. Such a project reads as
    // having no open work and is never spun, which is correct: cide has not been asked to look.
    let open_tasks = app
        .try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>()
        .and_then(|stores| stores.get(project))
        .map_or(0, |store| match store.board() {
            TaskBoard::Ready { tasks, .. } => tasks
                .iter()
                .filter(|task| task.status != TaskStatus::Done)
                .count(),
            TaskBoard::Absent { .. } | TaskBoard::Unreadable { .. } => 0,
        });

    // The dwell is driven by the **live** facts alone, never by the config: a project whose
    // `autoSpin` was switched on five minutes ago should not have to wait a fresh fifteen when
    // it has already been quiet for an hour. The config decides whether to act on the dwell,
    // not whether the dwell was accruing.
    let looks_quiet = live_runs == 0 && busy_panes == 0 && dispatching;
    let quiet_for = dwell(project, now, looks_quiet);

    Some(Quiet {
        dispatching,
        live_runs,
        busy_panes,
        open_tasks,
        quiet_for,
    })
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
                counts_as_busy(&state),
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
                !counts_as_busy(&state),
                "{state:?} is over and says nothing about now"
            );
        }
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
        assert!(pane_is_busy(SessionState::Spawning));
        assert!(pane_is_busy(SessionState::Splash));
        assert!(pane_is_busy(SessionState::Busy));
        assert!(pane_is_busy(SessionState::AwaitingPermission));
        assert!(pane_is_busy(SessionState::Paused));
        // And the three that genuinely mean nobody is working.
        assert!(!pane_is_busy(SessionState::Idle));
        assert!(!pane_is_busy(SessionState::AwaitingInput));
        assert!(!pane_is_busy(SessionState::Exited { code: 0 }));
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
