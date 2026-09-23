//! Milestones: an ordered list of goals, each with a command that says whether it is met. (M83)
//!
//! # What this is for
//!
//! A board of tasks has no end. Closing one is progress only if something decides what the
//! project is *for*, and before this the only thing deciding that was a language model re-reading
//! the board every turn — which read a growing board as a busy one. Measured on a real project
//! (`~/work/selfcraft`): 232 tasks after four months, 193 of them created by the orchestrator,
//! and of the 98 still open only 36 were the plan.
//!
//! A milestone is a goal a **program** can check. `gate` is a shell command run in the project
//! root; exit 0 is *met*. Everything that moves the project is measured against the active
//! milestone's gate, and everything that does not is inbox (see `TaskStatus::Inbox`).
//!
//! # Who may change it
//!
//! The user. A model may *define* milestones once, on a project that has none — that is the
//! start of a project, where somebody has to write the first draft — and after that it may only
//! **propose**: `cide_milestones` records the proposal as a comment on the milestone's task and
//! the user edits them in the Tasks panel's Milestones tab. The reason is the one the whole feature rests on: a goal the agent
//! working towards it can rewrite is not a goal. For the same reason `guard_paths` names the files
//! a gate reads, and `cide_agent_integrate` refuses a branch that touches them.
//!
//! # Where it lives
//!
//! `.cide/config.json`, key `milestones` — beside `agents`, committed, hand-editable. The last
//! gate result is **not** there: it is a fact about one machine at one moment and lives in the
//! profile's state directory.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{ProjectId, TaskId};

/// A project's milestones, as `.cide/config.json` holds them and the Milestones tab edits them.
///
/// Every field defaults, so a hand-written `{"milestones": {"items": [...]}}` is a complete file —
/// and, more importantly, so a shape this build does not recognise costs only this key: the loader
/// reads `milestones` separately from `agents`, because a parse failure in `agents` disables
/// subagents for the whole project, and a typo in a gate command must not do that.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct MilestonePlan {
    /// In order. The first one not yet accepted is where the project is, unless [`Self::active`]
    /// says otherwise.
    pub items: Vec<Milestone>,
    /// The milestone being worked on, by id. `None` with items present means the first one.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// Run in a run's worktree before its work is integrated; exit 0 is *done*. Empty is none.
    ///
    /// A role saying it ran the checks is a report; this is the checks. It is what makes *review*
    /// mean the work passes, for every harness alike — no harness hook is involved, because codex
    /// and opencode have none cide may write.
    pub verify: String,
    /// How many open tasks (todo, doing, review) the active milestone may hold before a new one
    /// the orchestrator creates goes to the inbox instead. `None` is [`DEFAULT_MAX_OPEN`].
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_open: Option<u32>,
    /// Paths (a file, or a directory with a trailing `/`) relative to the project root that a
    /// gate reads. A branch that touches one is refused at integration: the work may not move the
    /// goal it is measured against.
    pub guard_paths: Vec<String>,
}

/// [`MilestonePlan::max_open`] when unset. Twelve because a milestone with more open work than
/// that at once is a milestone nobody can review, and because it is what a person can read in one
/// screen of the panel.
pub const DEFAULT_MAX_OPEN: u32 = 12;

/// How long a gate may run when a milestone does not say. Half an hour: a game's bot playthrough
/// plus its unit tiers is minutes, and a gate that hangs must end in a sentence, not a spinner.
pub const DEFAULT_GATE_TIMEOUT_SECS: u64 = 1_800;

/// One goal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Milestone {
    /// A short handle — `slice`, `p2` — that the gate command and the prompts name it by.
    pub id: String,
    pub title: String,
    /// The task that holds this goal on the board. Work towards it is `subtaskOf` it.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    /// Shell command, run with `sh -c` in the project root. Exit 0 is met.
    pub gate: String,
    /// Seconds. `number` on the wire, not ts-rs's `bigint`: a day is 86,400 and a JavaScript
    /// number holds that exactly, while a `bigint` would make every reader cast.
    #[ts(optional, type = "number")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

impl MilestonePlan {
    /// The milestone being worked on: [`Self::active`] when it names one that exists, otherwise
    /// the first item. `None` for a project with no milestones.
    pub fn current(&self) -> Option<&Milestone> {
        match &self.active {
            Some(id) => self
                .items
                .iter()
                .find(|m| &m.id == id)
                .or_else(|| self.items.first()),
            None => self.items.first(),
        }
    }

    pub fn max_open(&self) -> u32 {
        self.max_open.unwrap_or(DEFAULT_MAX_OPEN)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The milestone after `id`, in order.
    pub fn next_after(&self, id: &str) -> Option<&Milestone> {
        let at = self.items.iter().position(|m| m.id == id)?;
        self.items.get(at + 1)
    }

    /// Whether `path` (relative to the project root, `/`-separated) is one a gate reads.
    pub fn guards(&self, path: &str) -> bool {
        self.guard_paths.iter().any(|guard| {
            let guard = guard.trim().trim_start_matches("./");
            if guard.is_empty() {
                return false;
            }
            if let Some(dir) = guard.strip_suffix('/') {
                path == dir || path.starts_with(guard)
            } else {
                path == guard
            }
        })
    }
}

/// What one run of a gate or a verify command answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckResult {
    /// The command, as run.
    pub command: String,
    /// Exit 0 within the timeout.
    pub passed: bool,
    /// `None` when the process was killed (a timeout, a signal) rather than exiting.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The last lines of combined output, which is where every test runner prints its verdict.
    pub tail: String,
    #[ts(type = "number")]
    pub started_unix_ms: u64,
    #[ts(type = "number")]
    pub duration_ms: u64,
    /// The commit that was checked, when the directory is a git checkout.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

/// A milestone's gate, as last seen. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GateState {
    pub milestone: String,
    /// `None` when it has never run on this machine.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<CheckResult>,
    /// A run is in progress now.
    pub running: bool,
    /// The full output of the last (or current) run, as a path — for a model that can read a
    /// file; the panel asks `milestones_check_log`. Absent until it has run here.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

/// Everything the Milestones tab of the Tasks panel draws about a project's milestones. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MilestonesView {
    pub project: ProjectId,
    pub plan: MilestonePlan,
    /// One per milestone that has ever been run here, in [`MilestonePlan::items`] order.
    pub gates: Vec<GateState>,
    /// Ids of milestones the user accepted — their task is done.
    pub accepted: Vec<String>,
    /// The work under each milestone, one entry per milestone in [`MilestonePlan::items`] order —
    /// every task that is `subtaskOf` its task at any depth, done ones included so the tab can
    /// count them. (M83)
    pub tasks: Vec<MilestoneTasks>,
    /// Verify on each task's branch that this cide has run or is running, newest first. (M83) The
    /// board and the card draw it on the task, so a run's work being checked is visible where the
    /// work is rather than only as a merge that has not happened yet.
    pub verifies: Vec<VerifyState>,
    /// Changes agents have proposed to the milestones or to guarded files, oldest first, waiting
    /// for the user to accept or reject them. (M83)
    pub proposals: Vec<crate::proposals::Proposal>,
}

/// Verify on one task's branch. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VerifyState {
    pub task: TaskId,
    pub agent: crate::AgentId,
    /// It is running now; `last` is then the previous result, if any.
    pub running: bool,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<CheckResult>,
    /// The full output, as [`GateState::log`].
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

/// One milestone's tasks, as a tree flattened in board order. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MilestoneTasks {
    pub milestone: String,
    pub tasks: Vec<MilestoneTask>,
}

/// One task under a milestone: enough to draw a row and open the card. Outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MilestoneTask {
    pub id: TaskId,
    pub title: String,
    pub status: crate::TaskStatus,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<crate::AgentId>,
    /// 0 for a direct subtask of the milestone's task, 1 for its subtasks, and so on.
    pub depth: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(ids: &[&str]) -> MilestonePlan {
        MilestonePlan {
            items: ids
                .iter()
                .map(|id| Milestone {
                    id: (*id).into(),
                    title: format!("the {id} milestone"),
                    gate: format!("check {id}"),
                    ..Milestone::default()
                })
                .collect(),
            ..MilestonePlan::default()
        }
    }

    #[test]
    fn the_current_milestone_is_the_named_one_or_the_first() {
        let mut p = plan(&["slice", "p2"]);
        assert_eq!(p.current().map(|m| m.id.as_str()), Some("slice"));
        p.active = Some("p2".into());
        assert_eq!(p.current().map(|m| m.id.as_str()), Some("p2"));
        p.active = Some("gone".into());
        assert_eq!(
            p.current().map(|m| m.id.as_str()),
            Some("slice"),
            "a stale name falls back to the first rather than to nothing, which would switch off \
             every rule that keys off the active milestone"
        );
        assert_eq!(plan(&[]).current(), None);
        assert_eq!(p.next_after("slice").map(|m| m.id.as_str()), Some("p2"));
        assert_eq!(p.next_after("p2"), None);
    }

    #[test]
    fn a_guard_is_a_file_or_a_directory_and_never_a_prefix_of_a_name() {
        let p = MilestonePlan {
            guard_paths: vec!["tools/ci/milestone.sh".into(), "tests/gates/".into()],
            ..MilestonePlan::default()
        };
        assert!(p.guards("tools/ci/milestone.sh"));
        assert!(!p.guards("tools/ci/milestone.sh.bak"));
        assert!(p.guards("tests/gates/slice.gd"));
        assert!(p.guards("tests/gates"));
        assert!(
            !p.guards("tests/gates_old/x.gd"),
            "a directory guard is a directory, not a string prefix"
        );
        assert!(!p.guards("src/main.gd"));
    }

    #[test]
    fn a_hand_written_plan_needs_only_its_items() {
        let p: MilestonePlan =
            serde_json::from_str(r#"{"items":[{"id":"slice","gate":"true"}]}"#).expect("parses");
        assert_eq!(p.items[0].title, "");
        assert_eq!(p.max_open(), DEFAULT_MAX_OPEN);
        assert!(p.verify.is_empty());
    }
}
