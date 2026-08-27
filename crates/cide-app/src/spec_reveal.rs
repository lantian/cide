//! A change an agent just linked to a task opens as a tab. (M31)
//!
//! # What this reacts to, and why that signal and not another
//!
//! *Make a proposal* on a task card sends `/openspec-propose` into a conversation and, in the
//! same line, tells it to call `cide_task_update` with the folder name once the change exists —
//! `cmd::spec::spec_propose_for_task` explains why cide cannot name the change in advance. So
//! there is one precise moment at which cide learns a proposal is finished and what it is
//! called: **that task's `change` going from unset to set**. No polling, no watcher, no new tool,
//! and nothing that has to guess a directory name.
//!
//! The alternatives were both worse. Watching `openspec/changes/` for a new directory fires on a
//! `git pull`, a branch switch and a checkout of somebody else's proposal, none of which is a
//! thing the user just asked for. And having the conversation open the tab itself would be a new
//! MCP tool for something cide already knows.
//!
//! # Three conditions, and each one is doing work
//!
//! * **The task existed, and had no change.** `before` is `Some` with `change: None`. A *creation*
//!   naming a change is the other shape a `Some` in `after` can arrive as, and it is agent
//!   bookkeeping — a task filed against a change that already existed — not a proposal anybody is
//!   waiting to see. Firing on it would open tabs during unrelated work.
//! * **The author is not the user.** The card's own change control is a `User` edit, made inside
//!   a modal; opening a tab behind that scrim would be a tab nobody asked for at the one moment
//!   the user cannot see the strip. The propose road is `Orchestrator` or `Agent`.
//! * **The change is not the one that was already there.** Implied by the first condition, and
//!   stated in the code as a `None` check rather than an inequality so a *re-link* — unset,
//!   then set to something else — reads as the deliberate re-arm it is.
//!
//! # Idempotency is the transition, and there is nothing else
//!
//! `spec_triggers`' discipline, and its argument transfers whole: there is no ledger of changes
//! already revealed, no set to persist and nothing to rebuild after a restart. Unset-to-set
//! happens once in a task's ordinary life, and the only way to arm it again is to unlink the
//! change by hand — which is exactly the re-fire that should exist. The second and hundredth
//! mutation of the same task read `Some` in `before.change` and return.
//!
//! # Opening is idempotent too, one layer down
//!
//! [`cmd::spec::open_subject_tab`] activates the tab this subject already has rather than
//! minting a second one, so even a burst that somehow contained two qualifying mutations for one
//! change ends with one tab. That is why this calls it instead of building a tab of its own.
//!
//! # Everything declines by doing nothing
//!
//! `task_triggers`' rule, inherited for its reason: the gesture that got here — an MCP call from
//! a conversation — has already succeeded and already answered. A project that has closed, a
//! workspace mutation that refused, a task deleted mid-burst: each is a log line. There is no
//! surface on which a failure to *reveal* something could be reported that would not be worse
//! than the silence.

use cide_ipc::{ProjectId, SpecSubject, TaskAuthor};
use tauri::{AppHandle, Manager as _};

use crate::task_triggers::TaskMutation;
use crate::workspace_state::WorkspaceState;

/// Open the change tab for every mutation in this burst that just linked one.
///
/// Called from `task_triggers::consider`'s blocking half, which is the one funnel both mutation
/// roads pass through — the panel's commands and the agent-RPC socket. Runs there rather than on
/// the caller's thread because taking the workspace lock is not something an RPC connection
/// thread should do while an agent waits on the answer.
pub fn consider(app: &AppHandle, project: ProjectId, mutations: &[TaskMutation]) {
    for linked in mutations.iter().filter_map(newly_linked) {
        reveal(app, project, linked);
    }
}

/// The change this mutation just linked to an existing task, if it did. Pure — see the header
/// for what each condition is keeping out.
fn newly_linked(mutation: &TaskMutation) -> Option<cide_ipc::ChangeName> {
    if matches!(mutation.author, TaskAuthor::User) {
        return None;
    }
    // `Some` with no change: the task existed and was not tracking one. A creation (`None`) is
    // deliberately not this, and the header says why.
    let before = mutation.before.as_ref()?;
    if before.change.is_some() {
        return None;
    }
    mutation.after.change.clone()
}

fn reveal(app: &AppHandle, project: ProjectId, change: cide_ipc::ChangeName) {
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return;
    };
    let subject = SpecSubject::Change {
        change: change.clone(),
    };
    match crate::cmd::spec::open_subject_tab(&state, project, subject) {
        Ok(tab) => {
            tracing::info!(%project, %change, %tab, "a proposal finished — opening its change tab")
        }
        // The project closed while the conversation was writing, most likely. Nothing to say to
        // anybody: see the header's last section.
        Err(error) => {
            tracing::debug!(%project, %change, %error, "not opening a tab for a linked change")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{AgentId, ChangeName, Task, TaskId, TaskStatus};

    fn task(change: Option<&str>) -> Task {
        Task {
            id: TaskId::from("t-1".to_string()),
            title: "Add dark mode".into(),
            body: String::new(),
            status: TaskStatus::Todo,
            agent: None,
            session: None,
            change: change.map(|name| ChangeName::from(name.to_string())),
            links: Vec::new(),
            comments: Vec::new(),
            history: Vec::new(),
            created_by: TaskAuthor::User,
            created_unix_ms: 0,
            updated_unix_ms: 0,
        }
    }

    fn mutation(before: Option<Task>, after: Task, author: TaskAuthor) -> TaskMutation {
        TaskMutation {
            before,
            after,
            author,
            assign_gesture: false,
            fresh_text: Vec::new(),
        }
    }

    /// The propose road: an existing task with no change acquires one, written by the
    /// conversation cide asked. This is the whole feature.
    #[test]
    fn an_agent_linking_a_change_to_an_existing_task_reveals_it() {
        let m = mutation(
            Some(task(None)),
            task(Some("add-dark-mode")),
            TaskAuthor::Orchestrator,
        );
        assert_eq!(
            newly_linked(&m).map(|c| c.as_str().to_string()),
            Some("add-dark-mode".to_string())
        );
    }

    /// A subagent is the other non-user author, and it must behave identically — the propose
    /// line can be answered by either, depending on which conversation it went to.
    #[test]
    fn a_subagent_counts_as_much_as_the_orchestrator() {
        let m = mutation(
            Some(task(None)),
            task(Some("add-dark-mode")),
            TaskAuthor::Agent {
                agent: AgentId::from("developer".to_string()),
                label: "Developer".into(),
            },
        );
        assert!(newly_linked(&m).is_some());
    }

    /// The card's own control. A tab opening behind the modal the user is standing in is a tab
    /// they did not ask for at the one moment they cannot see the strip.
    #[test]
    fn the_user_picking_a_change_by_hand_opens_nothing() {
        let m = mutation(
            Some(task(None)),
            task(Some("add-dark-mode")),
            TaskAuthor::User,
        );
        assert!(newly_linked(&m).is_none());
    }

    /// The condition that makes this need no ledger: every later mutation of the same task finds
    /// a change already in `before` and declines, however many arrive and in whatever order.
    #[test]
    fn a_task_that_already_had_a_change_never_fires_again() {
        let m = mutation(
            Some(task(Some("add-dark-mode"))),
            task(Some("add-dark-mode")),
            TaskAuthor::Orchestrator,
        );
        assert!(newly_linked(&m).is_none());

        // Including a re-link to a *different* change, which is a rename or a correction and not
        // a proposal finishing. The user unlinking first is what re-arms this.
        let moved = mutation(
            Some(task(Some("add-dark-mode"))),
            task(Some("add-light-mode")),
            TaskAuthor::Orchestrator,
        );
        assert!(newly_linked(&moved).is_none());
    }

    /// A creation naming a change is an agent filing a task against something that already
    /// exists — bookkeeping, not a proposal anybody is waiting on.
    #[test]
    fn a_creation_that_names_a_change_is_not_a_proposal_finishing() {
        let m = mutation(None, task(Some("add-dark-mode")), TaskAuthor::Orchestrator);
        assert!(newly_linked(&m).is_none());
    }

    /// Unlinking is the re-arm, and it must not itself open anything.
    #[test]
    fn unlinking_reveals_nothing() {
        let m = mutation(
            Some(task(Some("add-dark-mode"))),
            task(None),
            TaskAuthor::Orchestrator,
        );
        assert!(newly_linked(&m).is_none());
    }
}
