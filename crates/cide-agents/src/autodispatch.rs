//! Decides whether one task mutation should start a role working.
//!
//! This is the policy behind "assigning a task starts the agent" and "@mentioning a role starts
//! it on the task" — pure, so every rule below is a table row in the tests rather than a claim
//! about a live registry. The app side (`cide-app`'s task triggers) supplies the mutation and
//! acts on the answer; nothing here spawns, reads disk, or knows whether a run already exists.
//! Deduplication against live runs is the registry's fact and is decided at the dispatch funnel
//! — `cide_app::cmd::agents::plan_dispatch` and `AgentRegistry::enqueue_unique`. This function
//! has no memory of a previous mutation at all, which the table below asserts outright.
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
//! * **A blocked task starts nothing until every blocker is `Done`.** (M30) The rule that makes
//!   `LinkType::BlockedBy` a real edge rather than the advisory one `cide_ipc::tasks`' header
//!   used to cut: an assignment or mention on a blocked task records intent exactly as one on a
//!   `review` task does, and the block clearing is not itself a trigger — somebody re-gestures,
//!   or the orchestrator dispatches, once the blocker lands. `Done` alone satisfies, not
//!   `Review`: review work can bounce back to `Doing`, and a dependent run dispatched against
//!   unaccepted work builds on a branch `integrate` may yet rewrite. The blockers arrive as a
//!   parameter ([`blocker_statuses`] is the caller's half) because this function is pure over
//!   one task and must stay a table row in the tests; a blocker that no longer exists is not in
//!   the slice at all, because a deleted task can never become `Done` and an edge that gated
//!   for ever would make deleting a task corrupt every task that pointed at it. Blocking gates
//!   **dispatch only** — status edits stay free, which is both how a wedge is broken by hand
//!   and why `spec_triggers`' auto-moves need no gate of their own.
//! * **An assign *gesture* always dispatches; an assignment merely carried along never does.**
//!   The rule used to be change-only — "re-saving the same assignee does not dispatch" — and the
//!   debug report showed why that reads wrong at the board: after a run died, re-picking the
//!   same assignee in the dropdown is the user's natural revive gesture, and it did nothing.
//!   So the caller now says *which* mutations were assignment gestures (`assign_gesture`: the
//!   dropdown, `cide_task_assign`, a creation naming an assignee), and those dispatch even with
//!   no change; a status flip or comment that happens to carry the assignee still fires nothing.
//!   Stacking is refused downstream and not here: `cide_app::cmd::agents::plan_dispatch` names
//!   the run already holding the task in a sentence, and `AgentRegistry::enqueue_unique` refuses
//!   it again under the lock that mints the id. Until M66 this line claimed the registry "always"
//!   did so, which was true of the auto-dispatch road alone — the panel's button and
//!   `cide_agent_dispatch` asked nothing, and the auto road's own check was not atomic. One
//!   assignment plus one @mention of the same role really did produce two runs on one task.
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

use cide_ipc::{AgentId, LinkType, Task, TaskAuthor, TaskRow, TaskStatus};

use crate::mentions;

/// The statuses of `task`'s live blockers that exist on `board` — the caller's half of the
/// blocking rule, kept beside the policy that consumes it. (M30)
///
/// A dangling blocker contributes nothing, deliberately: a deleted task can never become
/// [`TaskStatus::Done`] and its id is never reused, so an edge to one must not gate for ever —
/// see the module header. Tombstoned edges are not blockers at all.
#[must_use]
pub fn blocker_statuses(task: &Task, board: &[TaskRow]) -> Vec<TaskStatus> {
    task.links
        .iter()
        .filter(|l| l.link == LinkType::BlockedBy && !l.deleted)
        .filter_map(|l| board.iter().find(|t| t.id == l.target))
        .map(|t| t.status)
        .collect()
}

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
/// `blockers` is [`blocker_statuses`] over the board the mutation left behind — statuses rather
/// than a pre-chewed `bool`, so the Done-only satisfaction rule lives *here*, where the table
/// tests can see it, and not in every caller; `assign_gesture` says whether this mutation *was*
/// an assignment gesture (the dropdown, an explicit `Assign` edit, a creation naming an
/// assignee) rather than a save that carries the field along; `fresh_text` is the prose *this
/// mutation introduced*; `roles` is the loaded catalog's id set, used to filter mentions only —
/// see the module header for all of them.
pub fn trigger(
    before: Option<&Task>,
    after: &Task,
    blockers: &[TaskStatus],
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
    // After the status gate, before the mention scan: a blocked task's mention must not adopt
    // it either — an assignee written by a gesture that then starts nothing is exactly the
    // carried-along state the `assign_gesture` rule exists to keep inert.
    if blockers.iter().any(|s| *s != TaskStatus::Done) {
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
            change: None,
            links: Vec::new(),
            session: None,
            history: Vec::new(),
            created_by: TaskAuthor::User,
            created_unix_ms: 1,
            updated_unix_ms: 1,
            attachments: Vec::new(),
        }
    }

    fn roles(names: &[&str]) -> Vec<AgentId> {
        names.iter().map(|n| AgentId((*n).into())).collect()
    }

    fn ids(trigger: &Trigger) -> Vec<&str> {
        trigger.dispatch.iter().map(|id| id.0.as_str()).collect()
    }

    /// The blocking table: Done is the one status that satisfies, and the empty slice — no
    /// blockers, or only dangling ones — gates nothing. (M30)
    #[test]
    fn a_blocked_task_starts_nothing_until_its_blockers_are_done() {
        let after = task(TaskStatus::Todo, Some("qa"));
        let rows: &[(&[TaskStatus], bool)] = &[
            (&[], true),
            (&[TaskStatus::Done], true),
            (&[TaskStatus::Done, TaskStatus::Done], true),
            (&[TaskStatus::Todo], false),
            (&[TaskStatus::Doing], false),
            // Review blocks, deliberately: review work can bounce back to Doing, and a run
            // dispatched against it builds on a branch integrate may yet rewrite.
            (&[TaskStatus::Review], false),
            (&[TaskStatus::Done, TaskStatus::Doing], false),
        ];
        for (blockers, starts) in rows {
            let hit = trigger(
                None,
                &after,
                blockers,
                &TaskAuthor::User,
                true,
                &[],
                &roles(&["qa"]),
            );
            assert_eq!(
                hit.is_some(),
                *starts,
                "blockers {blockers:?} must {}start the role",
                if *starts { "" } else { "not " }
            );
        }

        // And a mention on a blocked task neither adopts nor starts — an assignee written by a
        // gesture that then starts nothing is the carried-along state the gesture rule keeps
        // inert.
        let unassigned = task(TaskStatus::Todo, None);
        assert_eq!(
            trigger(
                None,
                &unassigned,
                &[TaskStatus::Doing],
                &TaskAuthor::User,
                false,
                &["@qa please"],
                &roles(&["qa"]),
            ),
            None
        );
    }

    #[test]
    fn blocker_statuses_ignores_a_blocker_that_no_longer_exists() {
        let mut blocked = task(TaskStatus::Todo, None);
        blocked.id = cide_ipc::TaskId("t-9".into());
        blocked.links = vec![
            cide_ipc::TaskLink {
                link: LinkType::BlockedBy,
                target: cide_ipc::TaskId("t-1".into()),
                deleted: false,
                at_unix_ms: 1,
            },
            // A dangling blocker: t-77 is on nobody's board. A deleted task can never become
            // Done and its id is never reused, so it must not gate for ever.
            cide_ipc::TaskLink {
                link: LinkType::BlockedBy,
                target: cide_ipc::TaskId("t-77".into()),
                deleted: false,
                at_unix_ms: 1,
            },
            // A tombstoned edge is not a blocker at all.
            cide_ipc::TaskLink {
                link: LinkType::BlockedBy,
                target: cide_ipc::TaskId("t-2".into()),
                deleted: true,
                at_unix_ms: 2,
            },
        ];
        let board: Vec<TaskRow> = [task(TaskStatus::Doing, None), blocked.clone()]
            .iter()
            .map(TaskRow::of)
            .collect();
        assert_eq!(blocker_statuses(&blocked, &board), vec![TaskStatus::Doing]);
    }

    #[test]
    fn the_author_gate_refuses_a_runs_own_assign() {
        let after = task(TaskStatus::Todo, Some("qa"));
        let author = TaskAuthor::Agent {
            agent: AgentId("developer".into()),
            label: "Developer".into(),
        };
        assert_eq!(
            trigger(
                None,
                &after,
                &[],
                &author,
                true,
                &["@qa on it"],
                &roles(&["qa"])
            ),
            None,
            "a subagent assigned/mentioned a role and cide would have spawned it"
        );
    }

    #[test]
    fn only_todo_and_doing_tasks_start_work() {
        for status in [TaskStatus::Review, TaskStatus::Done] {
            let after = task(status, Some("qa"));
            assert_eq!(
                trigger(
                    None,
                    &after,
                    &[],
                    &TaskAuthor::User,
                    true,
                    &[],
                    &roles(&["qa"])
                ),
                None,
                "assigning a {status:?} task records intent, never spawns"
            );
        }
        let after = task(TaskStatus::Doing, Some("qa"));
        let hit = trigger(
            None,
            &after,
            &[],
            &TaskAuthor::User,
            true,
            &[],
            &roles(&["qa"]),
        )
        .expect("doing");
        assert_eq!(ids(&hit), ["qa"]);
    }

    #[test]
    fn an_assign_gesture_dispatches_and_a_carried_assignee_does_not() {
        // Creation with an assignee is a gesture...
        let after = task(TaskStatus::Todo, Some("qa"));
        let hit = trigger(
            None,
            &after,
            &[],
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
            &[],
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
            &[],
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
                &[],
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
                &[],
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
            &[],
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
            &[],
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
                &[],
                &TaskAuthor::User,
                false,
                &["@develoepr have a look"],
                &roles(&["developer"]),
            ),
            None,
            "a typo in prose rewrote the assignee field or reached the spawn path"
        );
    }

    /// **Where the dedupe is not.** (M66)
    ///
    /// [`an_edge_and_a_mention_in_one_mutation_deduplicate`] below proves the within-mutation
    /// dedupe, and to a hurried eye it reads like the whole story. It is not: this function is
    /// pure over *one* mutation and remembers nothing, so the same gesture arriving twice — a
    /// creation naming an assignee, then a comment @mentioning the same role — answers twice,
    /// deliberately and correctly. That pair is what produced the report of two runs on one
    /// task. The refusal lives downstream, in `cide_app::cmd::agents::plan_dispatch` and
    /// `AgentRegistry::enqueue_unique`; this row exists so that nobody reading only the file
    /// below concludes it lives here.
    #[test]
    fn nothing_here_remembers_a_previous_mutation_so_the_dedupe_must_live_downstream() {
        let after = task(TaskStatus::Todo, Some("game-designer"));
        let names = roles(&["game-designer"]);

        // The creation, naming an assignee.
        let first = trigger(
            None,
            &after,
            &[],
            &TaskAuthor::Orchestrator,
            true,
            &[],
            &names,
        )
        .expect("the assignment edge");
        assert_eq!(ids(&first), ["game-designer"]);

        // A separate mutation a moment later: a comment mentioning the same role. Nothing about
        // the first call is visible from here, and that is the design.
        let second = trigger(
            Some(&after),
            &after,
            &[],
            &TaskAuthor::Orchestrator,
            false,
            &["@game-designer ship the rosters"],
            &names,
        )
        .expect("the mention");
        assert_eq!(
            ids(&second),
            ["game-designer"],
            "this function must go on answering; the registry is what refuses the second run"
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
            &[],
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
