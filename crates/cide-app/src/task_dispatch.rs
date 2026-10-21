//! Current dispatch diagnostics. No task writes, enqueues or speculative worktrees.
use std::path::Path;

use cide_ipc::{
    AgentId, LlmSettings, ProjectId, RunState, Task, TaskDispatchReason as Reason,
    TaskDispatchState as Status, TaskDispatchStatus, TaskRow,
};

use crate::agents::AgentRegistry;

pub fn needs_notice(status: &TaskDispatchStatus) -> bool {
    matches!(
        status.state,
        Status::Blocked | Status::Failed | Status::Paused
    ) || (status.state == Status::Queued
        && status.reasons.iter().any(|reason| reason.code != "queue"))
}

pub fn status(
    root: &Path,
    registry: &AgentRegistry,
    project: ProjectId,
    task: &Task,
    board: &[TaskRow],
    target: Option<&AgentId>,
    llm: &LlmSettings,
) -> TaskDispatchStatus {
    let agent = target.or(task.agent.as_ref()).cloned();
    let loaded = cide_agents::load_project(root);
    let def = agent.as_ref().and_then(|id| loaded.get(id));
    let mut result = TaskDispatchStatus {
        project,
        task: task.id.clone(),
        agent: agent.clone(),
        label: def.map(|a| a.def.label.clone()),
        state: Status::Ready,
        run: None,
        reasons: Vec::new(),
    };
    // A holding run is the most useful answer, even if settings changed since it started.
    let runs = registry.runs_for(project);
    if let Some(run) = runs.iter().find(|run| {
        run.task.as_ref() == Some(&task.id)
            && Some(&run.agent) == agent.as_ref()
            && crate::agents::holds_a_pair(&run.state)
    }) {
        result.run = Some(run.run);
        result.label = Some(run.agent_label.clone());
        result.state = match run.state {
            RunState::Queued => Status::Queued,
            RunState::Idle => Status::Idle,
            RunState::Paused { .. } => Status::Paused,
            RunState::Blocked { .. } => Status::Blocked,
            _ => Status::Running,
        };
        if let RunState::Blocked { reason } = &run.state {
            result.reasons.push(Reason::new(
                "turnFailed",
                format!("{reason} Open its conversation to retry or change models."),
            ));
        } else if matches!(run.state, RunState::Queued) {
            result.reasons = registry.queue_reasons(run.run);
        } else if matches!(run.state, RunState::Paused { .. }) {
            result.reasons.push(Reason::new(
                "paused",
                "Agents are paused. Resume them in the Agents panel.",
            ));
        } else if matches!(run.state, RunState::AwaitingPermission) {
            result.reasons.push(Reason::new(
                "permission",
                "The agent is waiting for permission. Open its run to review the request.",
            ));
        } else if let Some(note) = &run.note {
            result.reasons.push(Reason::new("run", note.clone()));
        }
        return result;
    }
    let config = &loaded.config.agents;
    let reasons = &mut result.reasons;
    if !cide_agents::config::load_tracker(root) {
        reasons.push(Reason::new(
            "trackerDisabled",
            "The task tracker is disabled for this project.",
        ));
    }
    if !config.enabled {
        reasons.push(Reason::new(
            "agentsDisabled",
            "Subagents are off for this project. Enable them in the Agents panel.",
        ));
    }
    if !config.auto_dispatch {
        reasons.push(Reason::new("autoDispatchDisabled", "Automatic dispatch is off. Assignment and @mentions do not start agents; use an explicit dispatch or enable automatic dispatch."));
    }
    if task.status != cide_ipc::TaskStatus::Inbox
        && !cide_agents::task_dispatch::automatic_status(task.status)
    {
        reasons.push(Reason::new("status", format!("Tasks in {:?} do not start automatically. Assign a Todo or Doing task to start work.", task.status)));
    }
    reasons.extend(cide_agents::task_dispatch::task_reasons(
        task,
        board,
        &cide_agents::config::load_milestones(root),
    ));
    match (agent.as_ref(), def) {
        (None, _) => reasons.push(Reason::new("unassigned", "No role is assigned. Assign a role or @mention one to start work.")),
        (Some(id), None) => reasons.push(Reason::new("missingRole", format!("This project defines no role named {id}. Choose an available role in the Agents panel."))),
        (_, Some(def)) => {
            // The same committed permission and packaging gate as plan_dispatch.
            reasons.extend(cide_agents::dispatch_refusals(def, config, crate::agents::cide_hook_binary().as_deref())
                .into_iter().filter(|reason| reason.code != "agentsDisabled"));
            let overrides = cide_core::persist::load_agent_overrides(&cide_core::persist::agent_overrides_path())
                .project(&root.to_string_lossy());
            let resolved = cide_agents::overrides::resolve(def, &overrides, llm);
            if let Some(why) = resolved.refusal {
                reasons.push(Reason::new("configuration", why));
            }
        }
    }
    if !reasons.is_empty() {
        result.state = Status::Blocked;
    }
    // An old failure is history only when a newer attempt has already succeeded.
    if let Some(run) = runs
        .iter()
        .filter(|run| run.task.as_ref() == Some(&task.id) && Some(&run.agent) == agent.as_ref())
        .max_by_key(|run| run.started_unix_ms)
    {
        let why = match &run.state {
            RunState::Failed { reason } => Some(reason.clone()),
            RunState::Finished { code } if *code != 0 => {
                Some(format!("The last run exited with code {code}."))
            }
            RunState::Interrupted => {
                Some("The last run was interrupted. Resume it in the Agents panel.".into())
            }
            _ => None,
        };
        if let Some(why) = why {
            result.run = Some(run.run);
            result.reasons.push(Reason::new("lastRunFailed", why));
            if result.state == Status::Ready {
                result.state = Status::Failed;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{TaskAuthor, TaskId, TaskStatus};

    #[test]
    fn dispatch_status_explains_disabled_switches_and_question_without_writing_or_starting() {
        let root = std::env::temp_dir().join(format!("cide-dispatch-status-{}", ProjectId::new()));
        std::fs::create_dir_all(root.join(".cide")).unwrap();
        let config = root.join(".cide/config.json");
        let original = br#"{"version":1,"agents":{"enabled":false,"autoDispatch":false}}"#;
        std::fs::write(&config, original).unwrap();
        let task = Task {
            id: TaskId("t-1488".into()),
            title: "A user task".into(),
            body: String::new(),
            status: TaskStatus::Todo,
            agent: Some(AgentId("missing-feedback-role".into())),
            comments: Vec::new(),
            change: None,
            links: Vec::new(),
            session: None,
            history: Vec::new(),
            created_by: TaskAuthor::User,
            created_unix_ms: 0,
            updated_unix_ms: 0,
            attachments: Vec::new(),
            acceptance: None,
            question: Some("Choose a color".into()),
            touches: Vec::new(),
        };
        let registry = AgentRegistry::default();
        let project = ProjectId::new();
        let status = status(
            &root,
            &registry,
            project,
            &task,
            &[TaskRow::of(&task)],
            None,
            &LlmSettings::default(),
        );
        assert_eq!(status.state, Status::Blocked);
        let codes: Vec<_> = status.reasons.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "trackerDisabled",
                "agentsDisabled",
                "autoDispatchDisabled",
                "question",
                "missingRole"
            ]
        );
        assert!(needs_notice(&status));
        assert!(registry.runs_for(project).is_empty());
        assert_eq!(std::fs::read(config).unwrap(), original);
        assert!(!root.join(".cide/worktrees").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dispatch_feedback_does_not_warn_for_a_running_agent_or_transient_queue() {
        let mut status = TaskDispatchStatus {
            project: ProjectId::new(),
            task: TaskId("t-1".into()),
            agent: None,
            label: None,
            state: Status::Running,
            run: None,
            reasons: Vec::new(),
        };
        assert!(!needs_notice(&status));
        status.state = Status::Queued;
        status
            .reasons
            .push(Reason::new("queue", "Waiting for admission"));
        assert!(!needs_notice(&status));
        status
            .reasons
            .push(Reason::new("projectLimit", "Project is full"));
        assert!(needs_notice(&status));
        status.state = Status::Paused;
        assert!(needs_notice(&status));
        status.state = Status::Failed;
        assert!(needs_notice(&status));
    }
}
