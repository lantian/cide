//! Turns "this task was assigned or a role was @mentioned" into a dispatch.
//!
//! The policy — who may trigger, which statuses start work, what a mention does to the assignee
//! field — is [`cide_agents::autodispatch`], pure and table-tested. This module is the app half:
//! it collects [`TaskMutation`]s from the two places tasks are mutated (the panel's commands in
//! `cmd::tasks`, and the agent-RPC socket's `StoreSink`), reads `.cide/config.json` fresh at the
//! moment of each burst (the `nudge_orchestrator` discipline: a committed file can be switched
//! off under a running app, and a cached copy would keep spawning), and hands each survivor to
//! [`crate::cmd::agents::dispatch_or_duplicate`] — the function behind the command the panel's
//! button and the orchestrator's MCP tool call, because that is where the refusal table, the
//! task lookup and the one-line opening prompt live, and a second path would eventually get the
//! `bypassPermissions` arm wrong.
//!
//! # Both mutation roads end here, and the author gate is applied *after* the funnel
//!
//! Run-authored mutations are deliberately collected and passed through like every other — the
//! refusal is `autodispatch::trigger`'s author gate, in the pure function, where the test sees
//! it. Filtering at the collection sites instead would be two copies of the security boundary
//! that keeps a subagent from spawning a subagent.
//!
//! # Everything declines by doing nothing
//!
//! `deliver_nudge`'s argument, inherited: a project with agents off, a role the catalog does not
//! know, a task deleted mid-flight, a dispatch refusal — each is a log line, never an error the
//! panel shows. The gesture that got us here (a comment, a `<select>` change) already succeeded
//! and already answered; a durable "could not auto-start" would need a comment from a system
//! author `TaskAuthor` has no variant for, and is a named possible follow-up rather than a thing
//! this module fakes with the user's own identity.
//!
//! # The duplicate guard is the funnel's; the check here is only a shortcut (M66)
//!
//! Until M66 the `run_holding` call below — then spelled `has_open_run` — was the *only* thing in
//! cide stopping a role from getting two runs on one task, and it never could have been, for two
//! reasons. It was on one of the **three** dispatch roads: the Agents panel's button and
//! `cide_agent_dispatch` asked nothing at all, so an orchestrating session that assigned a task
//! and then dispatched the same role onto it got exactly what it asked for, twice. And it is not
//! atomic: [`dispatch`] below spawns, so two mutations in one burst — a `cide_task_create` naming
//! an assignee and a `cide_task_comment` saying `@role`, which is the pair that produced the
//! report — both read *no open run* before either reached the queue.
//!
//! The rule now lives where every other named dispatch refusal lives.
//! [`crate::cmd::agents::plan_dispatch`] refuses it with a sentence naming the live run, and
//! `AgentRegistry::enqueue_unique` refuses it again under the lock that mints the id, which is
//! the only place the race can actually be lost.
//!
//! The check below stays, and is now what it should always have been called: a shortcut. It
//! spares a spawn, a `.cide/` read and a board list on the ordinary repeated gesture, and it is
//! where that case is logged with the burst still in hand. When it is beaten,
//! `dispatch_or_duplicate` answers [`crate::cmd::agents::DispatchOutcome::Duplicate`] rather than
//! an error, and that arm logs `debug!` — this module's "everything declines by doing nothing"
//! policy meeting a guard that finally exists is not a warning, and a re-picked assignee has no
//! business putting a frightening line in anybody's log.
//!
//! # `spawn`, never `block_on`
//!
//! `consider` is reachable from async command handlers, and
//! `tauri::async_runtime::block_on` is legal only on the agent-RPC connection threads (the rule
//! `agent_rpc`'s `RegistrySink` documents). So the disk read happens on `spawn_blocking` and
//! each dispatch on its own spawned task, exactly like [`crate::agents::AgentRegistry::pump`]'s
//! admissions.

use std::sync::Arc;

use cide_agents::autodispatch;
use cide_ipc::{
    AgentId, DispatchRequest, ProjectId, RunNotify, Task, TaskAuthor, TaskEdit, TaskId, TaskRow,
    TaskStatus,
};
use tauri::{AppHandle, Manager as _};

use crate::agents::AgentRegistry;
use crate::cmd::agents::DispatchOutcome;
use crate::tasks_state::TasksStores;
use crate::workspace_state::WorkspaceState;

/// One task mutation, as the trigger policy wants to see it.
///
/// `fresh_text` is the prose *this mutation introduced* — a creation body, a rewritten body, a
/// new comment — and never the stored task, so a mention cannot re-fire on every later status
/// flip. Collected as plain data at the mutation site; nothing here holds a lock or a handle.
pub struct TaskMutation {
    /// The task as it was, `None` for a creation.
    pub before: Option<Task>,
    /// The task as the mutation left it.
    pub after: Task,
    /// Who mutated — from the command layer (`User`) or the RPC connection's header, never from
    /// a payload.
    pub author: TaskAuthor,
    /// Whether this mutation *was* an assignment gesture — the dropdown, `cide_task_assign`, a
    /// creation naming an assignee — as opposed to a save that carries the field along. Decided
    /// at the collection site, where the edit's shape is still in hand; the policy uses it to
    /// dispatch a re-pick of the same assignee (the debug report's revive gesture) without
    /// letting a status flip or comment re-fire the standing assignment.
    pub assign_gesture: bool,
    pub fresh_text: Vec<String>,
}

/// Ask the policy about a burst of mutations and act on what it answers. Returns immediately;
/// the work happens on the blocking pool.
///
/// # This is the funnel, and a second reaction rides it
///
/// Both roads a task can be mutated by — `cmd::tasks` and the agent-RPC socket's `StoreSink` —
/// end here, which makes this the one place that sees every mutation exactly once. Since M31
/// [`crate::spec_reveal`] reads the same burst for a different question (*did a proposal just
/// finish?*), rather than the two collection sites growing a second call each and drifting apart
/// the way a duplicated funnel does.
///
/// It runs **before** [`consider_blocking`] and outside all of that function's gates, which is
/// the load-bearing half of the ordering: dispatch declines when subagents are off for the
/// project or auto-dispatch is disabled, and *neither has anything to do with proposing* — a
/// proposal is typed into a conversation, not spawned. Revealing behind those gates would have
/// made the feature silently absent on every project that has not turned subagents on.
///
/// `notify` is where a run these mutations start will announce its turn endings (M40): the
/// session whose connection made the edit, when there is one (`agent_rpc` passes its `Scope`'s),
/// and the primary pane for an edit made in the Tasks panel, which has no session to name. It
/// rides through to the `DispatchRequest`, so a role assigned from a second Claude pane reports
/// back to that pane rather than to a console that never asked.
pub fn consider(
    app: &AppHandle,
    project: ProjectId,
    mutations: Vec<TaskMutation>,
    notify: RunNotify,
) {
    if mutations.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::spec_reveal::consider(&app, project, &mutations);
        consider_blocking(&app, project, mutations, notify);
    });
}

fn consider_blocking(
    app: &AppHandle,
    project: ProjectId,
    mutations: Vec<TaskMutation>,
    notify: RunNotify,
) {
    let root = {
        let Some(workspace) = app.try_state::<WorkspaceState>() else {
            return;
        };
        match crate::tasks_state::project_root(&workspace, project) {
            Ok(root) => root,
            Err(_) => return,
        }
    };

    // Fresh, per burst — see the module header. The catalog doubles as the mention filter: a
    // typo'd `@develoepr` must act on nothing, above all not on the assignee field.
    let loaded = cide_agents::load_project(&root);
    if !loaded.config.agents.enabled || !loaded.config.agents.auto_dispatch {
        return;
    }
    let roles: Vec<AgentId> = loaded
        .catalog
        .agents
        .iter()
        .map(|agent| agent.def.id.clone())
        .collect();

    let Some(registry) = app.try_state::<Arc<AgentRegistry>>() else {
        return;
    };

    // One read per burst, for the blocking gate: `mutation.after` carries the task's own
    // `blockedBy` edges, but a blocker's *status* lives on the other task, so the policy is
    // handed `blocker_statuses` over the whole board. The read races later mutations only in
    // the direction that is safe — a blocker finished after this snapshot delays a dispatch
    // until the next gesture, it never starts one early. (M30)
    let board: Vec<TaskRow> = app
        .try_state::<Arc<TasksStores>>()
        .and_then(|stores| stores.get(project))
        .map(|store| store.list())
        .unwrap_or_default();

    // Read once per burst, like the board above. (M83)
    let milestones = cide_agents::config::load_milestones(&root);

    for mutation in mutations {
        let fresh: Vec<&str> = mutation.fresh_text.iter().map(String::as_str).collect();
        let blockers = autodispatch::blocker_statuses(&mutation.after, &board);
        let Some(trigger) = autodispatch::trigger(
            mutation.before.as_ref(),
            &mutation.after,
            &blockers,
            &mutation.author,
            mutation.assign_gesture,
            &fresh,
            &roles,
        ) else {
            continue;
        };

        let task = mutation.after.id.clone();
        // A later milestone's task waits, quietly: the explicit dispatch path refuses the same
        // case with a sentence (`plan_dispatch`), and an assignment is a record of intent that
        // starts when its milestone is active. (M83)
        if let Some(why) = cide_agents::milestones::outside_active(&milestones, &board, &task) {
            tracing::debug!(%project, %task, %why, "assignment recorded; its milestone is not active");
            continue;
        }
        // The adoption write (a mention on an unassigned task becomes the assignee) is applied
        // here, inline, and is structurally incapable of re-triggering: only the two collection
        // sites build `TaskMutation`s, and this write goes through neither.
        if let Some(agent) = trigger.assign {
            adopt(app, project, &task, agent, &mutation.author);
        }

        for agent in trigger.dispatch {
            // The shortcut, not the guard — see the module header. It spares a spawn, a
            // `load_project` and a board read on the ordinary repeated gesture, and it is where
            // that case is logged with the burst still in hand. The funnel refuses what beats it.
            if registry.run_holding(project, &agent, &task).is_some() {
                tracing::debug!(
                    %project, agent = %agent, task = %task,
                    "assignment/mention repeated while a run holds the task; not stacking a second"
                );
                continue;
            }
            dispatch(app, project, agent, task.clone(), notify.clone());
        }
    }
}

/// Write the mention's adoption into the task, and tell the windows.
fn adopt(app: &AppHandle, project: ProjectId, task: &TaskId, agent: AgentId, author: &TaskAuthor) {
    let Some(stores) = app.try_state::<Arc<TasksStores>>() else {
        return;
    };
    let Some(store) = stores.get(project) else {
        return;
    };
    let edit = TaskEdit::Assign {
        agent: Some(agent.clone()),
    };
    match store.edit(task, edit, author.clone()) {
        Ok(_) => {
            tracing::info!(%project, %task, agent = %agent, "a mention assigned the task");
            crate::tasks_state::broadcast(app, project, &store);
        }
        // Deleted between the comment landing and this write — an ordinary race, not a failure.
        Err(error) => tracing::debug!(%task, %error, "a mention's assignment found no task"),
    }
}

/// One auto-dispatch, on its own task so nothing here waits on the queue.
fn dispatch(app: &AppHandle, project: ProjectId, agent: AgentId, task: TaskId, notify: RunNotify) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (Some(workspace), Some(agents), Some(tasks)) = (
            app.try_state::<WorkspaceState>(),
            app.try_state::<Arc<AgentRegistry>>(),
            app.try_state::<Arc<TasksStores>>(),
        ) else {
            return;
        };
        let request = DispatchRequest {
            project,
            agent: agent.clone(),
            task: Some(task.clone()),
            // The run is pointed at the task and reads it with `cide_task_get`; assignment adds
            // no extra line. Anything more is `cide_agent_dispatch`'s `instructions`.
            prompt: None,
            notify: Some(notify),
            external: None,
            harness: None,
            model: None,
        };
        match crate::cmd::agents::dispatch_or_duplicate(
            app.clone(),
            workspace,
            agents,
            tasks,
            request,
        )
        .await
        {
            Ok(DispatchOutcome::Started(run)) => {
                tracing::info!(%run, agent = %agent, %task, "an assignment started a subagent run");
            }
            // The race the shortcut above cannot win, landing where it is harmless. `debug!` and
            // not `warn!`: nothing went wrong — the guard did its job, and the gesture that got
            // us here already succeeded and already answered. (M66)
            Ok(DispatchOutcome::Duplicate { held, .. }) => {
                tracing::debug!(
                    run = %held.run, agent = %agent, %task,
                    "the funnel refused a second run beside the one already holding the task"
                );
            }
            // `plan_dispatch` owns every refusal (role gone, project disabled mid-flight,
            // `bypassPermissions` never authorised); here each one declines by doing nothing
            // louder than a line.
            Err(error) => {
                tracing::warn!(agent = %agent, %task, %error, "an assignment could not start the role");
            }
        }
    });
}

/// A run admitted onto a task moves that task `Todo → Doing` — and only that hop.
///
/// Called from the registry's `bring_up` once the child is live. `Doing`, `Review` and `Done`
/// are left alone: a reviewer role dispatched onto a `review` task must not yank it backwards,
/// and the same rule is what makes a respawn (which re-enters `bring_up`) idempotent here.
/// Authored as [`TaskAuthor::Orchestrator`] — the honest nearest fit, and since `SetStatus`
/// records its author into `Task::history` (M27) that is now a visible claim: the card's status
/// log shows the automatic `Todo → Doing` hop as the orchestrator's, which is who dispatched.
pub fn note_run_started(app: &AppHandle, project: ProjectId, task: &TaskId) {
    let Some(stores) = app.try_state::<Arc<TasksStores>>() else {
        return;
    };
    // `get`, not `ensure`: the dispatch that admitted this run already ensured the store.
    let Some(store) = stores.get(project) else {
        return;
    };
    if store.get(task).map(|task| task.status) != Some(TaskStatus::Todo) {
        return;
    }
    let edit = TaskEdit::SetStatus {
        status: TaskStatus::Doing,
    };
    match store.edit(task, edit, TaskAuthor::Orchestrator) {
        Ok(_) => {
            tracing::info!(%project, %task, "the run's task moved to doing");
            crate::tasks_state::broadcast(app, project, &store);
        }
        Err(error) => tracing::debug!(%task, %error, "could not move the run's task to doing"),
    }
}
