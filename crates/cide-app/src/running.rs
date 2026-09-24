//! What is *working* in each project, for the header's project-tab badge — and the same
//! arithmetic the auto-spin quiet detector reads. (M94)
//!
//! The window header draws one tab per open project, and until this the only thing it said
//! about a project the user is **not** in was the amber chip: *N sessions are waiting for you*
//! (`ui/src/panes/awaiting.ts`). Nothing said the opposite and equally useful fact — *N things
//! are working in there right now* — so a user with agents dispatched across three projects had
//! to activate each tab to find out which one was busy.
//!
//! # Why this is not a fourth definition of "busy"
//!
//! [`crate::spinner::should_spin`] already answers *is anything in flight in this project*, over
//! two counts, with two predicates that each carry a paragraph of hard-won exceptions — an
//! `Idle` run whose task is sitting in review; a hook server that answers `Spawning` for a
//! session it will never hear from. Both of those cost a real outage to learn.
//!
//! A badge computed from its own definition would disagree with that timer on exactly the days
//! the exceptions matter, and neither number would be checkable against the other. So the
//! predicates live here, the gathering lives here, and `spinner::facts` reads this module: **the
//! spinner asks "is it zero", the header asks "how many", and it is one arithmetic.** The repo
//! already carries three deliberately different notions of busy — `RunState::counts_as_work`
//! (accruing time), `SessionState::is_live` (worth warning about at quit) and this one (anything
//! in flight) — and each of their docs says which question it answers. A fourth would need the
//! same, and there is no fourth question here.
//!
//! # Why Rust decides this outright, where it only *aggregates* the awaiting set
//!
//! `ui/src/panes/awaiting.ts` argues that the frontend must decide *awaiting* per session,
//! because acknowledgement — a click or a keystroke into a pane — is a webview event Rust never
//! sees. Nothing here is acknowledged, and every input lives in a registry no webview can reach:
//! a run's state in [`crate::agents::AgentRegistry`], whether a child is alive in
//! [`crate::state::SessionRegistry`], whether a pane is merely a run's mirror in
//! `AgentRegistry::owns_session`, and what the hook server last heard in
//! [`crate::hooks::HookServer`]. It needs all four **for projects the asking window is not
//! drawing**, which is the whole point of the feature. So the frontend contributes no input and
//! is given none to report: one listener, one catch-up, no report direction at all.
//!
//! # The event is free in the steady state, and that is the diff, not the debounce
//!
//! The traffic that could move this answer is heavy — a hook frame per tool call, a roster emit
//! about once a second per working run. Coalescing alone would still put one whole-map emit into
//! every window every second for the life of every run, and every project tab would re-render
//! each time to draw the same number. So [`flush`] **compares against the last broadcast and
//! returns without emitting when nothing moved**. A run working flat out produces zero
//! `cide://project-running` events, because zero of its tool calls change a count.
//!
//! That property is worth stating because it is the one a future refactor deletes by accident
//! while "simplifying the coalescer"; `a_burst_that_changes_no_count_emits_nothing` is the pin.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use cide_ipc::{
    PaneKind, ProjectId, ProjectRunning, ProjectRunningSet, RunState, SessionId, SessionState,
    TaskBoard, TaskId, TaskRow, Workspace,
};
use tauri::{AppHandle, Manager};

use crate::workspace_state::WorkspaceState;

/// Trailing debounce on `cide://project-running`. [`crate::agents`]'s numbers, for its reason:
/// a burst here is the same burst — four runs each reporting a tool call.
const COALESCE: Duration = Duration::from_millis(120);
/// The ceiling, so a project that never stops changing still updates about once a second.
const COALESCE_CEILING: Duration = Duration::from_secs(1);
/// How often the flusher wakes while a burst is settling.
const COALESCE_TICK: Duration = Duration::from_millis(20);

// ==========================================================================================
// The two predicates. Moved here from `spinner.rs`, with their doc comments, so that the
// badge and the quiet detector cannot drift apart.
// ==========================================================================================

/// Which question is being asked about a state. (M94)
///
/// The two readings below exist because two surfaces genuinely ask different things, and the
/// first version of this module answered both with one predicate and a `_ => true` arm. That
/// arm hid three disagreements, and the user found the loudest of them within an hour: **they
/// pressed Pause and the header went on saying their agents were running.**
///
/// A `_` arm is what made that possible, so both matches below are exhaustive: a new
/// [`RunState`] or [`SessionState`] variant now fails to compile until somebody has said what
/// it means to each reader, rather than defaulting to *working* in a badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Busy {
    /// *Must I hold off starting new work here?* — [`crate::spinner`]'s question, and the one
    /// the predicates were originally written for. Anything holding a child, a place in the
    /// queue or a worktree counts, because waking a project to plan around such a thing plans
    /// around work that is merely between turns, or frozen, or about to start.
    Claimed,
    /// *How much is executing right now?* — the header project tab's question.
    ///
    /// Strictly narrower, and the three arms where it differs are all cases where **nothing is
    /// running**: a queued run has no child, a paused one has a child the kernel has stopped,
    /// and an idle one is sitting at a prompt. Showing any of them as working makes the chip a
    /// thing the user learns to ignore — and pausing is the sharpest case, because it is a
    /// button they pressed *in order to* stop everything.
    Working,
}

/// Whether a run means "this project is working".
///
/// # `Claimed`
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
/// ## Except an idle run whose task is in review
///
/// A claude run hands its turn back, its task goes to `review`, and the child stays alive at
/// its prompt so a send-back has somewhere to land — `AgentRegistry::retire_done` ends it only
/// once the task is `done`. A review nobody finishes therefore left an `Idle` run alive for the
/// rest of the session, and the project never looked quiet: one stuck task switched the
/// *Quiet for* timer off without a word. Such a run has finished its work; waiting on it is
/// waiting on a person, which is exactly the situation the wake exists for, and its prompt
/// tells the planner to read what is sitting in review. `task_in_review` is read off the board
/// at the tick.
///
/// # `Working`
///
/// Three states only: `Starting`, `Running`, `AwaitingPermission`. A child exists and the turn
/// has not been handed back.
///
/// `Paused` is the arm this reading was added for. `AgentRegistry::pause` shuts the queue, sets
/// every live run to `Paused` and `SIGSTOP`s its session — the project's own console included —
/// so after it *nothing in the project is executing*, and a chip still showing a number is
/// contradicting the button the user just pressed. `Queued` follows for the same reason and
/// would have been the next report: pause shuts the queue but leaves queued rows queued, so a
/// badge that counted them would have gone from wrong to slightly-less-wrong. `Idle` is a live
/// child between turns, which is the Agents panel's business and not a count of work in flight.
///
/// `task_in_review` is not consulted at all here, and that is not an oversight: `Idle` is
/// already out, and it was the only arm the flag ever qualified.
pub(crate) fn run_is_busy(state: &RunState, task_in_review: bool, asking: Busy) -> bool {
    match state {
        // Over, under every reading.
        RunState::Finished { .. } | RunState::Failed { .. } | RunState::Interrupted => false,
        // A child exists and the turn is live.
        RunState::Starting | RunState::Running | RunState::AwaitingPermission => true,
        // A claim on a slot or a worktree, with nothing executing behind it.
        RunState::Queued | RunState::Paused { .. } => asking == Busy::Claimed,
        // Alive between turns — unless its task is parked in review, where even the spinner
        // stops counting it. See above.
        RunState::Idle => asking == Busy::Claimed && !task_in_review,
    }
}

/// Whether a console pane's session means somebody is working.
///
/// The inverse of `agent_rpc::may_be_typed_into`, widened by `Exited`: a pane whose child has
/// gone is not work in progress, it is an empty pane.
///
/// # Only a live child that is not a run's
///
/// `HookServer::state` answers `Spawning` for every session it has no entry for, and reading
/// that as busy kept a project busy for the rest of the process — the *Quiet for* timer simply
/// never fired, with nothing logged. Two shapes of it were on a real board:
///
/// * **A pane whose child has exited.** `lifecycle::report_exit` calls `HookServer::forget`, so
///   an exited session has no entry and reads as `Spawning`, not `Exited`. A Claude pane left
///   open over a dead conversation made its project permanently busy.
/// * **A run's session shown in a Claude pane.** terrastrike's third pane was an opencode run's
///   mirror, and opencode sends no Claude hooks at all. Runs are already counted — with the right
///   state machine — by the run count, so a session the agent registry owns is left to that
///   count rather than read a second time through hooks that do not describe it.
///
/// `alive` is asked of the [`crate::state::SessionRegistry`] at the tick, never remembered, and
/// `owned_by_run` of `AgentRegistry::owns_session`.
///
/// # Where the two readings differ, and why `Spawning` is the important one
///
/// `Spawning` means **this session has never said anything**. For [`Busy::Claimed`] that counts,
/// and has to: `claude_tab` forks the child before the pane exists, so the tab `spinner::tick`
/// has just opened reads as `Spawning` for its first second or so, and if that read as quiet the
/// next tick — thirty seconds later, with the dwell already satisfied — would open a second
/// planning tab over the first, and a third after that.
/// `a_just_spun_project_is_busy_by_its_own_new_pane` is the test.
///
/// For [`Busy::Working`] it must **not** count, and this arm is the whole reason that split
/// carries its own tests. M93 measured that codex fires `SessionStart` *with the first prompt,
/// not at startup* — so a codex console left at its composer, doing nothing whatsoever, sits in
/// `Spawning` for as long as it is open. The first cut counted it, and a project whose only
/// content was a freshly opened codex console drew a turning spinner and a `1`. Claude has the
/// same shape for a second or two, and any future harness that never speaks Claude's hook
/// protocol has it for ever.
///
/// A bounded grace — count `Spawning` only while the session is young — was the first fix, and
/// it was the wrong one: it made the chip lie for thirty seconds instead of for ever, and it
/// changed the spinner's behaviour to buy that. The right answer is that a badge does not guess.
/// *Nothing has been heard about this session* is not evidence of work, and the honest direction
/// for a count on a tab is to under-report for the moment before a real turn's first hook
/// arrives rather than to over-report every console anyone opens.
///
/// `Paused` and `Splash` split for a related reason: a session the kernel has stopped, and a
/// pane showing a resume splash with no child behind it at all, are both things the *user is
/// looking at* rather than work in progress. Neither is `Working`; both are `Claimed`, because
/// opening a planning tab over somebody's frozen console or unanswered splash is exactly what
/// the spinner must not do.
pub(crate) fn pane_is_busy(
    alive: bool,
    owned_by_run: bool,
    state: SessionState,
    asking: Busy,
) -> bool {
    if !alive || owned_by_run {
        return false;
    }
    match state {
        SessionState::Idle | SessionState::AwaitingInput | SessionState::Exited { .. } => false,
        SessionState::Busy | SessionState::AwaitingPermission => true,
        SessionState::Spawning | SessionState::Paused | SessionState::Splash => {
            asking == Busy::Claimed
        }
    }
}

// ==========================================================================================
// Gathering.
// ==========================================================================================

/// This project's board, or `None` when nobody has opened its tracker.
///
/// `get`, never `ensure` — `note_death`'s rule, and `spinner::facts` said it first. A project
/// whose tracker has never been opened has no board on screen; parsing the file on nobody's
/// behalf would be disk work on every change for every project for ever. Such a project's runs
/// simply never hit the review carve-out, which is the conservative direction.
pub(crate) fn board_of(app: &AppHandle, project: ProjectId) -> Option<TaskBoard> {
    app.try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>()
        .and_then(|stores| stores.get(project))
        .map(|store| store.board())
}

/// The rows of a board that has them. `&[]` for every other shape, including "never opened".
pub(crate) fn rows_of(board: &Option<TaskBoard>) -> &[TaskRow] {
    match board {
        Some(TaskBoard::Ready { tasks, .. }) => tasks,
        _ => &[],
    }
}

/// One project's two counts.
///
/// Takes the board rows rather than reading them, so the spinner — which needs them anyway, for
/// `open_tasks` — pays for one board clone per tick instead of two.
///
/// Takes a `&Workspace` rather than the [`WorkspaceState`] for a reason `spinner::tick` states
/// in full: no lock may be held across another, and this function reads three registries. The
/// caller takes its snapshot first and drops the workspace lock.
pub(crate) fn counts_for(
    app: &AppHandle,
    ws: &Workspace,
    project: ProjectId,
    tasks: &[TaskRow],
    asking: Busy,
) -> ProjectRunning {
    let Some(registry) = app.try_state::<std::sync::Arc<crate::agents::AgentRegistry>>() else {
        return ProjectRunning {
            project,
            runs: 0,
            panes: 0,
        };
    };

    let in_review = |task: &Option<TaskId>| {
        task.as_ref().is_some_and(|id| {
            tasks
                .iter()
                .any(|row| &row.id == id && row.status == cide_ipc::TaskStatus::Review)
        })
    };
    let runs = registry
        .runs_for(project)
        .iter()
        .filter(|run| run_is_busy(&run.state, in_review(&run.task), asking))
        .count();

    // `session_panes` rather than a walk over `project.tabs`, and that is not a tidy-up: a
    // torn-out pane is *removed* from its tab's tree and lives in `Project::detached`, so a walk
    // over tabs alone silently misses it — which is what `cmd::app::live_sessions` did, and why
    // the quit dialog never once warned about a busy `claude` in a detached window. A pane in
    // its own window is still this project's conversation and still somebody working.
    //
    // A `BTreeSet`, because two panes mirroring one child are **one** conversation. The spinner
    // could collect a `Vec` and not care — it only ever asks whether the count is zero — but a
    // badge that said `2` about one mirrored session would be counting windows onto a thing
    // rather than the thing, which is the point `awaitingRule::awaitingAmong` makes at length
    // for the chip sitting next to this one.
    let sessions: BTreeSet<SessionId> = cide_core::workspace::session_panes(ws, Some(project))
        .into_iter()
        .filter(|found| found.pane.kind == PaneKind::Claude)
        .map(|found| found.session)
        .collect();

    let ptys = app.try_state::<crate::state::SessionRegistry>();
    let alive = |session: SessionId| {
        ptys.as_ref()
            .and_then(|ptys| ptys.get(session))
            .is_some_and(|pty| !pty.has_exited())
    };
    let panes = match app.try_state::<crate::hooks::HookServer>() {
        Some(hooks) => sessions
            .iter()
            .filter(|session| {
                pane_is_busy(
                    alive(**session),
                    registry.owns_session(**session),
                    hooks.state(**session),
                    asking,
                )
            })
            .count(),
        // No hook server means no session state for anything, and `HookServer::state`'s own
        // unknown answer is `Spawning`. Counting every live unowned pane as busy is the same
        // conservative direction: a build that cannot tell does not spin. It is also
        // unreachable in a real app — the server is managed in `setup` — so it never reaches a
        // badge.
        None => sessions
            .iter()
            .filter(|session| alive(**session) && !registry.owns_session(**session))
            .count(),
    };

    ProjectRunning {
        project,
        runs: runs as u32,
        panes: panes as u32,
    }
}

/// Every open project with something running, in workspace order, plus a fresh generation.
///
/// Projects with nothing running are **omitted**, not carried as zeroes:
/// [`ProjectRunningSet`]'s doc says why, and it is `cide://session-awaiting`'s rule one level up.
pub(crate) fn compute(app: &AppHandle, ws: &Workspace) -> ProjectRunningSet {
    let projects = ws
        .projects
        .keys()
        .copied()
        .map(|project| {
            let board = board_of(app, project);
            // `Working`, never `Claimed`: this is the header's chip, and a queued, paused or
            // idle run is not something the user would call running. Pausing is the case that
            // proves it — see `Busy`.
            counts_for(app, ws, project, rows_of(&board), Busy::Working)
        })
        .filter(|running| running.runs > 0 || running.panes > 0)
        .collect();
    ProjectRunningSet {
        projects,
        at: AT.fetch_add(1, Ordering::Relaxed),
    }
}

/// The generation on every computed set. **Not a clock**: it exists only so a window can drop a
/// catch-up reply that a broadcast overtook. See [`ProjectRunningSet`].
static AT: AtomicU64 = AtomicU64::new(0);

// ==========================================================================================
// The emit coalescer, and the diff that makes it free.
// ==========================================================================================

/// What was last broadcast, so an unchanged recomputation can be dropped.
///
/// A process-wide `OnceLock<Mutex<…>>` rather than managed Tauri state, on `windows.rs`'s
/// `awaiting()` trade: every reader is in this module, so there is nothing for a `State` to buy
/// and an `AppHandle` fewer to thread through [`flush`].
fn last() -> &'static Mutex<Option<Vec<ProjectRunning>>> {
    static LAST: OnceLock<Mutex<Option<Vec<ProjectRunning>>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

/// Whether this set is news. Records it when it is.
fn moved(projects: &[ProjectRunning]) -> bool {
    let mut guard = last().lock().unwrap_or_else(|p| p.into_inner());
    if guard.as_deref() == Some(projects) {
        return false;
    }
    *guard = Some(projects.to_vec());
    true
}

/// A trailing debounce with a ceiling. [`crate::agents::AgentRegistry`]'s `Coalescer` with the
/// set taken out: the payload here is the whole map, so there is no per-project bookkeeping to
/// do and one flag is the entire state a burst needs.
#[derive(Default)]
struct Pulse {
    /// When the first un-noted change of this burst arrived. Drives [`COALESCE_CEILING`].
    first: Option<Instant>,
    /// And the most recent. Drives [`COALESCE`].
    last: Option<Instant>,
    /// Whether a flusher thread is already waiting this burst out.
    flushing: bool,
}

fn pulse() -> &'static Mutex<Pulse> {
    static PULSE: OnceLock<Mutex<Pulse>> = OnceLock::new();
    PULSE.get_or_init(Default::default)
}

/// Record a change. `true` means *you are the thread that must flush it*.
fn note() -> bool {
    let mut state = pulse().lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    state.first.get_or_insert(now);
    state.last = Some(now);
    if state.flushing {
        return false;
    }
    state.flushing = true;
    true
}

/// Whether the burst has settled. Clears it when it has.
fn due() -> bool {
    let mut state = pulse().lock().unwrap_or_else(|p| p.into_inner());
    let (Some(first), Some(last)) = (state.first, state.last) else {
        return false;
    };
    if last.elapsed() < COALESCE && first.elapsed() < COALESCE_CEILING {
        return false;
    }
    state.first = None;
    state.last = None;
    state.flushing = false;
    true
}

/// Note that something may have changed the answer.
///
/// Called from **three funnels and no individual call site** — `emit::session_state`,
/// `AgentRegistry::mark_changed` and `WorkspaceState::update`'s accepted-change tail. Marking
/// from each of the thirty-odd places that move a run, a session or the tree is the version of
/// this that goes stale the first time somebody adds one and does not know they have to;
/// `cmd::window::retitle` is in the third of those funnels for exactly the same reason, and its
/// comment there argues it.
///
/// Cheap by construction: one mutex and two `Option<Instant>`s. It has to stay that cheap,
/// because the hook applier that calls it through `emit::session_state` is a single ordered
/// thread whose ordering is a correctness requirement.
pub fn mark(app: &AppHandle) {
    if !note() {
        return;
    }
    let app = app.clone();
    // One short-lived thread per burst rather than a permanent ticker — `mark_changed`'s rule:
    // a process with nothing running should not wake up fifty times a second for ever.
    if let Err(error) = thread::Builder::new()
        .name("cide-running-emit".into())
        .spawn(move || flush(app))
    {
        tracing::warn!(%error, "no thread for the running counts; the project tabs will not update");
        // Release the latch, or every later `mark` in this process believes a flusher is
        // already waiting and the feature is off for good rather than for one burst.
        let mut state = pulse().lock().unwrap_or_else(|p| p.into_inner());
        state.first = None;
        state.last = None;
        state.flushing = false;
    }
}

/// Wait out the burst, compute once, and emit only if the answer moved.
fn flush(app: AppHandle) {
    loop {
        thread::sleep(COALESCE_TICK);
        if !due() {
            continue;
        }
        let Some(state) = app.try_state::<WorkspaceState>() else {
            return;
        };
        // One clone of the tree for every project, rather than one `with` per project: this is
        // off the hook applier already, and it is the only way to read the registries without
        // holding the workspace lock across them.
        let set = compute(&app, &state.snapshot());
        if moved(&set.projects) {
            crate::emit::project_running(&app, &set);
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The window between opening a tab and its child's first hook frame is closed by
    /// `Spawning` counting as busy.**
    ///
    /// `HookServer::state` answers `Spawning` for a session it has never seen, which is exactly
    /// what the tab `spinner::tick` just opened looks like for the first second or so of its
    /// life. If that read as quiet, the next tick — thirty seconds later, with the dwell already
    /// satisfied — would open a second planning tab over the first, and a third after that.
    /// `mark_busy` is the other half of the guard; this is the half that does not depend on it.
    #[test]
    fn a_just_spun_project_is_busy_by_its_own_new_pane() {
        let busy = |state| pane_is_busy(true, false, state, Busy::Claimed);
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
        assert!(!pane_is_busy(
            false,
            false,
            SessionState::Spawning,
            Busy::Claimed
        ));
        // An opencode run's mirror: alive, silent to hooks, owned by the registry — which
        // counts it through the run count instead.
        assert!(!pane_is_busy(
            true,
            true,
            SessionState::Spawning,
            Busy::Claimed
        ));
        // A claude run's mirror mid-turn is the registry's to count too, not a second time here.
        assert!(!pane_is_busy(true, true, SessionState::Busy, Busy::Claimed));
    }

    /// **A console that has never said a word is not working, and the badge must not guess.**
    ///
    /// The screenshot that produced this test: an empty project whose only content was a freshly
    /// opened **codex** console, sitting at its composer having been asked nothing, with the
    /// header drawing a turning spinner and a `1`.
    ///
    /// `HookServer::state` answers `Spawning` for any session it has no entry for, and M93
    /// measured that codex fires `SessionStart` *with the first prompt, not at startup* — so
    /// that console sits in `Spawning` for as long as it is open. Claude has the same shape for
    /// its first second or two, and a harness that speaks no Claude hooks has it for ever.
    ///
    /// The spinner's reading is the opposite and must stay that way: `claude_tab` forks the
    /// child before the pane exists, so the tab `tick` just opened reads `Spawning`, and if that
    /// read as quiet the next tick would open a second planning tab over the first.
    #[test]
    fn a_console_that_has_never_spoken_is_not_working() {
        assert!(!pane_is_busy(
            true,
            false,
            SessionState::Spawning,
            Busy::Working
        ));
        assert!(pane_is_busy(
            true,
            false,
            SessionState::Spawning,
            Busy::Claimed
        ));
    }

    /// A review nobody finishes must not switch the *Quiet for* timer off for the session, and
    /// must not leave the header saying a project is working when it is waiting on a person.
    /// Only `Idle` gets the exemption — a run still working on a task that says `review` is
    /// working.
    #[test]
    fn an_idle_run_on_a_task_in_review_does_not_hold_the_spinner_off() {
        assert!(!run_is_busy(&RunState::Idle, true, Busy::Claimed));
        assert!(run_is_busy(&RunState::Running, true, Busy::Claimed));
        assert!(run_is_busy(
            &RunState::AwaitingPermission,
            true,
            Busy::Claimed
        ));
        assert!(run_is_busy(
            &RunState::Paused { since_unix_ms: 1 },
            true,
            Busy::Claimed
        ));
    }

    /// An `Idle` run is a live child holding its role's only worktree, so the project is not
    /// quiet — [`run_is_busy`]'s argument, asserted rather than left to the reader. And the
    /// three that are genuinely over say nothing about now, whatever the board says.
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
                run_is_busy(&state, false, Busy::Claimed),
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
                !run_is_busy(&state, false, Busy::Claimed),
                "{state:?} is over and says nothing about now"
            );
            assert!(!run_is_busy(&state, true, Busy::Claimed));
            assert!(!run_is_busy(&state, false, Busy::Working));
        }
    }

    /// **Pause means nothing is running, and the header must say so.**
    ///
    /// Reported against the first cut of this feature, in one sentence: *"I paused subagents and
    /// title still shows me that this agents are running."* `AgentRegistry::pause` shuts the
    /// queue, sets every live run to `Paused` and `SIGSTOP`s its session — the project's own
    /// console included — so after it nothing in the project is executing. A chip still showing
    /// a number is contradicting the button the user just pressed, which is the fastest way to
    /// teach somebody to ignore a badge.
    ///
    /// The spinner's reading is deliberately the opposite and stays that way: a paused run holds
    /// its role's only worktree, and *start nothing new here* is exactly what Pause said.
    #[test]
    fn a_paused_project_is_working_on_nothing() {
        let paused = RunState::Paused { since_unix_ms: 1 };
        assert!(!run_is_busy(&paused, false, Busy::Working));
        assert!(run_is_busy(&paused, false, Busy::Claimed));

        // The console pause freezes too, through the session rather than the run.
        assert!(!pane_is_busy(
            true,
            false,
            SessionState::Paused,
            Busy::Working
        ));
        assert!(pane_is_busy(
            true,
            false,
            SessionState::Paused,
            Busy::Claimed
        ));
    }

    /// The two arms that would have been the *next* report, fixed in the same pass.
    ///
    /// Pause shuts the queue but leaves queued rows `Queued`, so a badge that counted them would
    /// have gone from wrong to slightly-less-wrong; and an `Idle` run is a live child sitting at
    /// a prompt between turns, which is the Agents panel's business and not a count of work in
    /// flight. Neither has a child doing anything.
    #[test]
    fn a_queued_or_idle_run_is_not_running() {
        for state in [RunState::Queued, RunState::Idle] {
            assert!(
                !run_is_busy(&state, false, Busy::Working),
                "{state:?} has nothing executing behind it"
            );
            assert!(
                run_is_busy(&state, false, Busy::Claimed),
                "{state:?} still holds a claim, so the spinner must not plan around it"
            );
        }
    }

    /// What the chip *does* count: a child exists and the turn has not been handed back. Equal
    /// under both readings, which is the point — the two questions only ever differ about things
    /// that are not executing.
    #[test]
    fn the_three_states_with_a_live_turn_count_under_both_readings() {
        for state in [
            RunState::Starting,
            RunState::Running,
            RunState::AwaitingPermission,
        ] {
            assert!(run_is_busy(&state, false, Busy::Working));
            assert!(run_is_busy(&state, true, Busy::Working));
            assert!(run_is_busy(&state, false, Busy::Claimed));
        }
        for state in [SessionState::Busy, SessionState::AwaitingPermission] {
            assert!(pane_is_busy(true, false, state, Busy::Working));
            assert!(pane_is_busy(true, false, state, Busy::Claimed));
        }
    }

    /// A pane at a resume splash has no child at all, so it is not working — but the spinner
    /// must still not open a planning tab over an unanswered one.
    #[test]
    fn a_splash_is_something_to_look_at_rather_than_work() {
        assert!(!pane_is_busy(
            true,
            false,
            SessionState::Splash,
            Busy::Working
        ));
        assert!(pane_is_busy(
            true,
            false,
            SessionState::Splash,
            Busy::Claimed
        ));
    }

    /// **The property that makes this event free.** A run working flat out reports a tool call a
    /// second and changes no count; every one of those must be dropped here rather than
    /// broadcast to every window, which would re-render every project tab to draw the same
    /// number. See the module header — this is what a "simplification" of the coalescer removes.
    #[test]
    fn a_burst_that_changes_no_count_emits_nothing() {
        let project = ProjectId::new();
        let one = vec![ProjectRunning {
            project,
            runs: 1,
            panes: 0,
        }];
        let two = vec![ProjectRunning {
            project,
            runs: 2,
            panes: 0,
        }];

        // Whatever the static holds from another test in this binary, the first distinct set is
        // news and a repeat of it is not.
        moved(&one);
        assert!(!moved(&one));
        assert!(moved(&two));
        assert!(!moved(&two));
        assert!(moved(&one));

        // And emptying is news too, or a project whose last run finished keeps its chip.
        assert!(moved(&[]));
        assert!(!moved(&[]));
    }

    /// A window that asks must be able to tell a stale reply from a fresh one, so the number
    /// only ever goes up. It is bumped on every computed set, including ones that are not
    /// emitted, which is harmless: the frontend compares, it does not count.
    #[test]
    fn the_generation_only_ever_increases() {
        let first = AT.fetch_add(1, Ordering::Relaxed);
        let second = AT.fetch_add(1, Ordering::Relaxed);
        assert!(second > first);
    }
}
