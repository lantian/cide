//! The rules that keep a board pointed at its milestone. (M83)
//!
//! Pure functions over rows and a [`MilestonePlan`], so every rule here is a table in a test and
//! none of them needs a store, a registry or a running app. `cide_ipc::milestones` carries the
//! argument for milestones existing at all; this is what they decide.
//!
//! # Where a new task goes
//!
//! [`placement`] answers it, and the answer is `Inbox` more often than a model expects:
//!
//! * **A run's task is always inbox.** A dispatched role is working on one task; what it notices
//!   on the way is an observation, not a decision that the project should now do it. Before this
//!   every such observation was a `Todo`, and the spinner's next planning turn read them as the
//!   plan.
//! * **The orchestrator's task is `Todo` when it serves the active milestone** — `subtaskOf` the
//!   milestone's task, directly or through a parent that is — **and the milestone has room**
//!   ([`MilestonePlan::max_open`]). Otherwise inbox, with a sentence saying which rule put it there
//!   so the model can link it and move it if it disagrees.
//! * **A project with no milestones behaves as before** for the orchestrator. The feature is
//!   opt-in by writing a plan, and a project without one gets only the first rule.
//!
//! # Why a duplicate is a comment
//!
//! [`duplicate_of`]: the same defect is noticed by every run that walks past it. Seven tasks for
//! one bug is seven rows to triage; one task with six *seen again* comments is one row whose
//! comment count says how much it matters.

use cide_ipc::{LinkType, MilestonePlan, TaskId, TaskRow, TaskStatus};

/// Where [`placement`] put a new task, and why when it was not where the caller might expect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub status: TaskStatus,
    /// A sentence for the tool's answer; `None` when the task went where it was asked to.
    pub reason: Option<String>,
}

impl Placement {
    fn todo() -> Self {
        Self {
            status: TaskStatus::Todo,
            reason: None,
        }
    }

    fn inbox(reason: impl Into<String>) -> Self {
        Self {
            status: TaskStatus::Inbox,
            reason: Some(reason.into()),
        }
    }
}

/// Where a task created through `cide_task_create` goes. See the module header.
///
/// `parent` is the task the new one is to be `subtaskOf`, if the call links one.
pub fn placement(
    by_run: bool,
    plan: &MilestonePlan,
    rows: &[TaskRow],
    parent: Option<&TaskId>,
) -> Placement {
    if by_run {
        return Placement::inbox(
            "it went to the inbox, because a task a run creates is something noticed on the way \
             and not yet a decision to do it; whoever plans the work moves it to todo when it is \
             needed.",
        );
    }
    let Some(current) = plan.current() else {
        return Placement::todo();
    };
    let Some(goal) = current.task.as_ref() else {
        // A milestone with no task on the board has nothing to be a subtask of, so the rule
        // cannot be followed and is not enforced. `cide_milestones define` always makes one.
        return Placement::todo();
    };
    let serves = parent.is_some_and(|p| p == goal || descends_from(rows, p, goal));
    if !serves {
        return Placement::inbox(format!(
            "it went to the inbox, because it is not part of the active milestone `{}` ({goal}). \
             If it is needed for that milestone, link it `subtaskOf` {goal} (or a subtask of it) \
             and set it to todo.",
            current.id
        ));
    }
    let open = open_under(rows, goal);
    let cap = plan.max_open() as usize;
    if open >= cap {
        return Placement::inbox(format!(
            "it went to the inbox, because milestone `{}` already has {open} open tasks, its limit. \
             Finish, merge or move one out before adding more; then set this to todo.",
            current.id
        ));
    }
    Placement::todo()
}

/// Whether `task` is `subtaskOf` `goal`, through any chain of parents.
///
/// Bounded, because a hand-edited or merged tracker can carry a cycle (`cide_tasks::validate`'s
/// doc says how) and a walk that follows one does not end.
pub fn descends_from(rows: &[TaskRow], task: &TaskId, goal: &TaskId) -> bool {
    let mut at = task.clone();
    for _ in 0..32 {
        let Some(row) = rows.iter().find(|r| r.id == at) else {
            return false;
        };
        let Some(parent) = row
            .links
            .iter()
            .find(|l| l.link == LinkType::SubtaskOf && !l.deleted)
            .map(|l| l.target.clone())
        else {
            return false;
        };
        if &parent == goal {
            return true;
        }
        at = parent;
    }
    false
}

/// Open tasks (todo, doing, review) under `goal`, the goal itself not counted.
pub fn open_under(rows: &[TaskRow], goal: &TaskId) -> usize {
    rows.iter()
        .filter(|r| &r.id != goal)
        .filter(|r| {
            matches!(
                r.status,
                TaskStatus::Todo | TaskStatus::Doing | TaskStatus::Review
            )
        })
        .filter(|r| descends_from(rows, &r.id, goal))
        .count()
}

/// Whether `task` may be started while milestones are in force: it serves the active milestone,
/// or belongs to none. A task under a *later* milestone waits for it. `None` means it may.
pub fn outside_active(plan: &MilestonePlan, rows: &[TaskRow], task: &TaskId) -> Option<String> {
    let current = plan.current()?;
    let goal = current.task.as_ref()?;
    if task == goal || descends_from(rows, task, goal) {
        return None;
    }
    let later = plan.items.iter().filter(|m| m.id != current.id).find(|m| {
        m.task
            .as_ref()
            .is_some_and(|t| t == task || descends_from(rows, task, t))
    })?;
    Some(format!(
        "{task} belongs to milestone `{}`, and the active one is `{}` ({goal}). Work on a later \
         milestone waits until the user accepts the current one.",
        later.id, current.id
    ))
}

/// Every task under `goal`, as a tree in board order: `(depth, row)`, depth 0 for its direct
/// subtasks. (M83)
///
/// The whole tree and not the direct children, because work nests — a phase task under a
/// milestone carries its own subtasks — and a list that stopped one level down would hide exactly
/// the concrete work an agent is looking for. Bounded like [`descends_from`], and a row reached
/// twice (a merged cycle) is drawn once.
pub fn tree_under<'a>(rows: &'a [TaskRow], goal: &TaskId) -> Vec<(u32, &'a TaskRow)> {
    fn walk<'a>(
        rows: &'a [TaskRow],
        parent: &TaskId,
        depth: u32,
        seen: &mut Vec<TaskId>,
        out: &mut Vec<(u32, &'a TaskRow)>,
    ) {
        if depth > 16 {
            return;
        }
        for row in rows.iter().filter(|r| {
            r.links
                .iter()
                .any(|l| l.link == LinkType::SubtaskOf && !l.deleted && &l.target == parent)
        }) {
            if seen.contains(&row.id) {
                continue;
            }
            seen.push(row.id.clone());
            out.push((depth, row));
            walk(rows, &row.id, depth + 1, seen, out);
        }
    }
    let mut out = Vec::new();
    let mut seen = vec![goal.clone()];
    walk(rows, goal, 0, &mut seen, &mut out);
    out
}

/// The milestone `task` belongs to: the one whose task it is, or which it descends from.
pub fn milestone_of<'a>(
    plan: &'a MilestonePlan,
    rows: &[TaskRow],
    task: &TaskId,
) -> Option<&'a cide_ipc::Milestone> {
    plan.items.iter().find(|m| {
        m.task
            .as_ref()
            .is_some_and(|goal| goal == task || descends_from(rows, task, goal))
    })
}

/// One line naming a task's milestone, for a model reading the task. `None` when it has none.
pub fn milestone_line(plan: &MilestonePlan, rows: &[TaskRow], task: &TaskId) -> Option<String> {
    let m = milestone_of(plan, rows, task)?;
    let active = plan.current().is_some_and(|c| c.id == m.id);
    let is_goal = m.task.as_ref() == Some(task);
    Some(format!(
        "Milestone: `{}` — {}{}{}.",
        m.id,
        m.title,
        m.task
            .as_ref()
            .filter(|_| !is_goal)
            .map(|t| format!(" (held by {t})"))
            .unwrap_or_default(),
        if is_goal {
            " — this task is the milestone itself"
        } else if active {
            " — the active milestone"
        } else {
            " — not the active milestone, so this waits"
        }
    ))
}

/// An open task whose title says the same thing as `title`, if there is one.
///
/// Jaccard similarity over normalised words of three letters or more, at [`SIMILAR`]. Crude on
/// purpose: it runs on every create, has to be explainable in one sentence to the model it
/// refuses, and its failure mode — two tasks for one defect — is the state before it existed.
/// Done tasks are not candidates, because *noticed again after it was fixed* is a regression and
/// deserves its own row.
pub fn duplicate_of(rows: &[TaskRow], title: &str) -> Option<TaskId> {
    let wanted = words(title);
    if wanted.len() < 2 {
        return None;
    }
    rows.iter()
        .filter(|r| r.status != TaskStatus::Done)
        .map(|r| (r, jaccard(&wanted, &words(&r.title))))
        .filter(|(_, score)| *score >= SIMILAR)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| r.id.clone())
}

/// The similarity at which two titles are one task. Measured against the titles on a real board
/// (the test corpus below): at 0.6 a reworded report of the same defect matches and two defects in
/// one subsystem do not.
pub const SIMILAR: f64 = 0.6;

fn words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_lowercase)
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Words that carry no identity in a task title.
const STOP: &[&str] = &[
    "the", "and", "for", "with", "from", "into", "that", "this", "when", "not", "are", "its",
    "but", "can", "does", "every", "all", "use", "via", "out",
];

fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let shared = a.iter().filter(|w| b.contains(w)).count();
    let union = a.len() + b.len() - shared;
    shared as f64 / union as f64
}

/// How many open tasks [`facts_line`] names before it says "and N more".
const FACT_TASKS: usize = 15;

/// A status as the tools spell it.
fn status_word(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Inbox => "inbox",
        TaskStatus::Todo => "todo",
        TaskStatus::Doing => "doing",
        TaskStatus::Review => "review",
        TaskStatus::Done => "done",
    }
}

/// One line of facts for a planning turn: what the active milestone is and how its gate stands.
/// Computed here rather than asked of the model, which would re-derive it from a board it reads
/// as a list.
pub fn facts_line(
    plan: &MilestonePlan,
    rows: &[TaskRow],
    gate: Option<&cide_ipc::CheckResult>,
) -> Option<String> {
    let current = plan.current()?;
    let inbox = rows
        .iter()
        .filter(|r| r.status == TaskStatus::Inbox)
        .count();
    let open = current
        .task
        .as_ref()
        .map(|goal| open_under(rows, goal))
        .unwrap_or(0);
    let gate = match gate {
        None => "its gate has not run yet".to_string(),
        Some(result) if result.passed => "its gate PASSES".to_string(),
        Some(result) => {
            let last: Vec<&str> = result
                .tail
                .lines()
                .filter(|l| !l.trim().is_empty())
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!(
                "its gate FAILS ({}) — last lines: {}",
                match (result.timed_out, result.exit_code) {
                    (true, _) => "timed out".to_string(),
                    (false, Some(code)) => format!("exit {code}"),
                    (false, None) => "killed".to_string(),
                },
                last.join(" / ")
            )
        }
    };
    // The open tasks themselves, not just their number (M83): a planning turn told "5 open" has
    // to go and find them, and one told which five does not. Capped, and the cap says so.
    let listed: Vec<String> = current
        .task
        .as_ref()
        .map(|goal| {
            tree_under(rows, goal)
                .into_iter()
                .filter(|(_, r)| {
                    matches!(
                        r.status,
                        TaskStatus::Todo | TaskStatus::Doing | TaskStatus::Review
                    )
                })
                .map(|(_, r)| {
                    let title: String = r.title.chars().take(60).collect();
                    format!("{} [{}] {title}", r.id, status_word(r.status))
                })
                .collect()
        })
        .unwrap_or_default();
    let shown = listed
        .iter()
        .take(FACT_TASKS)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let more = listed.len().saturating_sub(FACT_TASKS);
    let tasks = if listed.is_empty() {
        String::new()
    } else if more > 0 {
        format!(" Its open tasks: {shown}; and {more} more.")
    } else {
        format!(" Its open tasks: {shown}.")
    };
    Some(format!(
        "Facts from cide: the active milestone is `{}` ({}){}; {gate}. It has {open} open tasks \
         (limit {}); {inbox} tasks wait in the inbox.{tasks}",
        current.id,
        current.title,
        current
            .task
            .as_ref()
            .map(|t| format!(", held by {t}"))
            .unwrap_or_default(),
        plan.max_open()
    ))
}

/// What an agent asked `cide_milestone_proposals create` for, validated. (M83) The app reads the files'
/// current contents and diffs them; everything decidable from the arguments is decided here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalDraft {
    pub title: String,
    pub rationale: String,
    pub task: Option<TaskId>,
    pub change: DraftChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftChange {
    /// The whole plan proposed, with the tasks of milestones that already exist carried over.
    Plan(MilestonePlan),
    /// `(path, new content)`; `None` deletes.
    Files(Vec<(String, Option<String>)>),
    Note,
}

/// Validate `cide_milestone_proposals create`'s arguments against the plan as it stands.
///
/// **Files must be guarded paths.** Anything else is ordinary work and goes through a branch, a
/// review and verify like all work; a proposal is the road for what that road refuses, and a
/// queue that accepted any file would be a way round every check the branch road has.
pub fn proposal_draft(
    args: &serde_json::Value,
    current: &MilestonePlan,
) -> Result<ProposalDraft, String> {
    let text = |key: &str| {
        args.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let title = text("title").ok_or("`title` is required: one line naming the change")?;
    let rationale = text("rationale").ok_or(
        "`rationale` is required: the user decides from it, so say what is wrong now and why \
         this fixes it",
    )?;
    let task = text("task").map(TaskId);
    let has_plan = ["milestones", "verify", "guardPaths", "maxOpen"]
        .iter()
        .any(|k| args.get(*k).is_some_and(|v| !v.is_null()));
    let files = args.get("files").filter(|v| !v.is_null());
    let change = match (has_plan, files) {
        (true, Some(_)) => {
            return Err(
                "one proposal is one change: propose the milestones and the files separately"
                    .into(),
            );
        }
        (true, None) => DraftChange::Plan(proposed_plan(args, current)?),
        (false, Some(files)) => DraftChange::Files(proposed_files(files, current)?),
        (false, None) => DraftChange::Note,
    };
    Ok(ProposalDraft {
        title,
        rationale,
        task,
        change,
    })
}

fn proposed_plan(
    args: &serde_json::Value,
    current: &MilestonePlan,
) -> Result<MilestonePlan, String> {
    let mut plan = if args.get("milestones").is_some_and(|v| !v.is_null()) {
        crate::tools::milestone_plan_from(args)?
    } else {
        current.clone()
    };
    // A milestone that already exists keeps its task, or accepting would orphan its work.
    for item in &mut plan.items {
        if let Some(old) = current.items.iter().find(|m| m.id == item.id) {
            item.task = old.task.clone();
        }
    }
    plan.active = current
        .active
        .clone()
        .filter(|a| plan.items.iter().any(|m| &m.id == a))
        .or_else(|| plan.items.first().map(|m| m.id.clone()));
    if let Some(v) = args.get("verify").and_then(serde_json::Value::as_str) {
        plan.verify = v.trim().to_string();
    } else if args.get("milestones").is_some() {
        plan.verify = current.verify.clone();
    }
    if let Some(list) = args.get("guardPaths").and_then(serde_json::Value::as_array) {
        plan.guard_paths = list
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect();
    } else if args.get("milestones").is_some() {
        plan.guard_paths = current.guard_paths.clone();
    }
    match args.get("maxOpen").and_then(serde_json::Value::as_u64) {
        Some(n) => plan.max_open = Some(u32::try_from(n).unwrap_or(u32::MAX)),
        None => plan.max_open = current.max_open,
    }
    if plan == *current {
        return Err("that plan is the one the project already has".into());
    }
    Ok(plan)
}

fn proposed_files(
    files: &serde_json::Value,
    current: &MilestonePlan,
) -> Result<Vec<(String, Option<String>)>, String> {
    let list = files
        .as_array()
        .ok_or("`files` is a list of {path, content}")?;
    if list.is_empty() {
        return Err("`files` is empty".into());
    }
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for item in list {
        let path = item
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|p| p.trim().trim_start_matches("./").to_string())
            .filter(|p| !p.is_empty())
            .ok_or("every file needs a `path`")?;
        if path.starts_with('/') || path.split('/').any(|part| part == ".." || part.is_empty()) {
            return Err(format!(
                "`{path}` is not a plain path inside the project; give it relative to the root"
            ));
        }
        if !current.guards(&path) {
            return Err(format!(
                "`{path}` is not a guarded path ({}). Change it on a branch like any other work — \
                 a proposal is for what that road refuses.",
                if current.guard_paths.is_empty() {
                    "this project guards nothing".to_string()
                } else {
                    current.guard_paths.join(", ")
                }
            ));
        }
        if out.iter().any(|(p, _)| p == &path) {
            return Err(format!("`{path}` is named twice"));
        }
        let content = match item.get("content") {
            Some(serde_json::Value::String(text)) => Some(text.clone()),
            Some(serde_json::Value::Null) => None,
            _ => {
                return Err(format!(
                    "`{path}` needs `content`: the whole new text, or null"
                ));
            }
        };
        out.push((path, content));
    }
    Ok(out)
}

/// What the app knows about one verify it ran, or answered from an earlier run, before a merge —
/// the facts [`verify_ran_line`] and [`verify_refusal`] turn into sentences. (M92)
///
/// Built by `cide_app::milestones::before_integrate`; phrased here, where a test can construct
/// one, on the rule `tools::Integrated` states: the app decides, this crate phrases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport<'a> {
    /// `cide/<role>-<task>`.
    pub branch: &'a str,
    pub result: &'a cide_ipc::CheckResult,
    /// Answered from a run of the same commit that had already finished (the one started when
    /// the task went to review, usually) rather than run for this call.
    pub reused: bool,
    /// How long it queued behind another branch's verify under `agents.verifyExclusive`, and
    /// whose. `None` when it did not wait.
    pub waited: Option<(u64, &'a str)>,
    /// The other runs of this project that were alive while it ran: `role on task (where)`.
    pub live: &'a [String],
    /// Whether the project isolates any per-user directory (`agents.isolateEnv`).
    pub isolating: bool,
    /// Where the whole output is, when it was logged.
    pub log: Option<&'a str>,
}

/// `HH:MM:SS`, local — read beside the reader's own clock, and the same form as a plan's title.
fn clock(unix_ms: u64) -> String {
    use chrono::TimeZone;
    i64::try_from(unix_ms)
        .ok()
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .map(|at| at.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn span(ms: u64) -> String {
    match ms / 1000 {
        s if s < 60 => format!("{s}s"),
        s => format!("{}m{:02}s", s / 60, s % 60),
    }
}

/// One line saying when verify ran, whether it ran for this call, and what it queued behind.
///
/// # Why a verify that passed says it too, and why the times are wall-clock
///
/// The incident this answers: integrate refused one branch twice, four minutes apart, in
/// identical words. The second refusal was a cached verdict about the same commit, and nothing
/// in the text could say so — so a model retrying because "maybe it was the other runs" had no
/// way to learn that its retry had not retried anything. Start and end times are what a reader
/// can hold against the other runs' activity; "fresh" or "reused" is the question they asked.
#[must_use]
pub fn verify_ran_line(report: &VerifyReport<'_>) -> String {
    let result = report.result;
    let mut line = format!(
        "Verify ran {}–{} ({})",
        clock(result.started_unix_ms),
        clock(result.started_unix_ms.saturating_add(result.duration_ms)),
        if report.reused {
            "reused: it had already run on this commit; call integrate again to run it anew"
        } else {
            "fresh: it ran for this call"
        }
    );
    if let Some((ms, behind)) = report.waited {
        line.push_str(&format!(
            ", after waiting {} for `{behind}`'s verify",
            span(ms)
        ));
    }
    line.push('.');
    line
}

/// The refusal for a branch whose verify failed.
///
/// # Why it no longer says "hand it back"
///
/// It used to end its first sentence with *"Hand it back with this output as the feedback."*
/// In the incident that changed it (selfcraft, 2026-09-23) the branch was seventeen data files
/// and the failure was a guard in the project's own check noticing that some *other* worktree's
/// Godot had written the shared user directory. Following the instruction would have sent a
/// correct branch back to an author who could not fix it, and the re-run would have failed
/// again for the same reason: a retry loop that burns runs. The verdict is the project's
/// program's and the diagnosis is the reader's, so the text gives both roads and the facts to
/// choose between them — which other runs were alive, and whether the project isolates the
/// directories they share.
#[must_use]
pub fn verify_refusal(report: &VerifyReport<'_>) -> String {
    let result = report.result;
    let at = result
        .head
        .as_deref()
        .map(|h| format!(" at {}", &h[..h.len().min(10)]))
        .unwrap_or_default();
    let mut text = format!(
        "not merged: the verify command failed on {}{at}. Read the output: if the failure is in \
         this branch's change, hand it back; if it comes from the environment (other runs, \
         shared state), leave the task in review and retry.\n{}",
        report.branch,
        verify_ran_line(report)
    );
    if report.live.is_empty() {
        text.push_str("\nNo other run of this project was alive while it ran.");
    } else {
        text.push_str(&format!(
            "\nOther runs alive while it ran: {}.",
            report.live.join("; ")
        ));
        if !report.isolating {
            text.push_str(&format!(
                " They share this machine's per-user directories (~/.local/share, ~/.cache, …) \
                 with the verify; if the output points there, {} with `isolateEnv` gives each \
                 worktree its own.",
                crate::tools::tool::AGENTS_CONFIG
            ));
        }
    }
    if let Some(log) = report.log {
        text.push_str(&format!("\nThe whole output is in {log}."));
    }
    let ending = match (result.timed_out, result.exit_code) {
        (true, _) => "timed out".to_string(),
        (false, Some(code)) => format!("exit {code}"),
        (false, None) => "killed".to_string(),
    };
    let tail: Vec<&str> = result.tail.lines().collect();
    text.push_str(&format!(
        "\n\n`{}` — {ending}\n```\n{}\n```",
        result.command,
        tail[tail.len().saturating_sub(30)..].join("\n")
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{Milestone, TaskAuthor, TaskLink};

    fn row(id: &str, title: &str, status: TaskStatus, parent: Option<&str>) -> TaskRow {
        TaskRow {
            id: TaskId(id.into()),
            title: title.into(),
            status,
            agent: None,
            session: None,
            change: None,
            links: parent
                .map(|p| {
                    vec![TaskLink {
                        link: LinkType::SubtaskOf,
                        target: TaskId(p.into()),
                        deleted: false,
                        at_unix_ms: 1,
                    }]
                })
                .unwrap_or_default(),
            created_by: TaskAuthor::Orchestrator,
            created_unix_ms: 1,
            updated_unix_ms: 1,
            comment_count: 0,
            attachment_count: 0,
        }
    }

    fn plan(max_open: u32) -> MilestonePlan {
        MilestonePlan {
            items: vec![
                Milestone {
                    id: "slice".into(),
                    title: "The slice".into(),
                    task: Some(TaskId("t-1".into())),
                    gate: "true".into(),
                    timeout_secs: None,
                },
                Milestone {
                    id: "p2".into(),
                    title: "Content".into(),
                    task: Some(TaskId("t-2".into())),
                    gate: "true".into(),
                    timeout_secs: None,
                },
            ],
            max_open: Some(max_open),
            ..MilestonePlan::default()
        }
    }

    fn board() -> Vec<TaskRow> {
        vec![
            row("t-1", "Milestone slice", TaskStatus::Todo, None),
            row("t-2", "Milestone p2", TaskStatus::Todo, None),
            row(
                "t-3",
                "Bot finishes the run",
                TaskStatus::Doing,
                Some("t-1"),
            ),
            row("t-4", "Tune grub xp", TaskStatus::Todo, Some("t-3")),
            row("t-5", "Biome two", TaskStatus::Todo, Some("t-2")),
            row("t-6", "Old slice work", TaskStatus::Done, Some("t-1")),
        ]
    }

    #[test]
    fn a_runs_task_is_inbox_whatever_it_links() {
        let t1 = TaskId("t-1".into());
        let p = placement(true, &plan(12), &board(), Some(&t1));
        assert_eq!(p.status, TaskStatus::Inbox);
        let p = placement(true, &MilestonePlan::default(), &[], None);
        assert_eq!(p.status, TaskStatus::Inbox, "with or without milestones");
    }

    #[test]
    fn the_orchestrators_task_is_todo_only_under_the_active_milestone_with_room() {
        let rows = board();
        let t1 = TaskId("t-1".into());
        let t3 = TaskId("t-3".into());
        let t2 = TaskId("t-2".into());
        assert_eq!(
            placement(false, &plan(12), &rows, Some(&t1)).status,
            TaskStatus::Todo
        );
        assert_eq!(
            placement(false, &plan(12), &rows, Some(&t3)).status,
            TaskStatus::Todo,
            "a subtask of a subtask serves the milestone too"
        );
        let later = placement(false, &plan(12), &rows, Some(&t2));
        assert_eq!(
            later.status,
            TaskStatus::Inbox,
            "a later milestone's work waits"
        );
        assert!(later.reason.expect("says why").contains("t-1"));
        assert_eq!(
            placement(false, &plan(12), &rows, None).status,
            TaskStatus::Inbox
        );

        assert_eq!(
            open_under(&rows, &t1),
            2,
            "doing and todo count, done does not"
        );
        let full = placement(false, &plan(2), &rows, Some(&t1));
        assert_eq!(full.status, TaskStatus::Inbox);
        assert!(full.reason.expect("says why").contains("limit"));

        assert_eq!(
            placement(false, &MilestonePlan::default(), &rows, None).status,
            TaskStatus::Todo,
            "no milestones: the orchestrator plans as before"
        );
    }

    #[test]
    fn work_under_a_later_milestone_waits_and_unrelated_work_does_not() {
        let rows = board();
        let p = plan(12);
        assert!(outside_active(&p, &rows, &TaskId("t-4".into())).is_none());
        assert!(outside_active(&p, &rows, &TaskId("t-1".into())).is_none());
        let why = outside_active(&p, &rows, &TaskId("t-5".into())).expect("waits");
        assert!(why.contains("`p2`") && why.contains("`slice`"), "{why}");
        let mut loose = rows.clone();
        loose.push(row("t-9", "A loose chore", TaskStatus::Todo, None));
        assert!(
            outside_active(&p, &loose, &TaskId("t-9".into())).is_none(),
            "a task under no milestone is the orchestrator's call, not a later milestone's"
        );
    }

    #[test]
    fn a_cycle_in_the_parents_ends_the_walk() {
        let rows = vec![
            row("t-1", "a", TaskStatus::Todo, Some("t-2")),
            row("t-2", "b", TaskStatus::Todo, Some("t-1")),
        ];
        assert!(!descends_from(
            &rows,
            &TaskId("t-1".into()),
            &TaskId("t-9".into())
        ));
    }

    #[test]
    fn a_proposal_is_one_change_and_files_must_be_guarded() {
        let mut current = plan(12);
        current.guard_paths = vec!["tools/ci/milestone.sh".into(), "tests/gates/".into()];
        current.verify = "check".into();
        let base = |extra: serde_json::Value| {
            let mut v =
                serde_json::json!({ "title": "Add e2e", "rationale": "the slice gate misses e2e" });
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };

        let files = proposal_draft(
            &base(serde_json::json!({ "files": [{ "path": "./tools/ci/milestone.sh", "content": "x" }] })),
            &current,
        )
        .expect("a guarded file");
        assert_eq!(
            files.change,
            DraftChange::Files(vec![("tools/ci/milestone.sh".into(), Some("x".into()))])
        );
        for bad in ["src/main.gd", "../etc/passwd", "/abs", "tests/gates/../x"] {
            let why = proposal_draft(
                &base(serde_json::json!({ "files": [{ "path": bad, "content": "x" }] })),
                &current,
            )
            .expect_err(bad);
            assert!(why.contains(bad.trim_start_matches("./")), "{why}");
        }

        let plan_change = proposal_draft(
            &base(serde_json::json!({ "milestones": [
                { "id": "slice", "title": "The slice", "gate": "tools/ci/milestone.sh slice && e2e" },
                { "id": "p2", "title": "Content", "gate": "true" }
            ] })),
            &current,
        )
        .expect("a plan");
        let DraftChange::Plan(p) = plan_change.change else {
            panic!("a plan")
        };
        assert_eq!(
            p.items[0].task,
            Some(TaskId("t-1".into())),
            "an existing milestone keeps its task"
        );
        assert_eq!(
            p.verify, "check",
            "what the proposal did not name stays as it was"
        );

        assert!(
            proposal_draft(
                &base(serde_json::json!({ "verify": "check", "files": [] })),
                &current
            )
            .is_err(),
            "one change per proposal"
        );
        assert_eq!(
            proposal_draft(&base(serde_json::json!({})), &current)
                .expect("a note")
                .change,
            DraftChange::Note
        );
        assert!(
            proposal_draft(&serde_json::json!({ "title": "x" }), &current).is_err(),
            "needs a rationale"
        );
    }

    /// Titles from a real board (`~/work/selfcraft`): the pairs that are one defect match, the
    /// pairs that are two defects in one subsystem do not.
    #[test]
    fn duplicates_are_found_on_a_real_boards_titles() {
        let rows = vec![
            row(
                "t-219",
                "ScPlayerController.acquire_target queries a 9-px circle (TARGET_RANGE looks like tiles, not px)",
                TaskStatus::Todo,
                None,
            ),
            row(
                "t-229",
                "City avatar and Wayshrine launcher: use ScGame.active_profile_id() instead of the first saved profile",
                TaskStatus::Todo,
                None,
            ),
            row(
                "t-179",
                "ScTilePainter leaves one-tile-wide wall bands empty (grey holes)",
                TaskStatus::Todo,
                None,
            ),
            row(
                "t-100",
                "Fixed long ago: grey holes in walls",
                TaskStatus::Done,
                None,
            ),
        ];
        assert_eq!(
            duplicate_of(
                &rows,
                "Trainer: use ScGame.active_profile_id() instead of the first saved profile"
            ),
            None,
            "the same fix at a different call site (t-230 beside t-229 on that board) scores \
             0.55: two places are two tasks, and merging them is a planner's call, not a filter's"
        );
        assert_eq!(
            duplicate_of(
                &rows,
                "City avatar and Wayshrine launcher should use ScGame.active_profile_id(), not the first saved profile"
            ),
            Some(TaskId("t-229".into())),
            "a reworded report of the same place is the same task"
        );
        assert_eq!(
            duplicate_of(&rows, "ScTilePainter leaves one-tile-wide wall bands empty"),
            Some(TaskId("t-179".into()))
        );
        assert_eq!(
            duplicate_of(
                &rows,
                "Tile painter leaves most exposed wall cells unpainted (1-thick face ring can't match the 17-tile set)"
            ),
            None,
            "a second defect in the same painter is its own task"
        );
        assert_eq!(
            duplicate_of(&rows, "grey holes"),
            None,
            "too few words to judge"
        );
    }

    #[test]
    fn the_tree_under_a_milestone_is_every_level_in_board_order() {
        let rows = board();
        let tree: Vec<(u32, &str)> = tree_under(&rows, &TaskId("t-1".into()))
            .into_iter()
            .map(|(d, r)| (d, r.id.as_str()))
            .collect();
        assert_eq!(
            tree,
            [(0, "t-3"), (1, "t-4"), (0, "t-6")],
            "nested work is listed, done too"
        );

        let p = plan(12);
        assert_eq!(
            milestone_of(&p, &rows, &TaskId("t-4".into())).map(|m| m.id.as_str()),
            Some("slice")
        );
        assert_eq!(
            milestone_of(&p, &rows, &TaskId("t-5".into())).map(|m| m.id.as_str()),
            Some("p2")
        );
        let line = milestone_line(&p, &rows, &TaskId("t-5".into())).expect("has one");
        assert!(line.contains("`p2`") && line.contains("waits"), "{line}");
        let own = milestone_line(&p, &rows, &TaskId("t-1".into())).expect("the goal itself");
        assert!(own.contains("is the milestone itself"), "{own}");
        let mut loose = rows.clone();
        loose.push(row("t-9", "loose", TaskStatus::Todo, None));
        assert!(milestone_line(&p, &loose, &TaskId("t-9".into())).is_none());
    }

    #[test]
    fn the_facts_line_names_the_milestone_and_the_gate() {
        let rows = board();
        let red = cide_ipc::CheckResult {
            command: "gate".into(),
            passed: false,
            exit_code: Some(1),
            timed_out: false,
            tail: "ok 1\nbot: 12 kills, need 20\nFAILED\n".into(),
            started_unix_ms: 1,
            duration_ms: 1,
            head: None,
        };
        let line = facts_line(&plan(12), &rows, Some(&red)).expect("a plan");
        assert!(
            line.contains("`slice`") && line.contains("FAILS (exit 1)"),
            "{line}"
        );
        assert!(line.contains("need 20 / FAILED"), "{line}");
        assert!(line.contains("2 open tasks"), "{line}");
        assert!(
            line.contains(
                "Its open tasks: t-3 [doing] Bot finishes the run; t-4 [todo] Tune grub xp."
            ),
            "the open tasks are named, done ones are not: {line}"
        );
        assert!(facts_line(&MilestonePlan::default(), &rows, None).is_none());
    }

    fn failed_verify() -> cide_ipc::CheckResult {
        cide_ipc::CheckResult {
            command: "tools/dev/check.sh".into(),
            passed: false,
            exit_code: Some(1),
            timed_out: false,
            tail:
                "user_data_guard:   added: '.recovery_mode_lock'\nFAIL lint (real user dir touched)"
                    .into(),
            started_unix_ms: 1_790_000_000_000,
            duration_ms: 148_000,
            head: Some("0123456789abcdef".into()),
        }
    }

    /// The incident's refusal, rewritten: both roads, the named suspects, whether it re-ran.
    #[test]
    fn a_failed_verify_leaves_the_diagnosis_to_the_reader_and_names_the_suspects() {
        let result = failed_verify();
        let live = vec![
            "`worldgen-dev` on t-312 (.cide/worktrees/worldgen-dev-t-312)".to_string(),
            "`qa-tester` on t-301 (.cide/worktrees/qa-tester-t-301)".to_string(),
        ];
        let report = VerifyReport {
            branch: "cide/content-designer-t-295",
            result: &result,
            reused: false,
            waited: None,
            live: &live,
            isolating: false,
            log: Some("/state/verify-t-295.log"),
        };
        let text = verify_refusal(&report);
        assert!(!text.contains("Hand it back with this output"), "{text}");
        assert!(
            text.contains("failed on cide/content-designer-t-295 at 0123456789"),
            "{text}"
        );
        assert!(text.contains("if the failure is in this branch's change, hand it back"));
        assert!(text.contains("leave the task in review and retry"));
        assert!(text.contains("qa-tester-t-301") && text.contains("worldgen-dev-t-312"));
        assert!(
            text.contains("cide_agents_config with `isolateEnv`"),
            "the fix is named, as the tool that makes it: {text}"
        );
        assert!(text.contains("(fresh: it ran for this call)"), "{text}");
        assert!(text.contains(&format!(
            "Verify ran {}–{}",
            clock(result.started_unix_ms),
            clock(result.started_unix_ms + 148_000)
        )));
        assert!(text.contains("The whole output is in /state/verify-t-295.log."));
        assert!(text.contains("`tools/dev/check.sh` — exit 1"));
        assert!(text.contains(".recovery_mode_lock"));

        let isolated = verify_refusal(&VerifyReport {
            isolating: true,
            ..report.clone()
        });
        assert!(
            !isolated.contains("isolateEnv"),
            "no advice to turn on what is on"
        );
        let alone = verify_refusal(&VerifyReport {
            live: &[],
            ..report
        });
        assert!(
            alone.contains("No other run of this project was alive"),
            "{alone}"
        );
    }

    #[test]
    fn the_ran_line_says_reused_and_what_it_queued_behind() {
        let result = failed_verify();
        let line = verify_ran_line(&VerifyReport {
            branch: "cide/a-t-1",
            result: &result,
            reused: true,
            waited: Some((72_000, "cide/qa-tester-t-301")),
            live: &[],
            isolating: true,
            log: None,
        });
        assert!(
            line.contains("(reused: it had already run on this commit"),
            "{line}"
        );
        assert!(
            line.ends_with(", after waiting 1m12s for `cide/qa-tester-t-301`'s verify."),
            "{line}"
        );
    }
}
