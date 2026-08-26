//! Decides whether one task mutation should start a role working.
//!
//! This is the policy behind "assigning a task starts the agent" and "@mentioning a role starts
//! it on the task" — pure, so every rule below is a table row in the tests rather than a claim
//! about a live registry. The app side (`cide-app`'s task triggers) supplies the mutation and
//! acts on the answer; nothing here spawns, reads disk, or knows whether a run already exists
//! (deduplication against live runs is the registry's fact and is checked there).
//!
//! The rules, each of which earned its place:
//!
//! * **The author gate is the security boundary.** `cide_task_assign` and `cide_task_comment`
//!   are in [`crate::tools::tool::ALL`], which is served to *dispatched runs* as well as to the
//!   orchestrating session — the two-vocabulary split in `tools.rs` exists precisely so a
//!   subagent cannot start a subagent. An ungated trigger would reopen that door through the
//!   side: a run assigns a task, cide spawns the role. So only [`TaskAuthor::User`] and
//!   [`TaskAuthor::Orchestrator`] trigger anything; a run's assign or mention stays what it
//!   always was — recorded intent the orchestrator reads.
//! * **Only `Todo` and `Doing` tasks start work.** Assigning a `review` or `done` task records
//!   who it is *for* — `Task::agent`'s documented meaning — and starts nothing; a reviewer role
//!   is put on a `review` task with an explicit `cide_agent_dispatch`.
//! * **An assign *gesture* always dispatches; an assignment merely carried along never does.**
//!   The rule used to be change-only — "re-saving the same assignee does not dispatch" — and the
//!   debug report showed why that reads wrong at the board: after a run died, re-picking the
//!   same assignee in the dropdown is the user's natural revive gesture, and it did nothing.
//!   So the caller now says *which* mutations were assignment gestures (`assign_gesture`: the
//!   dropdown, `cide_task_assign`, a creation naming an assignee), and those dispatch even with
//!   no change; a status flip or comment that happens to carry the assignee still fires nothing.
//!   Stacking is not a risk this reopens — the registry's `has_open_run` dedupe downstream is
//!   what refuses a duplicate while a run lives, and it always was.
//! * **Mentions are scanned only in the text the mutation introduced** (`fresh_text`: a new
//!   comment, a rewritten body, a creation body) — never in the stored task, or the mention in
//!   the body would re-fire on every later status flip.
//! * **A mention always starts the role; it assigns only a task nobody holds.** The user's
//!   chosen semantics: tagging `@qa` on a task `developer` already owns starts qa *alongside*,
//!   without stealing the assignee field; on an unassigned task the first mention also becomes
//!   the assignee.
//! * **Mentions are filtered against the loaded catalog, assignment edges are not.** A typo'd
//!   `@develoepr` in prose must not silently rewrite the assignee field, so only names the
//!   catalog knows can act; an *explicit* assignment to an unknown role was the caller's own
//!   deliberate write, is left alone here, and dies downstream in `dispatch_refusal` with a
//!   logged sentence.
//! * **Unassigning and reassigning stop nothing.** A live run killed mid-turn by a `<select>`
//!   change would be a destructive act with no confirm; `agents_stop` and the Stop button stay
//!   the only ways to end a run, and the run-end nudge is what surfaces any work left orphaned.

use cide_ipc::{AgentId, Task, TaskAuthor, TaskStatus};

use crate::mentions;

/// What one mutation asks the registry to do.
#[derive(Debug, PartialEq, Eq)]
pub struct Trigger {
    /// Write this into `Task::agent` first — a mention adopting an unassigned task.
    pub assign: Option<AgentId>,
    /// Start each of these on the task, in order, skipping any with a live run already.
    pub dispatch: Vec<AgentId>,
}

/// The decision. `None` when the mutation starts nothing.
///
/// `assign_gesture` says whether this mutation *was* an assignment gesture (the dropdown, an
/// explicit `Assign` edit, a creation naming an assignee) rather than a save that carries the
/// field along; `fresh_text` is the prose *this mutation introduced*; `roles` is the loaded
/// catalog's id set, used to filter mentions only — see the module header for all three.
pub fn trigger(
    before: Option<&Task>,
    after: &Task,
    author: &TaskAuthor,
    assign_gesture: bool,
    fresh_text: &[&str],
    roles: &[AgentId],
) -> Option<Trigger> {
    if !matches!(author, TaskAuthor::User | TaskAuthor::Orchestrator) {
        return None;
    }
    if !matches!(after.status, TaskStatus::Todo | TaskStatus::Doing) {
        return None;
    }

    let mut dispatch: Vec<AgentId> = Vec::new();

    // The assignment edge, widened by the gesture: creation-with-assignee counts
    // (`before: None`), a changed assignee counts, and an assign *gesture* counts even when it
    // names the role already on the task — the revive the module header describes. What still
    // fires nothing is a save that merely carries the assignee along (a status flip, a comment
    // — those arrive with `assign_gesture: false`), and an unassign, gesture or not.
    let previously = before.and_then(|task| task.agent.as_ref());
    if let Some(agent) = after.agent.as_ref()
        && (assign_gesture || previously != Some(agent))
    {
        dispatch.push(agent.clone());
    }

    let mut assign = None;
    for text in fresh_text {
        for mention in mentions::mentions_in(text) {
            if !roles.contains(&mention) {
                continue;
            }
            if after.agent.is_none() && assign.is_none() {
                assign = Some(mention.clone());
            }
            if !dispatch.contains(&mention) {
                dispatch.push(mention);
            }
        }
    }

    if dispatch.is_empty() {
        return None;
    }
    Some(Trigger { assign, dispatch })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: TaskStatus, agent: Option<&str>) -> Task {
        Task {
            id: cide_ipc::TaskId("t-1".into()),
            title: "a task".into(),
            body: String::new(),
            status,
            agent: agent.map(|a| AgentId(a.into())),
            comments: Vec::new(),
            history: Vec::new(),
            created_by: TaskAuthor::User,
            created_unix_ms: 1,
            updated_unix_ms: 1,
        }
    }

    fn roles(names: &[&str]) -> Vec<AgentId> {
        names.iter().map(|n| AgentId((*n).into())).collect()
    }

    fn ids(trigger: &Trigger) -> Vec<&str> {
        trigger.dispatch.iter().map(|id| id.0.as_str()).collect()
    }

    #[test]
    fn the_author_gate_refuses_a_runs_own_assign() {
        let after = task(TaskStatus::Todo, Some("qa"));
        let author = TaskAuthor::Agent {
            agent: AgentId("developer".into()),
            label: "Developer".into(),
        };
        assert_eq!(
            trigger(None, &after, &author, true, &["@qa on it"], &roles(&["qa"])),
            None,
            "a subagent assigned/mentioned a role and cide would have spawned it"
        );
    }

    #[test]
    fn only_todo_and_doing_tasks_start_work() {
        for status in [TaskStatus::Review, TaskStatus::Done] {
            let after = task(status, Some("qa"));
            assert_eq!(
                trigger(None, &after, &TaskAuthor::User, true, &[], &roles(&["qa"])),
                None,
                "assigning a {status:?} task records intent, never spawns"
            );
        }
        let after = task(TaskStatus::Doing, Some("qa"));
        let hit =
            trigger(None, &after, &TaskAuthor::User, true, &[], &roles(&["qa"])).expect("doing");
        assert_eq!(ids(&hit), ["qa"]);
    }

    #[test]
    fn an_assign_gesture_dispatches_and_a_carried_assignee_does_not() {
        // Creation with an assignee is a gesture...
        let after = task(TaskStatus::Todo, Some("qa"));
        let hit = trigger(
            None,
            &after,
            &TaskAuthor::Orchestrator,
            true,
            &[],
            &roles(&["qa"]),
        )
        .expect("edge");
        assert_eq!(
            hit,
            Trigger {
                assign: None,
                dispatch: roles(&["qa"])
            }
        );

        // ...a reassignment is one (and the old role is deliberately not stopped here)...
        let before = task(TaskStatus::Todo, Some("developer"));
        let hit = trigger(
            Some(&before),
            &after,
            &TaskAuthor::User,
            true,
            &[],
            &roles(&["developer", "qa"]),
        )
        .expect("reassign");
        assert_eq!(ids(&hit), ["qa"]);

        // ...and so is re-picking the assignee already on the task — the debug report's revive:
        // the run died, the user re-selects the same role, and under the old change-only rule
        // this exact call answered `None`. The registry's `has_open_run` is what keeps this from
        // stacking a second run while one lives.
        let hit = trigger(
            Some(&after),
            &after,
            &TaskAuthor::User,
            true,
            &[],
            &roles(&["qa"]),
        )
        .expect("the revive gesture");
        assert_eq!(ids(&hit), ["qa"]);

        // A save that merely carries the assignee along — a status flip, a comment — is not a
        // gesture and fires nothing; neither does an unassign, gesture or not.
        assert_eq!(
            trigger(
                Some(&after),
                &after,
                &TaskAuthor::User,
                false,
                &[],
                &roles(&["qa"])
            ),
            None
        );
        let unassigned = task(TaskStatus::Todo, None);
        assert_eq!(
            trigger(
                Some(&after),
                &unassigned,
                &TaskAuthor::User,
                true,
                &[],
                &roles(&["qa"])
            ),
            None
        );
    }

    #[test]
    fn a_mention_adopts_an_unassigned_task_and_only_joins_an_assigned_one() {
        // Unassigned: the first mention becomes the assignee, every mentioned role starts.
        let after = task(TaskStatus::Todo, None);
        let hit = trigger(
            None,
            &after,
            &TaskAuthor::User,
            false,
            &["@qa and @developer, please"],
            &roles(&["developer", "qa"]),
        )
        .expect("mentions");
        assert_eq!(hit.assign, Some(AgentId("qa".into())));
        assert_eq!(ids(&hit), ["qa", "developer"]);

        // Assigned: the mentioned role starts alongside; the assignee field is not stolen.
        let after = task(TaskStatus::Todo, Some("developer"));
        let hit = trigger(
            Some(&after),
            &after,
            &TaskAuthor::Orchestrator,
            false,
            &["@qa please verify"],
            &roles(&["developer", "qa"]),
        )
        .expect("mention beside an assignee");
        assert_eq!(
            hit,
            Trigger {
                assign: None,
                dispatch: roles(&["qa"])
            }
        );
    }

    #[test]
    fn a_mention_of_nothing_in_the_catalog_neither_assigns_nor_spawns() {
        let after = task(TaskStatus::Todo, None);
        assert_eq!(
            trigger(
                None,
                &after,
                &TaskAuthor::User,
                false,
                &["@develoepr have a look"],
                &roles(&["developer"]),
            ),
            None,
            "a typo in prose rewrote the assignee field or reached the spawn path"
        );
    }

    #[test]
    fn an_edge_and_a_mention_in_one_mutation_deduplicate() {
        // Created assigned to developer with a body that also mentions developer and qa: one
        // dispatch each, the edge's first.
        let after = task(TaskStatus::Todo, Some("developer"));
        let hit = trigger(
            None,
            &after,
            &TaskAuthor::User,
            true,
            &["@developer builds it, @qa checks it"],
            &roles(&["developer", "qa"]),
        )
        .expect("both");
        assert_eq!(
            hit,
            Trigger {
                assign: None,
                dispatch: roles(&["developer", "qa"])
            }
        );
    }
}
