//! Task gates shared by explicit dispatch and its read-only explanation.
use cide_ipc::{LinkType, MilestonePlan, Task, TaskDispatchReason, TaskRow, TaskStatus};

pub fn automatic_status(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Todo | TaskStatus::Doing)
}

pub fn task_reasons(
    task: &Task,
    board: &[TaskRow],
    plan: &MilestonePlan,
) -> Vec<TaskDispatchReason> {
    let mut reasons = Vec::new();
    if task.status == TaskStatus::Inbox {
        reasons.push(TaskDispatchReason::new("inbox", format!(
            "{} is in the inbox, which is where noticed work waits until the user decides it is work now: only the user moves a task out of the inbox — name it in your report, or ask the user.", task.id)));
    }
    if let Some(question) = task.question.as_deref() {
        reasons.push(TaskDispatchReason::new("question", format!(
            "{} is waiting for the user's answer to its question (\"{}\"); it starts again when the user answers.",
            task.id, question.lines().next().unwrap_or_default())));
    }
    if let Some(why) = crate::milestones::outside_active(plan, board, &task.id) {
        let mut reason = TaskDispatchReason::new("inactiveMilestone", why);
        reason.milestone =
            crate::milestones::inside_milestone(plan, board, &task.id).map(|m| m.id.clone());
        reasons.push(reason);
    }
    for blocker in task
        .links
        .iter()
        .filter(|link| link.link == LinkType::BlockedBy && !link.deleted)
        .filter_map(|link| board.iter().find(|row| row.id == link.target))
        .filter(|row| row.status != TaskStatus::Done)
    {
        let mut reason = TaskDispatchReason::new(
            "dependency",
            format!(
                "{} is blocked by {} — {} ({:?}). The dependency must be done; finish it first or remove the link if it is incorrect.",
                task.id, blocker.id, blocker.title, blocker.status
            ),
        );
        reason.task = Some(blocker.id.clone());
        reasons.push(reason);
    }
    reasons
}
